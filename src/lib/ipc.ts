import type {
  CommandArgs,
  CommandName,
  CommandResult,
  EventMap,
  EventName,
  Unsubscribe,
} from './ipc-types';
import { createMockIpc } from './ipc-mock';
import { createTauriIpc } from './ipc-tauri';

/**
 * The only door between the UI and the backend. Mirrors Tauri's
 * `invoke(cmd, args)` / `listen(event, cb)` so the real implementation is a thin shim.
 */
export interface IpcClient {
  readonly kind: 'mock' | 'tauri';
  invoke<K extends CommandName>(cmd: K, args: CommandArgs<K>): Promise<CommandResult<K>>;
  listen<K extends EventName>(event: K, handler: (payload: EventMap[K]) => void): Unsubscribe;
}

/**
 * Mock vs real is chosen HERE and nowhere else.
 *  - VITE_IPC=tauri  -> real Tauri backend
 *  - anything else   -> in-memory mock (default until the backend exists)
 */
function selectIpc(): IpcClient {
  if (import.meta.env.VITE_IPC === 'tauri') return createTauriIpc();
  return createMockIpc();
}

export const ipc: IpcClient = selectIpc();
