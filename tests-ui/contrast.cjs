/**
 * Contrast audit of the REAL computed colours (after color-mix and skin accent overrides).
 * Usage: node tests-ui/contrast.cjs [baseUrl]   (needs `vite preview`)
 * Pairs: every text token on canvas/surface/subtle/sunken, role text on its soft fill, on-primary on
 * primary, control border on surface (3:1, non-text), plus each skin's accent pair in both themes.
 * Also checks that tokens.css still equals docs/design/design-tokens.json (quiet-desk) and
 * docs/design/skins/night-voyage-tokens.json (night-voyage).
 */
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const BASE = process.argv[2] || 'http://localhost:4173';
const ROOT = path.join(__dirname, '..');

const lum = ([r, g, b]) => {
  const f = (v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
};
const ratio = (a, b) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};
const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));

(async () => {
  const skins = fs.readdirSync(path.join(ROOT, 'src/assets/skins')).filter((d) => fs.existsSync(path.join(ROOT, 'src/assets/skins', d, 'skin.json')));
  const designs = {
    'quiet-desk': JSON.parse(fs.readFileSync(path.join(ROOT, 'docs/design/design-tokens.json'), 'utf8')).themes,
    'night-voyage': JSON.parse(fs.readFileSync(path.join(ROOT, 'docs/design/skins/night-voyage-tokens.json'), 'utf8')).themes,
  };
  const browser = await chromium.launch();
  let fails = 0, total = 0;
  const lines = [];
  for (const skin of skins) {
    const meta = JSON.parse(fs.readFileSync(path.join(ROOT, 'src/assets/skins', skin, 'skin.json'), 'utf8'));
    for (const theme of ['light', 'dark']) {
      const pg = await (await browser.newContext()).newPage();
      await pg.goto(`${BASE}/settings.html?theme=${theme}&skin=${skin}`);
      await pg.waitForSelector('.settings');
      await pg.waitForTimeout(150);
      const c = await pg.evaluate(() => {
        const names = ['bg','surface','surface-2','sunken','text','text-2','text-3','primary','on-primary','primary-soft','on-primary-soft','attn','attn-soft','on-attn-soft','success','success-soft','on-success-soft','warn','warn-soft','on-warn-soft','danger','danger-soft','on-danger-soft','link','border-strong','focus'];
        const cv = document.createElement('canvas').getContext('2d', { willReadFrequently: true });
        const out = {};
        for (const n of names) {
          const d = document.createElement('div');
          d.style.cssText = `position:fixed;background:var(--c-${n})`;
          document.body.appendChild(d);
          const col = getComputedStyle(d).backgroundColor;
          d.remove();
          cv.clearRect(0, 0, 1, 1); cv.fillStyle = '#000'; cv.fillStyle = col; cv.fillRect(0, 0, 1, 1);
          out[n] = [...cv.getImageData(0, 0, 1, 1).data.slice(0, 3)];
        }
        const raw = {};
        for (const k of ['canvas','surface','subtle','text','muted','primary','on-primary','secondary','success','warning','error','border','focus']) raw[k] = getComputedStyle(document.documentElement).getPropertyValue(`--ds-${k}`).trim();
        return { out, raw };
      });
      // tokens equal the designer JSON of the skin's family
      const want = designs[meta.family][theme];
      const camel = { 'on-primary': 'onPrimary' };
      for (const [k, v] of Object.entries(c.raw)) {
        const key = camel[k] || k;
        total++;
        const norm = (x) => { x = x.toLowerCase(); return x.length === 4 ? '#' + [...x.slice(1)].map((ch) => ch + ch).join('') : x; };
        if (norm(v) !== norm(want[key])) { fails++; lines.push(`MISMATCH ${skin}/${theme} --ds-${k}: css ${v} vs json ${want[key]}`); }
      }
      const P = c.out;
      const pairs = [];
      for (const fg of ['text', 'text-2', 'text-3', 'link', 'primary', 'attn', 'success', 'warn', 'danger']) {
        for (const bg of ['bg', 'surface', 'surface-2', 'sunken']) pairs.push([fg, bg, 4.5]);
      }
      pairs.push(['on-primary', 'primary', 4.5], ['on-primary-soft', 'primary-soft', 4.5], ['on-attn-soft', 'attn-soft', 4.5],
        ['on-success-soft', 'success-soft', 4.5], ['on-warn-soft', 'warn-soft', 4.5], ['on-danger-soft', 'danger-soft', 4.5],
        ['text', 'primary-soft', 4.5], ['text-2', 'primary-soft', 4.5], ['border-strong', 'surface', 3], ['focus', 'surface', 3], ['focus', 'bg', 3]);
      for (const [fg, bg, min] of pairs) {
        const r = ratio(P[fg], P[bg]);
        total++;
        if (r < min) { fails++; lines.push(`LOW ${skin}/${theme} ${fg} on ${bg}: ${r.toFixed(2)}:1 (< ${min})`); }
      }
      // the skin's own accent pair as designed (independent of CSS)
      if (meta.accent) {
        const r = ratio(hex(meta.onAccent[theme]), hex(meta.accent[theme]));
        total++;
        if (r < 4.5) { fails++; lines.push(`LOW accent ${skin}/${theme}: ${r.toFixed(2)}:1`); }
        const usedOk = P.primary.join() === hex(meta.accent[theme]).join() && P['on-primary'].join() === hex(meta.onAccent[theme]).join();
        total++;
        if (!usedOk) { fails++; lines.push(`ACCENT NOT APPLIED ${skin}/${theme}: primary ${P.primary} vs ${hex(meta.accent[theme])}`); }
        lines.push(`  ${skin}/${theme}: accent ${ratio(hex(meta.onAccent[theme]), hex(meta.accent[theme])).toFixed(2)}:1; min text pair ${Math.min(...pairs.filter((p) => p[2] === 4.5).map(([a, b]) => ratio(P[a], P[b]))).toFixed(2)}:1`);
      }
      await pg.context().close();
    }
  }
  await browser.close();
  console.log(lines.join('\n'));
  console.log(`\ncontrast audit: ${total - fails}/${total} checks passed, ${fails} failed`);
  process.exitCode = fails ? 1 : 0;
})();
