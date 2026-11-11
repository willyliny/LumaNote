import '../styles/tokens.css';
import '../styles/base.css';
import { initTheme } from '../lib/theme';
import { installNavigationGuards } from '../lib/external';
import { startApp } from '../state/app-store';

/** Shared bootstrap for every window entry. */
export function boot() {
  initTheme();
  installNavigationGuards();
  startApp();
}
