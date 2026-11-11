import type { IpcClient } from './ipc';
import { IpcError } from './ipc-types';

/**
 * Placeholder for the real implementation. `@tauri-apps/api` is intentionally not
 * installed yet (versions are being verified). When it is, this becomes:
 *   invoke: (cmd, args) => tauriInvoke(cmd, args).catch(rethrowAsIpcError)
 *   listen: (event, cb) => tauriListen(event, e => cb(e.payload))
 */
export function createTauriIpc(): IpcClient {
  const notWired = () =>
    new IpcError({
      kind: 'external_failure',
      message: 'Tauri IPC is not wired yet (ipc-tauri.ts is a placeholder).',
      retryable: false,
    });
  return {
    kind: 'tauri',
    invoke: () => Promise.reject(notWired()),
    listen: () => {
      throw notWired();
    },
  };
}
