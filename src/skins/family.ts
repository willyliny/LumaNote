import generated from './generated/family-tokens.json';
import type { SkinFamily } from './types';

/**
 * Which family palettes exist. quiet-desk is built into tokens.css; night-voyage is true only when
 * scripts/sync-skins.mjs found docs/design/skins/night-voyage-tokens.json and generated
 * src/styles/night-voyage.generated.css from it. The file is rewritten on every sync.
 */
export const FAMILY_TOKENS_READY: Record<SkinFamily, boolean> = {
  'quiet-desk': true,
  'night-voyage': (generated as { 'night-voyage'?: boolean })['night-voyage'] === true,
};
