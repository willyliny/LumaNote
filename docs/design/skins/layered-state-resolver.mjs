/** Pure asset resolver; URLs are relative to the selected skin's layered directory. */
export function resolveLayeredState(base, manifest, state = 'idle', scale = '2x') {
  if (!['1x', '2x'].includes(scale)) throw new Error('Unsupported resolution');
  if (manifest.schemaVersion !== 1 || manifest.id !== base.id) throw new Error('Incompatible state manifest');
  const selected = Object.hasOwn(manifest.states, state) ? state : manifest.defaultState;
  const spec = manifest.states[selected];
  const layers = base.layers.filter(l => !spec.hiddenBaseLayers.includes(l.id)).map(l => ({
    ...l, src: `${l.file}@${scale}.png`,
    opacity: l.role === 'eyes-open' ? Number(spec.eyes === 'open') : l.role === 'eyes-closed' ? Number(spec.eyes === 'closed') : 1,
    motion: spec.motion === 'none' ? 'none' : l.role,
  }));
  if (spec.overlay) layers.push({...spec.overlay, src: `${spec.overlay.file}@${scale}.png`, opacity: 1, motion: 'none'});
  if (layers.length > manifest.maxMountedLayers) throw new Error('Layer budget exceeded');
  return {state: selected, label: spec.label, canvas: base.canvas, pivot: base.pivot, layers, blink: spec.blink, motion: spec.motion, composite: `${spec.composite}@${scale}.png`};
}
