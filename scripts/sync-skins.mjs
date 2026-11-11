#!/usr/bin/env node
/**
 * Skin asset sync + validation.
 *
 *   node scripts/sync-skins.mjs [--src DIR] [--dest DIR] [--tokens-css FILE] [--family-json FILE]
 *                               [--registry FILE]
 *
 * Reads designer skin folders (v2 layout, docs/briefs/astra-skins-v2-brief.zh-TW.md):
 *   <src>/<id>/{idle,listening,processing,approval,suggestion,paused,collapsed,avatar}.svg + skin.json
 * validates each one and copies ONLY the passing ones to <dest>/<id>/ (default src/assets/skins).
 * These SVGs are rendered inside the app, so a folder with any violation is rejected as a whole and
 * nothing of it is copied (a previously synced copy is left untouched).
 *
 * Also: if <src>/night-voyage-tokens.json exists, generates the [data-family="night-voyage"] token
 * block (src/styles/night-voyage.generated.css) and flags the family as available. Without the file the
 * block is empty and night-voyage skins stay "素材準備中".
 *
 * Folders in <src> that are neither registered in registry.ts nor contain a skin.json (e.g. app-icon) are skipped.
 *
 * Exit code 0: everything found passed. 1: at least one skin or the token file was rejected.
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/* ------------------------------ CLI ------------------------------ */
const argv = process.argv.slice(2);
const opt = (name) => {
  const i = argv.indexOf(`--${name}`);
  return i >= 0 ? argv[i + 1] : undefined;
};
const SRC = path.resolve(opt('src') ?? path.join(ROOT, 'docs/design/skins'));
const DEST = path.resolve(opt('dest') ?? path.join(ROOT, 'src/assets/skins'));
const TOKENS_CSS = path.resolve(opt('tokens-css') ?? path.join(ROOT, 'src/styles/night-voyage.generated.css'));
const FAMILY_JSON = path.resolve(opt('family-json') ?? path.join(ROOT, 'src/skins/generated/family-tokens.json'));
const REGISTRY = path.resolve(opt('registry') ?? path.join(ROOT, 'src/skins/registry.ts'));

/* ------------------------------ contract ------------------------------ */
const STATES = ['idle', 'listening', 'processing', 'approval', 'suggestion', 'paused'];
const REQUIRED_SVGS = [...STATES, 'collapsed', 'avatar'];
const FAMILIES = ['quiet-desk', 'night-voyage'];
const HEX = /^#[0-9a-fA-F]{6}$/;
const TOKEN_KEYS = [
  'canvas', 'surface', 'subtle', 'text', 'muted', 'primary', 'onPrimary',
  'secondary', 'success', 'warning', 'error', 'border', 'focus',
];
const MAX_SVG_BYTES = 256 * 1024;

/* ------------------------------ registry (read-only, regex) ------------------------------ */
function readRegistry() {
  try {
    const src = fs.readFileSync(REGISTRY, 'utf8');
    const out = new Map();
    for (const m of src.matchAll(/\{\s*id:\s*'([a-z0-9-]+)'\s*,\s*name:\s*'([^']+)'\s*,\s*family:\s*'([a-z-]+)'\s*\}/g)) {
      out.set(m[1], { name: m[2], family: m[3] });
    }
    return out;
  } catch {
    return new Map();
  }
}

/* ------------------------------ SVG safety ------------------------------ */
// Elements that can run code, load things, or embed other documents / rasters.
const FORBIDDEN_ELEMENTS = [
  'script', 'foreignobject', 'image', 'iframe', 'object', 'embed', 'link', 'audio', 'video',
  'canvas', 'meta', 'base', 'html', 'body', 'form', 'input', 'textarea', 'button',
];

