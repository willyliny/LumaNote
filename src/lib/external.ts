/**
 * The only way the UI leaves the WebView. External links never navigate the window.
 * Later this calls the Tauri opener (system browser); until then it opens a new context.
 */
const ALLOWED = new Set(['http:', 'https:', 'mailto:']);

export function isSafeExternalUrl(raw: string): boolean {
  try {
    return ALLOWED.has(new URL(raw).protocol);
  } catch {
    return false;
  }
}

export function openExternal(raw: string): void {
  if (!isSafeExternalUrl(raw)) return;
  window.open(raw, '_blank', 'noopener,noreferrer');
}

/**
 * Safety net for every window: any anchor click is routed through openExternal, and
 * files dropped outside a drop zone must not make the WebView navigate to the file.
 */
export function installNavigationGuards(): void {
  document.addEventListener(
    'click',
    (e) => {
      const a = (e.target as Element | null)?.closest?.('a[href]');
      if (!a) return;
      const href = a.getAttribute('href') ?? '';
      if (href.startsWith('#')) return;
      e.preventDefault();
      openExternal(href);
    },
    true,
  );
  for (const type of ['dragover', 'drop'] as const) {
    window.addEventListener(type, (e) => e.preventDefault());
  }
}
