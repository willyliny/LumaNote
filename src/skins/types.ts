import type { SkinState } from './states';
import type { LayeredArt } from './layered/schema';

export type SkinFamily = 'quiet-desk' | 'night-voyage';
export type SkinTheme = 'light' | 'dark';

/** Shape of a delivered skin.json (v2 brief). Validated by scripts/sync-skins.mjs and again here. */
export interface SkinJson {
  id: string;
  name: string;
  family: SkinFamily;
  preferredTheme: SkinTheme;
  accent?: { light: string; dark: string };
  onAccent?: { light: string; dark: string };
  glow?: string;
}

/** One registry line: what we know before any asset arrives. */
export interface SkinDeclaration {
  id: string;
  name: string;
  family: SkinFamily;
}

export type MissingReason =
  | 'asset' // a state SVG, collapsed.svg, avatar.svg or skin.json is absent
  | 'metadata' // skin.json invalid or disagrees with the registry line
  | 'family-tokens'; // the skin's family palette has not been delivered

export interface SkinAssets {
  /** present URLs only; a missing state is simply absent, never substituted */
  states: Partial<Record<SkinState, string>>;
  collapsed?: string;
  avatar?: string;
}

export interface Skin extends SkinDeclaration {
  preferredTheme: SkinTheme | null;
  accent: SkinJson['accent'] | null;
  onAccent: SkinJson['onAccent'] | null;
  glow: string | null;
  assets: SkinAssets;
  /**
   * 2.5D capability: the layered set, present only when layers.json + states.json validate and every
   * referenced PNG exists at @1x and @2x in the bundled assets. Computed, never declared.
   * Optional art: a skin without it is still complete and renders its flat SVG.
   */
  layered: LayeredArt | null;
  /** why a layered folder was ignored (dev tools and tests); null when absent or fine */
  layeredProblem: string | null;
  /** true only when every asset exists, skin.json is valid and the family palette is available */
  complete: boolean;
  /** human-readable list of what is missing (for dev tools and tests, not shown to users) */
  missing: string[];
  reasons: MissingReason[];
}

/** A skin that may be applied: all assets are guaranteed present. */
export interface CompleteSkin extends Skin {
  complete: true;
  assets: {
    states: Record<SkinState, string>;
    collapsed: string;
    avatar: string;
  };
}
