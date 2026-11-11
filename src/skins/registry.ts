/**
 * Skin registry.
 *
 * To add a skin: (1) the designer's folder docs/design/skins/<id>/ is complete,
 * (2) `npm run sync:skins` validates and copies it to src/assets/skins/<id>/,
 * (3) there is a line for it in SKINS below. Nothing else changes.
 *
 * Which assets exist is discovered at build time from src/assets/skins (import.meta.glob), so the
 * `complete` flag can never claim more than what is actually shipped.
 */
import type { MissingReason, Skin, SkinDeclaration, SkinJson, CompleteSkin, SkinFamily } from './types';
import { SKIN_STATES, type SkinState } from './states';
import { FAMILY_TOKENS_READY } from './family';

export const DEFAULT_SKIN_ID = 'leaf-cloak';

/** ---- the registry: one line per skin ---- */
export const SKINS: readonly SkinDeclaration[] = [
  { id: 'leaf-cloak', name: '葉披風', family: 'quiet-desk' },
  { id: 'moon-pebble', name: '月環石', family: 'quiet-desk' },
  { id: 'paper-lantern', name: '紙燈', family: 'quiet-desk' },
  { id: 'night-letter', name: '夜航', family: 'night-voyage' },
  { id: 'orbit-courier', name: '星軌信使', family: 'night-voyage' },
  { id: 'prism-scribe', name: '稜鏡書靈', family: 'night-voyage' },
];

/* ---- asset discovery (build time) ---- */
// `?no-inline` keeps every file a real file (no data: URIs), so a strict CSP stays possible.
const svgUrls = import.meta.glob('../assets/skins/*/*.svg', {
  eager: true,
  query: '?no-inline',
  import: 'default',
}) as Record<string, string>;
const skinJsons = import.meta.glob('../assets/skins/*/skin.json', {
  eager: true,
  import: 'default',
}) as Record<string, unknown>;

function urlFor(id: string, file: string): string | undefined {
  return svgUrls[`../assets/skins/${id}/${file}.svg`];
}

const HEX = /^#[0-9a-fA-F]{6}$/;
const pair = (v: unknown): v is { light: string; dark: string } =>
  !!v &&
  typeof v === 'object' &&
  HEX.test((v as Record<string, string>).light ?? '') &&
  HEX.test((v as Record<string, string>).dark ?? '');

/** Runtime re-check of skin.json (the sync script already did the strict one). */
export function parseSkinJson(raw: unknown, decl: SkinDeclaration): SkinJson | null {
  if (!raw || typeof raw !== 'object') return null;
  const o = raw as Record<string, unknown>;
  if (o.id !== decl.id || o.family !== decl.family) return null;
  if (typeof o.name !== 'string' || o.name.length === 0) return null;
  if (o.preferredTheme !== 'light' && o.preferredTheme !== 'dark') return null;
  if (o.accent !== undefined && !pair(o.accent)) return null;
  if (o.onAccent !== undefined && !pair(o.onAccent)) return null;
  if (o.glow !== undefined && !(typeof o.glow === 'string' && HEX.test(o.glow))) return null;
  return o as unknown as SkinJson;
}

export function resolveSkin(
  decl: SkinDeclaration,
  familyReady: (f: SkinFamily) => boolean = (f) => FAMILY_TOKENS_READY[f] === true,
): Skin {
  const missing: string[] = [];
  const reasons = new Set<MissingReason>();
  const states: Partial<Record<SkinState, string>> = {};
  for (const s of SKIN_STATES) {
    const u = urlFor(decl.id, s);
    if (u) states[s] = u;
    else {
      missing.push(`${s}.svg`);
      reasons.add('asset');
    }
  }
  const collapsed = urlFor(decl.id, 'collapsed');
  const avatar = urlFor(decl.id, 'avatar');
  if (!collapsed) {
    missing.push('collapsed.svg');
    reasons.add('asset');
  }
  if (!avatar) {
    missing.push('avatar.svg');
    reasons.add('asset');
  }
  const rawJson = skinJsons[`../assets/skins/${decl.id}/skin.json`];
  const json = rawJson === undefined ? null : parseSkinJson(rawJson, decl);
  if (rawJson === undefined) {
    missing.push('skin.json');
    reasons.add('asset');
  } else if (!json) {
    missing.push('skin.json (invalid)');
    reasons.add('metadata');
  }
  if (!familyReady(decl.family)) {
    missing.push(`${decl.family} palette`);
    reasons.add('family-tokens');
  }
  return {
    ...decl,
    preferredTheme: json?.preferredTheme ?? null,
    accent: json?.accent ?? null,
    onAccent: json?.onAccent ?? null,
    glow: json?.glow ?? null,
    assets: { states, collapsed, avatar },
    complete: missing.length === 0,
    missing,
    reasons: [...reasons],
  };
}

export const ALL_SKINS: readonly Skin[] = SKINS.map((d) => resolveSkin(d));

export function getSkin(id: string): Skin | undefined {
  return ALL_SKINS.find((s) => s.id === id);
}

export function isComplete(skin: Skin | undefined): skin is CompleteSkin {
  return !!skin && skin.complete;
}

/** The skin that must always be usable. If this is ever false the build is shipping broken art. */
export const DEFAULT_SKIN = getSkin(DEFAULT_SKIN_ID) as Skin;
