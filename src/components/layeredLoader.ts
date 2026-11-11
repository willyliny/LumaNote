/**
 * Preloads every layer of a skin (base layers and all five state markers) at one scale, so a state
 * change can never show a missing image, and tracks failure so the caller can fall back to the flat art.
 * Status is cached per skin+scale for the life of the window; a failure (load error now or later, reported
 * by the <img> itself) is sticky for the skin.
 */
import { useSyncExternalStore } from 'react';
import type { ImageScale, LayeredArt } from '../skins/layered/schema';

export type LoadStatus = 'loading' | 'ready' | 'failed';

interface Entry {
  status: LoadStatus;
  imgs: HTMLImageElement[]; // keep references so decoded bitmaps are not dropped early
}
const entries = new Map<string, Entry>();
const failedSkins = new Set<string>();
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((f) => f());
const key = (art: LayeredArt, scale: ImageScale) => `${art.id}@${scale}`;

export function markLayeredFailed(art: LayeredArt, reason: string) {
  if (failedSkins.has(art.id)) return;
  failedSkins.add(art.id);
  console.error(`[sprite] layered art of "${art.id}" failed (${reason}); using the flat art`);
  emit();
}

function start(art: LayeredArt, scale: ImageScale): Entry {
  const e: Entry = { status: 'loading', imgs: [] };
  entries.set(key(art, scale), e);
  const jobs = art.files.map((file) => {
    const img = new Image();
    img.decoding = 'async';
    img.src = art.urls[file]?.[scale] ?? '';
    e.imgs.push(img);
    return img.decode();
  });
  Promise.all(jobs).then(
    () => {
      e.status = 'ready';
      emit();
    },
    (err) => {
      e.status = 'failed';
      markLayeredFailed(art, String(err));
      emit();
    },
  );
  return e;
}

export function layeredStatus(art: LayeredArt, scale: ImageScale): LoadStatus {
  if (failedSkins.has(art.id)) return 'failed';
  return (entries.get(key(art, scale)) ?? start(art, scale)).status;
}

/**
 * Status for the wanted scale. While the wanted scale loads, an already-loaded other scale is used
 * (e.g. the window moved to another monitor), so the sprite never blanks.
 */
export function useLayeredStatus(art: LayeredArt | null, wanted: ImageScale): { status: LoadStatus; scale: ImageScale } {
  const snap = useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => {
      if (!art) return 'none';
      const s = layeredStatus(art, wanted);
      if (s === 'ready' || s === 'failed') return `${s}:${wanted}`;
      const other: ImageScale = wanted === 1 ? 2 : 1;
      return entries.get(key(art, other))?.status === 'ready' ? `ready:${other}` : 'loading:' + wanted;
    },
    () => 'none',
  );
  if (snap === 'none') return { status: 'failed', scale: wanted };
  const [status, scale] = snap.split(':');
  return { status: status as LoadStatus, scale: Number(scale) as ImageScale };
}
