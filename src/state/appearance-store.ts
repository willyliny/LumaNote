/**
 * Appearance (skin + theme) state, shared by every window.
 *
 * Source of truth is the backend (`get_appearance` / `set_appearance`, event `appearance_changed`).
 * localStorage only caches the last applied skin id so a window does not flash the wrong art while
 * the first IPC round-trip is in flight.
 *
 * Interaction rules:
 *  - the active skin picks the token FAMILY (<html data-family>);
 *  - the user's theme (light | dark | system) still applies inside that family;
 *  - applying a skin never changes the theme (preferredTheme is only a hint);
 *  - skin.json accent/onAccent override the family's primary/onPrimary (CSS vars, see tokens.css);
 *  - an unknown / incomplete stored skin id falls back to the default with a visible notice,
 *    and is NOT written back to the backend.
 */
import { useStore, createStore } from '../lib/store';
import { ipc } from '../lib/ipc';
import { setThemePref, hasThemeOverride } from '../lib/theme';
import { t } from '../lib/strings';
import type { Appearance, ThemeChoice } from '../lib/ipc-types';
import { DEFAULT_SKIN_ID, getSkin, isComplete } from '../skins/registry';
import type { CompleteSkin } from '../skins/types';
import { notify, reportError } from './app-store';

const CACHE_KEY = 'luma-skin';

interface AppearanceState {
  loaded: boolean;
  /** what the backend has stored (may be unusable) */
  requestedSkinId: string;
  /** what is actually shown: always a complete skin */
  skinId: string;
  /** true when requestedSkinId could not be used */
  fallback: boolean;
  theme: ThemeChoice;
  saving: boolean;
}

function resolve(id: string): { skin: CompleteSkin; fallback: boolean } {
  const s = getSkin(id);
  if (isComplete(s)) return { skin: s, fallback: false };
  const d = getSkin(DEFAULT_SKIN_ID);
  if (!isComplete(d)) throw new Error(`default skin "${DEFAULT_SKIN_ID}" is incomplete: run npm run sync:skins`);
  return { skin: d, fallback: true };
}

function urlSkin(): string | null {
  try {
    const q = new URLSearchParams(location.search).get('skin'); // preview/screenshot override, not persisted
    return q && isComplete(getSkin(q)) ? q : null;
  } catch {
    return null;
  }
}

function cachedSkin(): string {
  const forced = urlSkin();
  if (forced) return forced;
  try {
    const v = localStorage.getItem(CACHE_KEY);
    if (v && isComplete(getSkin(v))) return v;
  } catch {
    /* ignore */
  }
  return DEFAULT_SKIN_ID;
}

export const appearanceStore = createStore<AppearanceState>({
  loaded: false,
  requestedSkinId: DEFAULT_SKIN_ID,
  skinId: DEFAULT_SKIN_ID,
  fallback: false,
  theme: 'system',
  saving: false,
});
export const useAppearance = <R>(select: (s: AppearanceState) => R): R => useStore(appearanceStore, select);

/** The skin to render. Hook form for components. */
export function useActiveSkin(): CompleteSkin {
  const id = useAppearance((s) => s.skinId);
  return resolve(id).skin;
}

/** Push the active skin's family + accent into the document. */
function applySkinToDocument(skin: CompleteSkin) {
  const root = document.documentElement;
  root.setAttribute('data-family', skin.family);
  root.setAttribute('data-skin', skin.id);
  const set = (name: string, v: string | undefined) =>
    v ? root.style.setProperty(name, v) : root.style.removeProperty(name);
  set('--skin-accent-light', skin.accent?.light);
  set('--skin-accent-dark', skin.accent?.dark);
  set('--skin-on-accent-light', skin.onAccent?.light);
  set('--skin-on-accent-dark', skin.onAccent?.dark);
  set('--skin-glow', skin.glow ?? undefined);
}

function commit(requested: string, theme: ThemeChoice, opts: { announceFallback?: boolean } = {}) {
  const forced = urlSkin();
  const { skin, fallback } = forced ? { skin: resolve(forced).skin, fallback: false } : resolve(requested);
  applySkinToDocument(skin);
  if (!hasThemeOverride()) setThemePref(theme);
  try {
    localStorage.setItem(CACHE_KEY, skin.id);
  } catch {
    /* ignore */
  }
  appearanceStore.set({ loaded: true, requestedSkinId: requested, skinId: skin.id, fallback, theme });
  if (fallback && opts.announceFallback) {
    const name = getSkin(requested)?.name;
    notify('info', t.skins.fallbackTitle, t.skins.fallbackDetail(name ?? requested, resolve(requested).skin.name));
  }
}

let started = false;
export function startAppearance() {
  if (started) return;
  started = true;
  // optimistic first paint from the cache, then the authoritative value
  const skin = resolve(cachedSkin()).skin;
  applySkinToDocument(skin);
  appearanceStore.set({ skinId: skin.id, requestedSkinId: skin.id });
  ipc.listen('appearance_changed', (e) => commit(e.appearance.skin_id, e.appearance.theme, { announceFallback: true }));
  void ipc
    .invoke('get_appearance', {})
    .then((a) => commit(a.skin_id, a.theme, { announceFallback: true }))
    .catch(() => {
      // backend unreachable: keep what we show, mark loaded so the UI is usable
      appearanceStore.set({ loaded: true });
    });
}

/** Persist. Resolves true on success. Only skin/theme are sent: no other setting is touched. */
export async function saveAppearance(patch: Partial<Appearance>): Promise<boolean> {
  const cur = appearanceStore.get();
  const next: Appearance = {
    skin_id: patch.skin_id ?? cur.requestedSkinId,
    theme: patch.theme ?? cur.theme,
  };
  appearanceStore.set({ saving: true });
  try {
    const saved = await ipc.invoke('set_appearance', next);
    commit(saved.skin_id, saved.theme);
    return true;
  } catch (e) {
    reportError(e);
    return false;
  } finally {
    appearanceStore.set({ saving: false });
  }
}
