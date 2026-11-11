import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';
import type { IpcClient } from './ipc';
import {
  IpcError,
  type CommandArgs,
  type CommandName,
  type CommandResult,
  type EventMap,
  type EventName,
  type IpcErrorKind,
  type Unsubscribe,
} from './ipc-types';

/**
 * Real Tauri bridge.
 *
 * Two layers live here:
 *  1. `createTauriIpc()`: the full IpcClient used when VITE_IPC=tauri (not the default yet).
 *  2. The "native window shim" (bottom of the file): while the mock backend is still the
 *     default, the mock opens windows with `window.open`. Inside Tauri that cannot work, so
 *     importing this module (ipc.ts always does) reroutes those calls to the real
 *     `open_window` command. Outside Tauri (plain browser) nothing is changed.
 */

/** Shape returned by the Rust `app_info` command (src-tauri/src/lib.rs). */
export interface AppInfo {
  app_name: string;
  app_version: string;
  tauri_version: string;
}

/** Window labels that exist on the Rust side. The UI says 'panel'; Tauri calls it 'main'. */
type NativeLabel = 'main' | 'settings';

const IPC_ERROR_KINDS: readonly IpcErrorKind[] = [
  'permission_denied',
  'authorization_required',
  'external_failure',
  'timeout',
  'cancelled',
  'outcome_uncertain',
  'not_found',
  'invalid_transition',
];

/** Tauri rejects with whatever the Rust command returned as Err (an object, or a string). */
function toIpcError(e: unknown): IpcError {
  if (e instanceof IpcError) return e;
  if (typeof e === 'object' && e !== null) {
    const o = e as { kind?: unknown; message?: unknown; retryable?: unknown };
    if (typeof o.kind === 'string' && IPC_ERROR_KINDS.includes(o.kind as IpcErrorKind)) {
      return new IpcError({
        kind: o.kind as IpcErrorKind,
        message: typeof o.message === 'string' ? o.message : o.kind,
        retryable: o.retryable === true,
      });
    }
  }
  return new IpcError({
    kind: 'external_failure',
    message: typeof e === 'string' ? e : e instanceof Error ? e.message : 'unknown Tauri error',
    retryable: false,
  });
}

async function tauriInvoke(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    throw toIpcError(e);
  }
}

/** Real `open_window`. */
function openNativeWindow(label: NativeLabel): Promise<unknown> {
  return tauriInvoke('open_window', { label });
}

/** Real `app_info`. Not part of CommandMap yet, so it is a plain helper, not `ipc.invoke`. */
export async function getAppInfo(): Promise<AppInfo> {
  return (await tauriInvoke('app_info', {})) as AppInfo;
}

/**
 * Result of the Rust `set_floating_expanded` command (src-tauri/src/floating.rs). Defined here,
 * not in ipc-types.ts, so this file does not depend on the frontend's CommandMap.
 * `sprite_offset_*` is the logical offset of the sprite centre from the window centre.
 */
export interface FloatingExpandedResult {
  width: number;
  height: number;
  sprite_offset_x: number;
  sprite_offset_y: number;
}

/** Real `set_floating_expanded`. Only the floating window is allowed to call it. */
export function setFloatingExpanded(expanded: boolean): Promise<FloatingExpandedResult> {
  return tauriInvoke('set_floating_expanded', { expanded }) as Promise<FloatingExpandedResult>;
}

/**
 * Commands that exist in Rust and must never be answered by the mock inside a Tauri window.
 * Add a name here when the Rust side gains the command.
 */
const NATIVE_COMMANDS: readonly string[] = ['set_floating_expanded'];

/**
 * Hybrid dispatch for "mock by default": returns `base` unchanged outside Tauri; inside Tauri,
 * routes NATIVE_COMMANDS to the real backend and everything else to `base`.
 * `selectIpc()` in ipc.ts wraps the mock with this: `return withNativeCommands(createMockIpc())`.
 */
export function withNativeCommands(base: IpcClient): IpcClient {
  if (typeof window === 'undefined' || !isTauri()) return base;
  return {
    kind: base.kind,
    invoke<K extends CommandName>(cmd: K, args: CommandArgs<K>): Promise<CommandResult<K>> {
      if (NATIVE_COMMANDS.includes(cmd)) {
        return tauriInvoke(cmd, args as Record<string, unknown>) as Promise<CommandResult<K>>;
      }
      return base.invoke(cmd, args);
    },
    listen: (event, handler) => base.listen(event, handler),
  };
}

export function createTauriIpc(): IpcClient {
  return {
    kind: 'tauri',
    async invoke<K extends CommandName>(cmd: K, args: CommandArgs<K>): Promise<CommandResult<K>> {
      if (cmd === 'open_window') {
        const { label } = args as CommandArgs<'open_window'>;
        // `tab` is not forwarded yet: the Rust command opens the window only.
        await openNativeWindow(label === 'panel' ? 'main' : 'settings');
        return {} as CommandResult<K>;
      }
      return (await tauriInvoke(cmd, args as Record<string, unknown>)) as CommandResult<K>;
    },
    listen<K extends EventName>(event: K, handler: (payload: EventMap[K]) => void): Unsubscribe {
      let off: (() => void) | null = null;
      let cancelled = false;
      void tauriListen<EventMap[K]>(event, (e) => handler(e.payload)).then((un) => {
        if (cancelled) un();
        else off = un;
      });
      return () => {
        cancelled = true;
        off?.();
      };
    },
  };
}

/* ------------------------------------------------------------------ */
/* Native window shim (mock backend inside a Tauri window)             */
/* ------------------------------------------------------------------ */

/** The mock calls `window.open(url, 'elf-panel' | 'elf-settings', ...)` (ipc-mock.ts open_window). */
const MOCK_WINDOW_TARGETS: Record<string, NativeLabel> = {
  'elf-panel': 'main',
  'elf-settings': 'settings',
};

function installNativeWindowShim(): void {
  const original = window.open.bind(window);
  window.open = ((url?: string | URL, target?: string, features?: string) => {
    const label = target ? MOCK_WINDOW_TARGETS[target] : undefined;
    if (label) {
      openNativeWindow(label).catch((e: unknown) => console.error('open_window failed', e));
      return null;
    }
    return original(url, target, features);
  }) as typeof window.open;
}

if (typeof window !== 'undefined' && isTauri()) {
  installNativeWindowShim();
  // Smoke test for the shell: show versions once per window. The floating window is not
  // allowed to call app_info (least privilege), so it is skipped.
  void (async () => {
    try {
      const label = (window as unknown as {
        __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } };
      }).__TAURI_INTERNALS__?.metadata?.currentWindow?.label;
      if (label === 'floating') return;
      console.info('[elf] app_info', await getAppInfo());
    } catch (e) {
      console.error('[elf] app_info failed', e);
    }
  })();
}

/**
 * Start a native drag of the current window (used by the floating sprite).
 * `data-tauri-drag-region` only fires when the pressed element itself carries the
 * attribute, and the sprite button covers the whole window, so the drag is started
 * explicitly once the pointer has moved past a small threshold.
 * Outside Tauri this is a no-op.
 */
export async function startWindowDrag(): Promise<void> {
  if (typeof window === 'undefined' || !isTauri()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().startDragging();
}