/** Minimal tag tokenizer: enough to inspect elements/attributes without a dependency. */
function tokenize(svg) {
  const tags = [];
  const re = /<(\/?)([A-Za-z_][\w:.-]*)((?:"[^"]*"|'[^']*'|[^>"'])*?)(\/?)>/g;
  let m;
  while ((m = re.exec(svg))) {
    const attrs = [];
    const are = /([^\s=/"'<>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
    let a;
    while ((a = are.exec(m[3]))) attrs.push({ name: a[1], value: a[2] ?? a[3] ?? a[4] ?? '' });
    tags.push({ close: m[1] === '/', name: m[2], attrs, self: m[4] === '/' });
  }
  return tags;
}

function checkCssText(css, where, errors) {
  const c = css.replace(/\/\*[\s\S]*?\*\//g, '');
  if (/@import/i.test(c)) errors.push(`${where}: CSS @import (external stylesheet)`);
  if (/@font-face/i.test(c)) errors.push(`${where}: @font-face (external or embedded font)`);
  if (/expression\s*\(/i.test(c) || /javascript:/i.test(c) || /-moz-binding|behavior\s*:/i.test(c)) {
    errors.push(`${where}: scriptable CSS`);
  }
  for (const u of c.matchAll(/url\(\s*(['"]?)([^)'"]*)\1\s*\)/gi)) {
    if (!u[2].trim().startsWith('#')) errors.push(`${where}: url(${u[2].slice(0, 40)}) is not a local #reference`);
  }
}

function validateSvg(file, kind) {
  const errors = [];
  const warnings = [];
  const buf = fs.readFileSync(file);
  if (buf.length > MAX_SVG_BYTES) errors.push(`file is ${buf.length} bytes (limit ${MAX_SVG_BYTES})`);
  const text = buf.toString('utf8');
  if (text.includes('\u0000')) errors.push('contains NUL bytes');

  if (/<!DOCTYPE[^>]*\[/i.test(text) || /<!ENTITY/i.test(text)) errors.push('DOCTYPE with entity declarations');
  if (/<\?xml-stylesheet/i.test(text)) errors.push('xml-stylesheet processing instruction');
  if (/data:\s*image\//i.test(text) || /data:\s*application\//i.test(text)) {
    errors.push('embedded data: URI (raster or other document)');
  }

  const tags = tokenize(text);
  const root = tags.find((t) => !t.close);
  if (!root || root.name.toLowerCase() !== 'svg') errors.push('root element is not <svg>');
  const stack = [];
  let filters = 0;
  for (const t of tags) {
    const name = t.name.toLowerCase();
    const local = name.includes(':') ? name.split(':').pop() : name;
    if (!t.close && FORBIDDEN_ELEMENTS.includes(local)) errors.push(`forbidden element <${t.name}>`);
    if (local === 'filter' && !t.close) filters++;
    if (local === 'style' && !t.close) {
      // style element body: scan the text between this tag and its close
      const start = text.indexOf(`<${t.name}`);
      const end = text.indexOf(`</${t.name}`, start);
      if (end > start) checkCssText(text.slice(text.indexOf('>', start) + 1, end), '<style>', errors);
    }
    if (!t.close && !t.self) stack.push(name);
    else if (t.close) {
      const top = stack.pop();
      if (top !== name) errors.push(`malformed markup: </${t.name}> does not match <${top ?? 'nothing'}>`);
    }
    for (const a of t.attrs) {
      const an = a.name.toLowerCase();
      const v = a.value;
      if (/^on/i.test(an)) errors.push(`event-handler attribute ${a.name} on <${t.name}>`);
      if (an === 'href' || an.endsWith(':href') || an === 'src' || an === 'data' || an === 'action' || an === 'formaction') {
        if (!v.trim().startsWith('#')) errors.push(`${a.name}="${v.slice(0, 50)}" on <${t.name}> is not a local #reference`);
        else if (/&|\s/.test(v.trim())) errors.push(`${a.name} on <${t.name}> contains entity or whitespace tricks`);
      }
      if (/(?:^|[\s;])(?:https?:|ftp:|file:|javascript:|vbscript:)/i.test(v) && !['xmlns', 'xmlns:xlink'].includes(an)) {
        errors.push(`external or scriptable reference in ${a.name} on <${t.name}>`);
      }
      if (an === 'style') checkCssText(v, `style on <${t.name}>`, errors);
      if (['fill', 'stroke', 'filter', 'clip-path', 'mask', 'marker-start', 'marker-mid', 'marker-end'].includes(an)) {
        for (const u of v.matchAll(/url\(\s*(['"]?)([^)'"]*)\1\s*\)/gi)) {
          if (!u[2].trim().startsWith('#')) errors.push(`${a.name}=url(${u[2].slice(0, 40)}) on <${t.name}> is not local`);
        }
      }
      if (an === 'font-family' && /https?:|url\(/i.test(v)) errors.push('external font reference');
    }
  }
  if (stack.length) errors.push(`malformed markup: unclosed <${stack.at(-1)}>`);
  if (filters > 2) warnings.push(`${filters} <filter> elements (brief allows at most 2)`);

  // geometry contract
  if (root) {
    const vb = (root.attrs.find((a) => a.name === 'viewBox')?.value ?? '').trim().split(/[\s,]+/).map(Number);
    if (vb.length !== 4 || vb.some((n) => !Number.isFinite(n))) errors.push('missing or invalid viewBox');
    else if (STATES.includes(kind)) {
      if (vb[2] !== 96 || vb[3] !== 96) errors.push(`state canvas must be 96x96 (viewBox is ${vb.join(' ')})`);
    } else if (kind === 'collapsed') {
      if (Math.abs(vb[2] / vb[3] - 12 / 48) > 0.01) errors.push(`collapsed canvas must be 12x48 aspect (viewBox is ${vb.join(' ')})`);
    } else if (kind === 'avatar') {
      if (vb[2] !== vb[3]) errors.push(`avatar canvas must be square (viewBox is ${vb.join(' ')})`);
    }
  }
  return { errors, warnings };
}

/* ------------------------------ skin.json ------------------------------ */
function validateSkinJson(file, id, registry) {
  const errors = [];
  const warnings = [];
  let j;
  try {
    j = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (e) {
    return { errors: [`skin.json does not parse: ${e.message}`], warnings };
  }
  if (!j || typeof j !== 'object' || Array.isArray(j)) return { errors: ['skin.json is not an object'], warnings };
  if (j.id !== id) errors.push(`skin.json id "${j.id}" does not match folder "${id}"`);
  if (typeof j.name !== 'string' || !j.name.trim()) errors.push('skin.json name missing');
  if (!FAMILIES.includes(j.family)) errors.push(`skin.json family must be one of ${FAMILIES.join(', ')}`);
  if (j.preferredTheme !== 'light' && j.preferredTheme !== 'dark') errors.push('skin.json preferredTheme must be "light" or "dark"');
  for (const k of ['accent', 'onAccent']) {
    if (j[k] === undefined) continue;
    if (!j[k] || !HEX.test(j[k].light ?? '') || !HEX.test(j[k].dark ?? '')) {
      errors.push(`skin.json ${k} must be { light: "#rrggbb", dark: "#rrggbb" }`);
    }
  }
  if ((j.accent === undefined) !== (j.onAccent === undefined)) errors.push('skin.json accent and onAccent must come together');
  if (j.glow !== undefined && !HEX.test(j.glow)) errors.push('skin.json glow must be "#rrggbb"');
  const known = new Set(['id', 'name', 'family', 'preferredTheme', 'accent', 'onAccent', 'glow']);
  for (const k of Object.keys(j)) if (!known.has(k)) warnings.push(`skin.json has unknown field "${k}" (ignored)`);

  // contrast of accent pairs (brief: >= 4.5:1)
  if (!errors.length && j.accent) {
    for (const th of ['light', 'dark']) {
      const r = contrast(j.onAccent[th], j.accent[th]);
      if (r < 4.5) errors.push(`onAccent/${th} on accent/${th} contrast ${r.toFixed(2)}:1 is below 4.5:1`);
    }
  }
  const reg = registry.get(id);
  if (!reg) warnings.push(`"${id}" has no line in src/skins/registry.ts yet: add one or it will not be offered`);
  else {
    if (reg.family !== j.family) errors.push(`family "${j.family}" disagrees with registry line ("${reg.family}")`);
    if (reg.name !== j.name) warnings.push(`name "${j.name}" differs from registry name "${reg.name}" (registry wins)`);
  }
  return { errors, warnings };
}

function lum(hex) {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const l = c.map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
}
function contrast(a, b) {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/* ------------------------------ folder validation + copy ------------------------------ */
function validateSkinDir(dir, id, registry) {
  const errors = [];
  const warnings = [];
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(id)) errors.push('folder name is not a valid skin id');
  const files = new Set(fs.readdirSync(dir));
  for (const kind of REQUIRED_SVGS) {
    const f = `${kind}.svg`;
    if (!files.has(f)) {
      errors.push(`missing ${f}`);
      continue;
    }
    const st = fs.lstatSync(path.join(dir, f));
    if (!st.isFile()) {
      errors.push(`${f} is not a regular file`);
      continue;
    }
    const r = validateSvg(path.join(dir, f), kind);
    errors.push(...r.errors.map((e) => `${f}: ${e}`));
    warnings.push(...r.warnings.map((e) => `${f}: ${e}`));
  }
  if (!files.has('skin.json')) errors.push('missing skin.json');
  else {
    const r = validateSkinJson(path.join(dir, 'skin.json'), id, registry);
    errors.push(...r.errors);
    warnings.push(...r.warnings);
  }
  return { errors, warnings };
}

function copySkin(dir, id) {
  const tmp = path.join(DEST, `.${id}.tmp`);
  fs.rmSync(tmp, { recursive: true, force: true });
  fs.mkdirSync(tmp, { recursive: true });
  for (const f of [...REQUIRED_SVGS.map((k) => `${k}.svg`), 'skin.json']) {
    fs.copyFileSync(path.join(dir, f), path.join(tmp, f));
  }
  fs.rmSync(path.join(DEST, id), { recursive: true, force: true });
  fs.renameSync(tmp, path.join(DEST, id));
}

/* ------------------------------ night-voyage tokens ------------------------------ */
const kebab = (k) => k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);

function readFamilyTokens(file) {
  let j;
  try {
    j = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (e) {
    return { errors: [`night-voyage-tokens.json does not parse: ${e.message}`] };
  }
  const errors = [];
  for (const th of ['light', 'dark']) {
    const set = j?.themes?.[th];
    if (!set) {
      errors.push(`themes.${th} missing`);
      continue;
    }
    for (const k of TOKEN_KEYS) if (!HEX.test(set[k] ?? '')) errors.push(`themes.${th}.${k} must be "#rrggbb"`);
    if (!errors.length) {
      for (const fg of ['text', 'muted', 'primary', 'secondary', 'success', 'warning', 'error']) {
        for (const bg of ['canvas', 'surface', 'subtle']) {
          const r = contrast(set[fg], set[bg]);
          if (r < 4.5) errors.push(`themes.${th}: ${fg} on ${bg} is ${r.toFixed(2)}:1 (< 4.5:1)`);
        }
      }
      const r = contrast(set.onPrimary, set.primary);
      if (r < 4.5) errors.push(`themes.${th}: onPrimary on primary is ${r.toFixed(2)}:1 (< 4.5:1)`);
    }
  }
  return errors.length ? { errors } : { errors, tokens: j.themes };
}

function tokenBlock(selector, set, scheme) {
  const lines = TOKEN_KEYS.map((k) => `  --ds-${kebab(k)}: ${set[k].toLowerCase()};`);
  return `${selector} {\n  color-scheme: ${scheme};\n${lines.join('\n')}\n}`;
}

function writeFamilyOutputs(tokens) {
  const fam = `:root[data-family='night-voyage']`;
  const css = tokens
    ? [
        '/* GENERATED by scripts/sync-skins.mjs from docs/design/skins/night-voyage-tokens.json. Do not edit. */',
        'ZLIGHT',
        '@media (prefers-color-scheme: dark) {',
        '  ZDARKSYS',
        '}',
        'ZDARK',
        '',
      ]
        .join('\n')
        .replace('ZLIGHT', tokenBlock(fam, tokens.light, 'light'))
        .replace(
          'ZDARKSYS',
          tokenBlock(`${fam}:not([data-theme='light'])`, tokens.dark, 'dark').replace(/\n/g, '\n  '),
        )
        .replace('ZDARK', tokenBlock(`${fam}[data-theme='dark']`, tokens.dark, 'dark'))
    : '/* GENERATED by scripts/sync-skins.mjs from docs/design/skins/night-voyage-tokens.json. Do not edit.\n   The designer has not delivered the night-voyage palette yet, so there are no overrides here. */\n';
  fs.mkdirSync(path.dirname(TOKENS_CSS), { recursive: true });
  fs.mkdirSync(path.dirname(FAMILY_JSON), { recursive: true });
  fs.writeFileSync(TOKENS_CSS, css);
  fs.writeFileSync(FAMILY_JSON, JSON.stringify({ 'night-voyage': !!tokens }) + '\n');
}

/* ------------------------------ main ------------------------------ */
function main() {
  const registry = readRegistry();
  const results = [];
  fs.mkdirSync(DEST, { recursive: true });

  /** [{id, dir, origin}] */
  const candidates = [];
  if (fs.existsSync(SRC)) {
    for (const e of fs.readdirSync(SRC, { withFileTypes: true })) {
      if (e.isDirectory() && !e.name.startsWith('.')) {
        const dir = path.join(SRC, e.name);
        if (!registry.has(e.name) && !fs.existsSync(path.join(dir, 'skin.json'))) {
          console.log(`(skipped ${e.name}/: not registered and has no skin.json)`);
          continue;
        }
        candidates.push({ id: e.name, dir, origin: SRC });
      }
      // symlinked skin folders could point anywhere: refuse them
      else if (e.isSymbolicLink()) results.push({ id: e.name, ok: false, errors: ['symbolic link, refused'], warnings: [] });
    }
  } else {
    console.log(`(no skin source folder at ${SRC})`);
  }

  for (const c of candidates) {
    const v = validateSkinDir(c.dir, c.id, registry);
    const ok = v.errors.length === 0;
    if (ok) copySkin(c.dir, c.id);
    results.push({ id: c.id, ok, errors: v.errors, warnings: v.warnings, origin: c.origin });
  }

  // family palette
  let familyNote;
  let familyFailed = false;
  const nvFile = path.join(SRC, 'night-voyage-tokens.json');
  if (fs.existsSync(nvFile)) {
    const r = readFamilyTokens(nvFile);
    if (r.tokens) {
      writeFamilyOutputs(r.tokens);
      familyNote = `night-voyage palette: generated ${path.relative(ROOT, TOKENS_CSS)} (13 tokens x light/dark)`;
    } else {
      writeFamilyOutputs(null);
      familyFailed = true;
      familyNote = `night-voyage palette: REJECTED\n    - ${r.errors.join('\n    - ')}`;
    }
  } else {
    writeFamilyOutputs(null);
    familyNote = 'night-voyage palette: night-voyage-tokens.json not delivered; family stays unavailable (its skins show 素材準備中)';
  }

  // report
  console.log(`skin source: ${SRC}\nskin dest:   ${DEST}\n`);
  if (results.length === 0) console.log('no skin folders found');
  for (const r of results) {
    console.log(`${r.ok ? 'PASS  ' : 'REJECT'} ${r.id}${r.origin && r.origin !== SRC ? `   [from ${r.origin}]` : ''}`);
    if (r.ok) console.log(`       copied ${REQUIRED_SVGS.length} svg + skin.json -> ${path.relative(ROOT, path.join(DEST, r.id)) || DEST}`);
    for (const e of r.errors) console.log(`       x ${e}`);
    for (const w of r.warnings) console.log(`       ! ${w}`);
  }
  console.log(`\n${familyNote}`);
  const registered = [...registry.keys()];
  const synced = results.filter((r) => r.ok).map((r) => r.id);
  const waiting = registered.filter((id) => !fs.existsSync(path.join(DEST, id, 'skin.json')));
  if (waiting.length) console.log(`registered but no assets in dest (素材準備中): ${waiting.join(', ')}`);
  console.log(`\n${synced.length} synced, ${results.filter((r) => !r.ok).length} rejected`);

  process.exit(results.some((r) => !r.ok) || familyFailed ? 1 : 0);
}

main();
