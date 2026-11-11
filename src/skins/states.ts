/**
 * The ONE place where the app's sprite state names meet the skin file names.
 * App side (components/spriteState.ts): idle | listening | working | awaiting | suggestion | dnd
 * Skin side (design contract, docs/briefs/astra-skins-v2-brief): idle | listening | processing |
 *   approval | suggestion | paused  ->  <state>.svg
 */
import type { SpriteState } from '../components/spriteState';

export type SkinState = 'idle' | 'listening' | 'processing' | 'approval' | 'suggestion' | 'paused';

export const SKIN_STATES: readonly SkinState[] = [
  'idle',
  'listening',
  'processing',
  'approval',
  'suggestion',
  'paused',
];

export const APP_TO_SKIN_STATE: Record<SpriteState, SkinState> = {
  idle: 'idle',
  listening: 'listening',
  working: 'processing',
  awaiting: 'approval',
  suggestion: 'suggestion',
  dnd: 'paused',
};
