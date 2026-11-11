export type ThemePref = 'system' | 'light' | 'dark';
const KEY = 'luma-theme';

function readPref(): ThemePref {
  try {
    const q = new URLSearchParams(location.search).get('theme');
    if (q === 'light' || q === 'dark') return q; // preview/screenshot override, not persisted
    const v = localStorage.getItem(KEY);
    if (v === 'light' || v === 'dark' || v === 'system') return v;
  } catch {
    /* storage unavailable: fall back to system */
  }
  return 'system';
}

function apply(pref: ThemePref) {
  const root = document.documentElement;
  if (pref === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', pref);
}

/** True when the URL forces a theme (screenshots/preview). Stored preferences must not override it. */
export function hasThemeOverride(): boolean {
  try {
    const q = new URLSearchParams(location.search).get('theme');
    return q === 'light' || q === 'dark';
  } catch {
    return false;
  }
}

export function getThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'light' || v === 'dark' || v === 'system') return v;
  } catch {
    /* ignore */
  }
  return 'system';
}

export function setThemePref(pref: ThemePref) {
  try {
    localStorage.setItem(KEY, pref);
  } catch {
    /* ignore */
  }
  apply(pref);
}

export function initTheme() {
  apply(readPref());
  window.addEventListener('storage', (e) => {
    if (e.key === KEY) apply(readPref());
  });
}
