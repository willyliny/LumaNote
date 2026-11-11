/**
 * Drives the mock demo scenario with Playwright and saves screenshots.
 * Usage: node tests-ui/capture.cjs [baseUrl]   (default http://localhost:4173)
 * Needs `vite preview` (or dev server) running. Playwright is pre-installed globally.
 */
const { chromium } = require('playwright');
const path = require('node:path');
const fs = require('node:fs');

const BASE = process.argv[2] || 'http://localhost:4173';

/** Window geometry comes from the single frontend source of truth (src/features/floating/layout.ts). */
const LAYOUT = (() => {
  const ts = fs.readFileSync(path.join(__dirname, '..', 'src', 'features', 'floating', 'layout.ts'), 'utf8');
  const n = (name) => Number(new RegExp(`^export const ${name} = (\\d+(?:\\.\\d+)?);`, 'm').exec(ts)[1]);
  return { SPRITE: n('FLOATING_SPRITE_PX'), COLLAPSED: n('FLOATING_COLLAPSED_PX'), EXPANDED: n('FLOATING_EXPANDED_PX'), RX: n('WHEEL_RX'), RY: n('WHEEL_RY'), ITEM_W: n('WHEEL_ITEM_W'), ITEM_H: n('WHEEL_ITEM_H'), GAP: n('WHEEL_MIN_GAP') };
})();
const { COLLAPSED: CW, EXPANDED: EW, SPRITE: SP } = LAYOUT;
const CC = CW / 2; // centre of the collapsed window
const EC = EW / 2; // centre of the expanded window
const OUT = path.join(__dirname, 'screenshots');
fs.mkdirSync(OUT, { recursive: true });

const problems = [];
const shot = async (page, name, opts = {}) => {
  await page.screenshot({ path: path.join(OUT, `${name}.png`), ...opts });
  console.log('saved', name);
};
function watch(page, label) {
  page.on('pageerror', (e) => problems.push(`[${label}] pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') problems.push(`[${label}] console.error: ${m.text()}`);
  });
}

async function runTheme(browser, theme) {
  const ctx = await browser.newContext({
    deviceScaleFactor: 2,
    timezoneId: 'Asia/Taipei',
    locale: 'zh-TW',
    reducedMotion: 'reduce', // freeze sprite motion so shots are deterministic
    viewport: { width: 420, height: 720 },
    permissions: ['clipboard-read', 'clipboard-write'],
  });
  const q = `theme=${theme}&mockSpeed=3`;

  /* ---------- floating window opened FIRST, to prove cross-window sync ---------- */
  const float = await ctx.newPage();
  watch(float, `float-live-${theme}`);
  await float.setViewportSize({ width: 480, height: 110 });
  await float.goto(`${BASE}/floating.html?theme=${theme}&backdrop=1`);
  await float.waitForSelector('[data-testid="float-expanded"]');
  await shot(float, `floating-live-idle-${theme}`);

  /* ---------- main panel: demo scenario ---------- */
  const page = await ctx.newPage();
  watch(page, `panel-${theme}`);
  await page.goto(`${BASE}/panel.html?${q}`);
  await page.waitForSelector('text=有什麼可以交給我');
  await shot(page, `panel-chat-empty-${theme}`);

  await page.getByRole('button', { name: '用範例會議筆記試試' }).click();
  await shot(page, `panel-chat-composed-${theme}`);
  await page.getByRole('button', { name: '送出' }).click();

  await page.waitForSelector('.md--streaming');
  await page.waitForFunction(() => document.body.innerText.includes('待辦'));
  await shot(page, `panel-chat-streaming-${theme}`);

  await page.waitForSelector('text=需要你核准', { timeout: 30000 });
  await page.waitForTimeout(400);
  await shot(page, `panel-chat-approval-${theme}`);
  const card = page.locator('.approval').first();
  await card.scrollIntoViewIfNeeded();
  await card.screenshot({ path: path.join(OUT, `approval-card-${theme}.png`) });
  console.log('saved approval-card', theme);

  // the other window should now show the "awaiting confirmation" state from live events
  await float.waitForSelector('[data-state="awaiting"]', { timeout: 5000 });
  await float.waitForTimeout(500);
  await shot(float, `floating-live-awaiting-${theme}`);

  // tasks tab while the task awaits the user
  await page.getByRole('tab', { name: /任務/ }).click();
  await page.waitForSelector('.task');
  await shot(page, `panel-tasks-${theme}`);
  await page.setViewportSize({ width: 420, height: 1900 });
  await shot(page, `panel-tasks-full-${theme}`);
  await page.setViewportSize({ width: 420, height: 720 });

  // approve once, then wait for the authoritative completed state
  await page.getByRole('tab', { name: /對話/ }).click();
  await page.getByRole('button', { name: '核准一次' }).click();
  await page.waitForSelector('.strip .badge:has-text("已完成")', { timeout: 30000 });
  await page.waitForTimeout(300);
  await shot(page, `panel-chat-completed-${theme}`);

  // chat scrolled to the top (summary + sources)
  await page.evaluate(() => (document.querySelector('.chat__scroll').scrollTop = 0));
  await page.locator('.sources summary').first().click();
  await page.waitForTimeout(200);
  await shot(page, `panel-chat-sources-${theme}`);

  // today tab: todos, agenda, reminder created by the task, and the later proactive suggestion
  await page.getByRole('tab', { name: /今日/ }).click();
  await page.waitForSelector('.suggest', { timeout: 30000 });
  await shot(page, `panel-today-${theme}`);
  await page.setViewportSize({ width: 420, height: 1500 });
  await shot(page, `panel-today-full-${theme}`);
  await page.setViewportSize({ width: 420, height: 720 });

  await float.waitForSelector('[data-state="suggestion"]', { timeout: 5000 });
  await float.waitForTimeout(500);
  await shot(float, `floating-live-suggestion-${theme}`);

  await page.getByRole('tab', { name: /任務/ }).click();
  await page.setViewportSize({ width: 420, height: 1900 });
  await shot(page, `panel-tasks-after-${theme}`);
  await page.setViewportSize({ width: 420, height: 720 });

  /* ---------- floating entry: six states, expanded + docked ---------- */
  const states = ['idle', 'listening', 'working', 'awaiting', 'suggestion', 'dnd'];
  const f2 = await ctx.newPage();
  watch(f2, `float-${theme}`);
  await f2.setViewportSize({ width: 480, height: 110 });
  for (const s of states) {
    await f2.goto(`${BASE}/floating.html?theme=${theme}&backdrop=1&state=${s}&dock=none`);
    await f2.waitForSelector('[data-testid="float-expanded"]');
    await f2.waitForTimeout(450);
    await shot(f2, `floating-${s}-${theme}`);
  }
  await f2.setViewportSize({ width: CW, height: CW });
  for (const s of states) {
    await f2.goto(`${BASE}/floating.html?theme=${theme}&backdrop=1&state=${s}&dock=left`);
    await f2.waitForSelector('[data-testid="float-collapsed"]');
    await shot(f2, `floating-docked-left-${s}-${theme}`);
  }
  for (const s of ['idle', 'awaiting']) {
    await f2.goto(`${BASE}/floating.html?theme=${theme}&backdrop=1&state=${s}&dock=right`);
    await f2.waitForSelector('[data-testid="float-collapsed"]');
    await shot(f2, `floating-docked-right-${s}-${theme}`);
  }
  await f2.close();
  await float.close();

  /* ---------- settings & memory ---------- */
  const sp = await ctx.newPage();
  watch(sp, `settings-${theme}`);
  await sp.setViewportSize({ width: 760, height: 900 });
  await sp.goto(`${BASE}/settings.html?theme=${theme}&mockSpeed=3`);
  await sp.waitForSelector('.profile');
  await shot(sp, `settings-models-${theme}`);
  await sp.setViewportSize({ width: 760, height: 1500 });
  await sp.getByRole('button', { name: '測試連線' }).first().click();
  await sp.waitForSelector('.testresult', { timeout: 15000 });
  await shot(sp, `settings-models-tested-${theme}`);
  await sp.getByRole('button', { name: '編輯' }).first().click();
  await sp.waitForSelector('.pform');
  await shot(sp, `settings-models-edit-${theme}`);
  await sp.setViewportSize({ width: 760, height: 900 });

  await sp.getByRole('tab', { name: '觀察與隱私' }).click();
  await sp.waitForSelector('text=排除清單');
  await sp.setViewportSize({ width: 760, height: 1500 });
  await shot(sp, `settings-observation-${theme}`);

  await sp.getByRole('tab', { name: '記憶' }).click();
  await sp.waitForSelector('.memory');
  await sp.getByRole('button', { name: '修正' }).last().click();
  await shot(sp, `settings-memory-${theme}`);

  await sp.getByRole('tab', { name: '外觀' }).click();
  await sp.waitForSelector('.skin');
  await sp.setViewportSize({ width: 760, height: 1500 });
  await shot(sp, `settings-appearance-${theme}`);
  await ctx.close();
}

async function checkResync(browser) {
  // Prove the UI heals from the backend when events are lost (never infers completion from events).
  const ctx = await browser.newContext({ timezoneId: 'Asia/Taipei', locale: 'zh-TW', viewport: { width: 420, height: 720 } });
  const page = await ctx.newPage();
  watch(page, 'resync');
  await page.goto(`${BASE}/panel.html?theme=light&mockSpeed=6`);
  await page.waitForSelector('text=有什麼可以交給我');
  await page.evaluate(() => window.__elfMock.dropNextEvents(40)); // swallow a burst, incl. terminal events
  await page.getByRole('button', { name: '用範例會議筆記試試' }).click();
  await page.getByRole('button', { name: '送出' }).click();
  await page.waitForSelector('text=需要你核准', { timeout: 30000 });
  const ok1 = await page.locator('.approval').count();
  await page.getByRole('button', { name: '核准一次' }).click();
  await page.evaluate(() => window.__elfMock.dropNextEvents(1000)); // drop everything, incl. `completed`
  await page.waitForTimeout(6000);
  const stillBefore = await page.locator('.strip .badge:has-text("已完成")').count();
  await page.evaluate(() => window.__elfMock.dropNextEvents(0));
  // reconnect trigger: window becomes visible again
  await page.evaluate(() => {
    window.dispatchEvent(new Event('online'));
  });
  await page.waitForTimeout(1500);
  const after = await page.locator('.strip .badge:has-text("已完成")').count();
  console.log(`RESYNC approval-visible-after-drops=${ok1} completed-before-resync=${stillBefore} completed-after-resync=${after}`);
  await ctx.close();
}


/* =====================================================================================
 * Skins, wheel, transparency, appearance flow
 * ===================================================================================== */
const os = require('node:os');
const SKIN_DIR = path.join(__dirname, '..', 'src', 'assets', 'skins');
const SKIN_IDS = fs.readdirSync(SKIN_DIR).filter((d) => fs.existsSync(path.join(SKIN_DIR, d, 'skin.json'))).sort();
const STATES = ['idle', 'listening', 'working', 'awaiting', 'suggestion', 'dnd'];
const SKIN_FILE = { idle: 'idle', listening: 'listening', working: 'processing', awaiting: 'approval', suggestion: 'suggestion', dnd: 'paused' };
const SK_OUT = path.join(OUT, 'skins');
fs.mkdirSync(SK_OUT, { recursive: true });
const results = [];
const check = (name, ok, detail = '') => {
  results.push({ name, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ' - ' + detail : ''}`);
};

/** Body (outline + body groups) must be byte-identical across the six states of a skin. */
function bodyIdentity() {
  for (const id of SKIN_IDS) {
    const bodies = {};
    for (const [app, file] of Object.entries(SKIN_FILE)) {
      const svg = fs.readFileSync(path.join(SKIN_DIR, id, `${file}.svg`), 'utf8');
      const a = svg.indexOf('<g id="body-outline"');
      const b = svg.indexOf('<g id="state-outline"');
      bodies[app] = a >= 0 && b > a ? svg.slice(a, b) : null;
    }
    const base = bodies.idle;
    const differs = STATES.filter((st) => bodies[st] !== base);
    // a drawn mark (eyes closing) is allowed to differ only inside <g id="eyes">; report separately
    const stripEyes = (x) => (x ? x.replace(/<g id="eyes"[\s\S]*?<\/g>/, '') : x);
    const bodyOnly = STATES.filter((st) => stripEyes(bodies[st]) !== stripEyes(base));
    check(
      `body identical across states: ${id}`,
      differs.length === 0,
      differs.length ? `differs from idle: ${differs.join(',')}; outside <g id="eyes">: ${bodyOnly.join(',') || 'none'}` : '',
    );
  }
}

async function contactSheet(browser, files, cols, label, bg, cell) {
  const html = path.join(os.tmpdir(), `elf-sheet-${label}.html`);
  const rows = files
    .map((f) => `<div class="c"><img src="file://${f.src}" width="${cell}" height="${cell}"><span>${f.caption}</span></div>`)
    .join('');
  fs.writeFileSync(
    html,
    `<meta charset=utf-8><style>body{margin:0;background:${bg};font:12px sans-serif;color:${bg === '#1c2630' ? '#e6edf3' : '#1b252d'}}
    .g{display:grid;grid-template-columns:repeat(${cols},${cell + 24}px);gap:6px;padding:10px}.c{display:grid;justify-items:center}</style><div class=g>${rows}</div>`,
  );
  const ctx = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: cols * (cell + 30) + 24, height: 200 } });
  const pg = await ctx.newPage();
  await pg.goto('file://' + html);
  await pg.waitForTimeout(200);
  await pg.screenshot({ path: path.join(OUT, `${label}.png`), fullPage: true });
  console.log('saved', label);
  await ctx.close();
}

async function runSkins(browser) {
  bodyIdentity();
  for (const bg of ['light', 'dark']) {
    const ctx = await browser.newContext({ deviceScaleFactor: 2, reducedMotion: 'reduce', viewport: { width: CW, height: CW } });
    const pg = await ctx.newPage();
    watch(pg, `skins-${bg}`);
    const files = [];
    for (const id of SKIN_IDS) {
      for (const st of STATES) {
        await pg.goto(`${BASE}/floating.html?theme=${bg}&backdrop=${bg}&skin=${id}&state=${st}&dock=none`);
        await pg.waitForSelector('[data-testid="float-expanded"] img.sprite');
        await pg.waitForFunction(() => [...document.images].every((i) => i.complete && i.naturalWidth > 0));
        const attr = await pg.evaluate(() => document.documentElement.getAttribute('data-skin'));
        if (attr !== id) check(`forced skin applied ${id}`, false, `got ${attr}`);
        const file = path.join(SK_OUT, `${id}-${st}-${bg}.png`);
        await pg.screenshot({ path: file });
        files.push({ src: file, caption: st === 'idle' ? id : st });
      }
    }
    await ctx.close();
    await contactSheet(browser, files, 6, `skins-sheet-${bg}`, bg === 'light' ? '#d6dde3' : '#1c2630', CW);
  }

  // docked (collapsed) form for every skin, left and right, idle + awaiting
  for (const bg of ['light', 'dark']) {
    const ctx = await browser.newContext({ deviceScaleFactor: 4, reducedMotion: 'reduce', viewport: { width: CW, height: CW } });
    const pg = await ctx.newPage();
    const files = [];
    for (const id of SKIN_IDS) {
      for (const [dock, st] of [['left', 'idle'], ['left', 'awaiting'], ['right', 'idle']]) {
        await pg.goto(`${BASE}/floating.html?theme=${bg}&backdrop=${bg}&skin=${id}&state=${st}&dock=${dock}`);
        await pg.waitForSelector('[data-testid="float-collapsed"] img');
        await pg.waitForFunction(() => [...document.images].every((i) => i.complete && i.naturalWidth > 0));
        const file = path.join(SK_OUT, `${id}-docked-${dock}-${st}-${bg}.png`);
        await pg.screenshot({ path: file, clip: { x: dock === 'left' ? 0 : CW - 48, y: CC - 24, width: 48, height: 48 } });
        files.push({ src: file, caption: `${id.split('-')[0]} ${dock[0]}/${st[0]}` });
      }
    }
    await ctx.close();
    await contactSheet(browser, files, 6, `skins-docked-sheet-${bg}`, bg === 'light' ? '#d6dde3' : '#1c2630', CW);
  }

  // avatar legibility at 24 / 32 / 40 px on panel surfaces (quiet-desk and night-voyage, light and dark)
  const html = path.join(os.tmpdir(), 'elf-avatars.html');
  const surfaces = [
    ['quiet light', '#ffffff', '#202b25'],
    ['quiet dark', '#202a23', '#f1f4ed'],
    ['night light', '#ffffff', '#17283b'],
    ['night dark', '#131f30', '#f0f6ff'],
  ];
  const cells = SKIN_IDS.map(
    (id) =>
      `<div class=r><b>${id}</b>` +
      surfaces
        .map(
          ([n, bgc, fg]) =>
            `<span class=s style="background:${bgc};color:${fg}">${[24, 32, 40]
              .map((z) => `<img src="file://${SKIN_DIR}/${id}/avatar.svg" width=${z} height=${z}>`)
              .join('')}<i>${n}</i></span>`,
        )
        .join('') +
      `</div>`,
  ).join('');
  fs.writeFileSync(
    html,
    `<meta charset=utf-8><style>body{margin:0;padding:10px;background:#888;font:11px sans-serif}.r{display:flex;gap:8px;align-items:center;margin-bottom:6px}b{width:110px;color:#fff}.s{display:flex;gap:8px;align-items:center;padding:6px 10px;border-radius:8px}i{font-style:normal}</style>${cells}`,
  );
  const ctx = await browser.newContext({ deviceScaleFactor: 3, viewport: { width: 860, height: 300 } });
  const pg = await ctx.newPage();
  await pg.goto('file://' + html);
  await pg.waitForTimeout(300);
  await pg.screenshot({ path: path.join(OUT, 'skins-avatars.png'), fullPage: true });
  console.log('saved skins-avatars');
  await ctx.close();
}

/** Panel in each family with a night-voyage skin applied, plus the quiet-desk default. */
async function runFamilies(browser) {
  for (const theme of ['light', 'dark']) {
    for (const skin of ['leaf-cloak', 'night-letter']) {
      const ctx = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: 440, height: 720 } });
      const pg = await ctx.newPage();
      watch(pg, `family-${skin}-${theme}`);
      await pg.goto(`${BASE}/panel.html?theme=${theme}&skin=${skin}&mockSpeed=3`);
      await pg.waitForSelector('text=有什麼可以交給我');
      await pg.waitForFunction(() => [...document.images].every((i) => i.complete && i.naturalWidth > 0));
      const fam = await pg.evaluate(() => document.documentElement.getAttribute('data-family'));
      const prim = await pg.evaluate(() => {
        const d = document.createElement('div');
        d.style.background = 'var(--c-primary)';
        document.body.appendChild(d);
        const c = getComputedStyle(d).backgroundColor;
        d.remove();
        return c;
      });
      console.log(`family ${skin}/${theme}: data-family=${fam} --c-primary=${prim}`);
      await pg.getByRole('button', { name: '用範例會議筆記試試' }).click();
      await shot(pg, `panel-family-${skin}-${theme}`);
      await ctx.close();
    }
  }
}

/** Real hover interactions on the floating entry (viewport = expanded window). */
async function runWheel(browser) {
  for (const [theme, bg] of [['light', 'light'], ['dark', 'dark'], ['light', 'dark'], ['dark', 'light']]) {
    const ctx = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: EW, height: EW } });
    const pg = await ctx.newPage();
    watch(pg, `wheel-${theme}-${bg}`);
    await pg.goto(`${BASE}/floating.html?theme=${theme}&backdrop=${bg}&dock=none`);
    await pg.waitForSelector('[data-testid="float-expanded"]');
    await pg.waitForFunction(() => document.documentElement.getAttribute('data-skin'));
    await pg.waitForTimeout(500); // store sync
    const log = () => pg.evaluate(() => window.__elfMock.expandedLog);

    // passing over the sprite (< 250 ms) must not open
    await pg.mouse.move(10, 10);
    await pg.mouse.move(EC, EC);
    await pg.waitForTimeout(100);
    await pg.mouse.move(10, 10);
    await pg.waitForTimeout(500);
    check(`pass-over does not open (${theme}/${bg})`, (await log()).length === 0 && (await pg.locator('.wheel').count()) === 0);

    // rest on the sprite
    await pg.mouse.move(EC, EC);
    await pg.waitForSelector('.wheel', { state: 'attached', timeout: 2000 });
    const l1 = await log();
    check(`set_floating_expanded(true) called before wheel is in DOM (${theme}/${bg})`, l1.length === 1 && l1[0].expanded === true && l1[0].wheelInDom === false, JSON.stringify(l1));
    await pg.waitForTimeout(150);
    await shot(pg, `wheel-open-${theme}-on-${bg}`);

    if (theme === bg) {
      const box = await pg.locator('.wheel__item[data-item="tasks"]').boundingBox();
      const sprite = await pg.locator('.float__sprite').boundingBox();
      const items = await pg.locator('.wheel__item').evaluateAll((els) => els.map((e) => ({ id: e.dataset.item, ...e.getBoundingClientRect().toJSON(), text: e.innerText.replace(/\n/g, ' / ') })));
      console.log('wheel values:', items.map((i) => `${i.id}=[${i.text}]`).join(' '));
      check('every wheel item >= 44px hit area', items.every((i) => i.width >= 44 && i.height >= 44));
      const rects = items.map((i) => i);
      let overlap = false;
      for (let a = 0; a < rects.length; a++) for (let b = a + 1; b < rects.length; b++) {
        const A = rects[a], B = rects[b];
        if (A.left < B.right && B.left < A.right && A.top < B.bottom && B.top < A.bottom) overlap = true;
      }
      check('wheel items do not overlap each other', !overlap);
      check(`all wheel items inside the ${EW}x${EW} window`, items.every((i) => i.left >= 0 && i.top >= 0 && i.right <= EW && i.bottom <= EW));
      check(`sprite centred at (${EC},${EC}) while wheel open`, Math.abs(sprite.x + sprite.width / 2 - EC) < 0.5 && Math.abs(sprite.y + sprite.height / 2 - EC) < 0.5, JSON.stringify(sprite));
      const focused = await pg.evaluate(() => document.activeElement && document.activeElement.tagName);
      check('wheel did not take focus', focused === 'BODY', String(focused));
      // hover + pressed segment
      await pg.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await pg.waitForTimeout(150);
      await shot(pg, `wheel-hover-segment-${theme}-on-${bg}`);
      await pg.mouse.down();
      await pg.waitForTimeout(100);
      await shot(pg, `wheel-active-segment-${theme}-on-${bg}`);
      await pg.mouse.up(); // click: opens panel (window.open is blocked/ignored in headless) and closes the wheel
      await pg.waitForTimeout(400);
      check('choosing an item closes the wheel', (await pg.locator('.wheel').count()) === 0);
      await pg.waitForTimeout(100);
      const l2 = await log();
      check('set_floating_expanded(false) after close', l2.at(-1).expanded === false, JSON.stringify(l2));

      // leave => closes ~300ms later
      await pg.mouse.move(EC, EC);
      await pg.waitForSelector('.wheel', { state: 'attached', timeout: 3000 }).catch(async (e) => { console.log('DEBUG phase', await pg.locator('[data-testid=float-expanded]').getAttribute('data-wheel'), JSON.stringify(await log())); throw e; });
      await pg.mouse.move(2, 2);
      await pg.waitForTimeout(150);
      check('wheel still open 150 ms after leaving', (await pg.locator('.wheel').count()) === 1);
      await pg.waitForTimeout(600);
      check('wheel closed after leaving', (await pg.locator('.wheel').count()) === 0);

      // Escape closes immediately
      await pg.mouse.move(EC, EC);
      await pg.waitForSelector('.wheel', { state: 'attached' });
      await pg.keyboard.press('Escape');
      await pg.waitForTimeout(250);
      check('Escape closes the wheel', (await pg.locator('.wheel').count()) === 0);
      await pg.mouse.move(2, 2);
      await pg.waitForTimeout(400);

      // drag never opens it
      await pg.mouse.move(EC, EC);
      await pg.mouse.down();
      await pg.mouse.move(EC + 10, EC + 10, { steps: 4 });
      await pg.waitForTimeout(600);
      check('pressing/dragging the sprite never opens the wheel', (await pg.locator('.wheel').count()) === 0);
      await pg.mouse.up();
      await pg.mouse.move(2, 2);
      await pg.waitForTimeout(300);

      // DnD toggles in place
      await pg.mouse.move(EC, EC);
      await pg.waitForSelector('.wheel', { state: 'attached' });
      const dnd = pg.locator('.wheel__item[data-item="dnd"]');
      const before = await dnd.getAttribute('aria-pressed');
      const b = await dnd.boundingBox();
      await pg.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
      await pg.mouse.click(b.x + b.width / 2, b.y + b.height / 2);
      await pg.waitForTimeout(500);
      await pg.mouse.move(EC, EC);
      await pg.waitForSelector('.wheel', { state: 'attached', timeout: 3000 }).catch(async (e) => { console.log('DEBUG2 phase', await pg.locator('[data-testid=float-expanded]').getAttribute('data-wheel'), JSON.stringify(await log())); throw e; });
      const after = await pg.locator('.wheel__item[data-item="dnd"]').getAttribute('aria-pressed');
      check('勿擾 toggles in place', before !== after, `${before} -> ${after}`);
      await pg.waitForTimeout(200);
      await shot(pg, `wheel-dnd-on-${theme}-on-${bg}`);
    }
    await ctx.close();
  }

  // collapsed window: sprite centred; docked hover undocks first
  const ctx = await browser.newContext({ deviceScaleFactor: 2, reducedMotion: 'reduce', viewport: { width: CW, height: CW } });
  const pg = await ctx.newPage();
  watch(pg, 'wheel-dock');
  await pg.goto(`${BASE}/floating.html?theme=light&backdrop=light&dock=none`);
  await pg.waitForSelector('[data-testid="float-expanded"]');
  const sb = await pg.locator('.float__sprite').boundingBox();
  check(`sprite centred in the collapsed ${CW}x${CW} window`, Math.abs(sb.x + sb.width / 2 - CC) < 0.5 && Math.abs(sb.y + sb.height / 2 - CC) < 0.5 && sb.width === SP, JSON.stringify(sb));
  await pg.goto(`${BASE}/floating.html?theme=light&backdrop=light&dock=left`);
  await pg.waitForSelector('[data-testid="float-collapsed"]');
  const db = await pg.locator('.dock').boundingBox();
  const strip = await pg.locator('.dock__strip').boundingBox();
  check('docked: visible strip 12x48, hit area 44 wide', strip.width === 12 && strip.height === 48 && db.width === 44, `strip ${strip.width}x${strip.height}, hit ${db.width}x${db.height}`);
  await pg.mouse.move(6, CC);
  await pg.waitForSelector('[data-testid="float-expanded"]', { timeout: 2000 });
  check('hovering a docked sprite undocks it (wheel needs a second rest)', (await pg.locator('.wheel').count()) === 0);
  await ctx.close();

  // live values: seeded mock with an approval waiting (second window drives the demo)
  const lctx = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: EW, height: EW } });
  const f = await lctx.newPage();
  watch(f, 'wheel-live');
  await f.goto(`${BASE}/floating.html?theme=light&backdrop=light&dock=none`);
  const panel = await lctx.newPage();
  await panel.setViewportSize({ width: 420, height: 720 });
  await panel.goto(`${BASE}/panel.html?theme=light&mockSpeed=3`);
  await panel.waitForSelector('text=有什麼可以交給我');
  await panel.getByRole('button', { name: '用範例會議筆記試試' }).click();
  await panel.getByRole('button', { name: '送出' }).click();
  await panel.waitForSelector('text=需要你核准', { timeout: 30000 });
  await f.bringToFront();
  await f.waitForSelector('[data-state="awaiting"]', { timeout: 5000 });
  await f.mouse.move(EC, EC);
  await f.waitForSelector('.wheel', { state: 'attached' });
  await f.waitForTimeout(200);
  console.log('wheel live values:', await f.locator('.wheel__item').evaluateAll((els) => els.map((e) => `${e.dataset.item}=[${e.innerText.replace(/\n/g, ' / ')}]`).join(' ')));
  await shot(f, 'wheel-open-awaiting-light');
  await lctx.close();
}

/** Prove nothing opaque is painted outside the sprite art: loud checkerboard behind an iframe. */
async function runTransparency(browser) {
  for (const theme of ['light', 'dark']) {
    for (const wheel of [false, true]) {
      const size = wheel ? EW : CW;
      const ctx = await browser.newContext({ deviceScaleFactor: 1, reducedMotion: 'reduce', viewport: { width: size + 40, height: size + 40 } });
      const pg = await ctx.newPage();
      watch(pg, `transparency-${theme}`);
      const board = `background-color:#ff00ff;background-image:linear-gradient(45deg,#00ff00 25%,transparent 25%,transparent 75%,#00ff00 75%),linear-gradient(45deg,#00ff00 25%,transparent 25%,transparent 75%,#00ff00 75%);background-size:20px 20px;background-position:0 0,10px 10px`;
      const frame = (on) =>
        `<body style="margin:0;${board}"><div style="position:absolute;left:20px;top:20px;width:${size}px;height:${size}px">` +
        (on ? `<iframe id=f src="${BASE}/floating.html?theme=${theme}&dock=none" style="border:0;width:${size}px;height:${size}px;background:transparent" allowtransparency></iframe>` : '') +
        `</div></body>`;
      await pg.setContent(frame(false));
      const bare = await pg.screenshot();
      await pg.setContent(frame(true));
      const fr = pg.frameLocator('#f');
      await fr.locator('[data-testid="float-expanded"]').waitFor();
      await pg.waitForTimeout(500);
      if (wheel) {
        await pg.mouse.move(20 + size / 2, 20 + size / 2);
        await fr.locator('.wheel').waitFor({ state: 'attached' });
        await pg.waitForTimeout(250);
      }
      const withFrame = await pg.screenshot();
      fs.writeFileSync(path.join(OUT, `transparency-${wheel ? 'wheel-' : ''}${theme}.png`), withFrame);
      console.log('saved transparency', theme, wheel ? 'wheel' : '');
      // pixels outside every painted element must equal the bare checkerboard
      const verdict = await pg.evaluate(
        async ({ a, b, size, wheel, L }) => {
          const load = (d) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = 'data:image/png;base64,' + d; });
          const [A, B] = await Promise.all([load(a), load(b)]);
          const px = (img) => { const c = document.createElement('canvas'); c.width = img.width; c.height = img.height; const x = c.getContext('2d'); x.drawImage(img, 0, 0); return x.getImageData(0, 0, img.width, img.height); };
          const pa = px(A), pb = px(B);
          // allowed (painted) regions in page coords: sprite 64x64 centred; for the wheel, the item cards
          const cx = 20 + size / 2, cy = 20 + size / 2;
          const rects = [[cx - L.SPRITE / 2 - 12, cy - L.SPRITE / 2 - 12, L.SPRITE + 24, L.SPRITE + 24]]; // sprite + the flat art's drop-shadow/halo filter
          if (wheel) for (let i = 0; i < 6; i++) {
            const ang = ((-90 + 60 * i) * Math.PI) / 180;
            const x = Math.round(L.RX * Math.cos(ang)), y = Math.round(L.RY * Math.sin(ang));
            rects.push([cx + x - L.ITEM_W / 2 - 32, cy + y - L.ITEM_H / 2 - 32, L.ITEM_W + 64, L.ITEM_H + 64]); // card + its 24px-blur drop shadow
          }
          let diff = 0, total = 0;
          for (let y = 0; y < pa.height; y++) for (let x = 0; x < pa.width; x++) {
            if (rects.some(([rx, ry, rw, rh]) => x >= rx && x < rx + rw && y >= ry && y < ry + rh)) continue;
            total++;
            const i = (y * pa.width + x) * 4;
            if (Math.abs(pa.data[i] - pb.data[i]) + Math.abs(pa.data[i+1] - pb.data[i+1]) + Math.abs(pa.data[i+2] - pb.data[i+2]) > 6) diff++;
          }
          return { diff, total };
        },
        { a: bare.toString('base64'), b: withFrame.toString('base64'), size, wheel, L: LAYOUT },
      );
      check(`transparent outside art (${theme}${wheel ? ', wheel open' : ''})`, verdict.diff === 0, `${verdict.diff} of ${verdict.total} outside pixels differ`);
      await ctx.close();
    }
  }
  // computed backgrounds before/after mount
  const ctx = await browser.newContext({ viewport: { width: CW, height: CW } });
  const pg = await ctx.newPage();
  for (const theme of ['light', 'dark']) {
    await pg.goto(`${BASE}/floating.html?theme=${theme}&dock=none`);
    await pg.waitForSelector('[data-testid="float-expanded"]');
    const bgs = await pg.evaluate(() => ['html', 'body', '#root', '.float'].map((q) => `${q}:${getComputedStyle(document.querySelector(q)).backgroundColor}|${getComputedStyle(document.querySelector(q)).boxShadow}`));
    check(`html/body/#root/.float background transparent (${theme})`, bgs.every((x) => x.startsWith(`${x.split(':')[0]}:rgba(0, 0, 0, 0)|none`)), bgs.join(' '));
  }
  await ctx.close();
}

/** Settings flow: preview, apply, cross-window live update, restore, fallback. */
async function runAppearanceFlow(browser) {
  const ctx = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: 760, height: 1500 } });
  const float = await ctx.newPage();
  watch(float, 'flow-float');
  await float.setViewportSize({ width: CW, height: CW });
  await float.goto(`${BASE}/floating.html?backdrop=light&theme=light&dock=none`);
  const panel = await ctx.newPage();
  watch(panel, 'flow-panel');
  await panel.setViewportSize({ width: 440, height: 720 });
  await panel.goto(`${BASE}/panel.html?theme=light`);
  const sp = await ctx.newPage();
  watch(sp, 'flow-settings');
  await sp.goto(`${BASE}/settings.html?theme=light&tab=appearance`);
  await sp.waitForSelector('.skin');
  const cards = await sp.locator('.skin').evaluateAll((els) => els.map((e) => ({ id: e.dataset.skinId, disabled: e.getAttribute('aria-disabled'), text: e.innerText.replace(/\n/g, ' ') })));
  check('six skin cards, all selectable (assets complete)', cards.length === 6 && cards.every((c) => c.disabled === 'false'), JSON.stringify(cards.map((c) => c.id + ':' + c.disabled)));
  check('default skin is leaf-cloak and marked 目前使用', /目前使用/.test(cards.find((c) => c.id === 'leaf-cloak').text));
  await shot(sp, 'settings-skins-all-six-light');
  const skinOf = (pg) => pg.evaluate(() => document.documentElement.getAttribute('data-skin') + '/' + document.documentElement.getAttribute('data-family'));
  check('floating starts with leaf-cloak', (await skinOf(float)).startsWith('leaf-cloak'), await skinOf(float));

  // preview does not save / change other windows
  await sp.locator('.skin[data-skin-id="night-letter"]').click();
  await sp.waitForTimeout(300);
  check('preview does not change floating window', (await skinOf(float)).startsWith('leaf-cloak'));
  check('preview does not change the settings window itself', (await skinOf(sp)).startsWith('leaf-cloak'));
  const stored = await sp.evaluate(() => localStorage.getItem('elf-mock-appearance'));
  check('preview did not persist', stored === null, String(stored));
  await shot(sp, 'settings-skins-preview-night-letter-light');

  // snapshot of unrelated state before apply
  const beforeTasks = await panel.evaluate(() => document.body.innerText.length);
  await sp.getByRole('button', { name: '套用造型' }).click();
  await float.waitForFunction(() => document.documentElement.getAttribute('data-skin') === 'night-letter', null, { timeout: 5000 });
  await panel.waitForFunction(() => document.documentElement.getAttribute('data-family') === 'night-voyage', null, { timeout: 5000 });
  check('apply updates floating entry live', (await skinOf(float)) === 'night-letter/night-voyage');
  check('apply updates panel avatar live (img src is the night-letter avatar)', await panel.evaluate(() => /avatar/.test(document.querySelector('.shell__sprite img')?.getAttribute('src') ?? '') && document.querySelector('.shell__sprite img').dataset.skin === 'night-letter'));
  check('theme untouched by applying a skin (stays light)', await panel.evaluate(() => document.documentElement.getAttribute('data-theme')) === 'light');
  const afterStored = JSON.parse(await sp.evaluate(() => localStorage.getItem('elf-mock-appearance')));
  check('persisted skin_id', afterStored.skin_id === 'night-letter' && afterStored.theme === 'system', JSON.stringify(afterStored));
  await sp.waitForTimeout(400);
  await shot(sp, 'settings-skins-applied-night-letter-light');
  await shot(panel, 'panel-after-apply-night-letter-light');
  void beforeTasks;

  // reload a window: stored choice is restored from the backend
  await float.reload();
  await float.waitForSelector('[data-testid="float-expanded"]');
  await float.waitForTimeout(400);
  check('stored skin survives reload', (await skinOf(float)).startsWith('night-letter'));

  // restore default
  await sp.getByRole('button', { name: '還原預設' }).click();
  await float.waitForFunction(() => document.documentElement.getAttribute('data-skin') === 'leaf-cloak', null, { timeout: 5000 });
  check('還原預設 returns to leaf-cloak and quiet-desk family', (await skinOf(float)) === 'leaf-cloak/quiet-desk');
  await ctx.close();

  // fallback: unknown stored id
  const c2 = await browser.newContext({ deviceScaleFactor: 2, locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: 760, height: 1100 } });
  await c2.addInitScript(() => { if (!sessionStorage.getItem('seeded')) { localStorage.setItem('elf-mock-appearance', JSON.stringify({ skin_id: 'ghost-skin', theme: 'system' })); sessionStorage.setItem('seeded', '1'); } });
  const s2 = await c2.newPage();
  watch(s2, 'fallback');
  await s2.goto(`${BASE}/settings.html?theme=light&tab=appearance`);
  await s2.waitForSelector('[data-testid="skin-fallback"]', { timeout: 5000 });
  check('unknown stored skin id: default shown + visible inline notice', (await skinOf(s2)).startsWith('leaf-cloak'));
  await s2.waitForTimeout(300);
  await shot(s2, 'settings-skins-fallback-light');
  const still = await s2.evaluate(() => JSON.parse(localStorage.getItem('elf-mock-appearance')).skin_id);
  check('fallback does not overwrite the stored value', still === 'ghost-skin');
  await c2.close();
}


/* =====================================================================================
 * 2.5D layered sprite (work package F3). Reduced motion is OFF here, except for the fallback check.
 * ===================================================================================== */
const LAYERED_OUT = path.join(OUT, 'layered');
fs.mkdirSync(LAYERED_OUT, { recursive: true });
const BGS = { light: '#d6dde3', dark: '#1c2630', vivid: '#ff2d95', white: '#ffffff', black: '#000000' };
const SKIN_STATE_NAME = { idle: 'idle', listening: 'listening', working: 'processing', awaiting: 'approval', suggestion: 'suggestion', dnd: 'paused' };
const MOTION_PROPS = new Set(['translate', 'scale', 'rotate', 'opacity', 'transform']);

/** pause the looping float/shadow/glow animations at their rest pose so shots and boxes are reproducible */
const freezeAtRest = (pg) =>
  pg.evaluate(() => {
    for (const a of document.getAnimations()) if (String(a.animationName || '').startsWith('ls-')) { a.pause(); a.currentTime = 0; }
  });
const layersReady = (pg) => pg.waitForFunction(() => { const i = [...document.querySelectorAll('.lsprite img')]; return i.length > 0 && i.every((x) => x.complete && x.naturalWidth > 0); });

/** pixel analysis of transparent screenshots / source PNGs inside a throwaway page */
async function analysePage(browser) {
  const ctx = await browser.newContext();
  const pg = await ctx.newPage();
  await pg.setContent('<canvas id=c></canvas>');
  await pg.evaluate(() => {
    window.__px = async (b64) => {
      const img = await new Promise((r, j) => { const i = new Image(); i.onload = () => r(i); i.onerror = j; i.src = 'data:image/png;base64,' + b64; });
      const c = document.createElement('canvas');
      c.width = img.width; c.height = img.height;
      const x = c.getContext('2d', { willReadFrequently: true });
      x.drawImage(img, 0, 0);
      return x.getImageData(0, 0, img.width, img.height);
    };
    const lum = (r, g, b) => (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
    /** transparent screenshot: what is painted outside the sprite box, where the art is */
    window.__outside = async (b64, box) => {
      const d = await window.__px(b64);
      let outside = 0, minX = 1e9, minY = 1e9, maxX = -1, maxY = -1, maxOutsideAlpha = 0;
      for (let y = 0; y < d.height; y++) for (let x = 0; x < d.width; x++) {
        const a = d.data[(y * d.width + x) * 4 + 3];
        if (a > 0 && (x < box[0] || x >= box[2] || y < box[1] || y >= box[3])) { outside++; maxOutsideAlpha = Math.max(maxOutsideAlpha, a); }
        if (a > 8) { minX = Math.min(minX, x); minY = Math.min(minY, y); maxX = Math.max(maxX, x); maxY = Math.max(maxY, y); }
      }
      return { outside, maxOutsideAlpha, bbox: [minX, minY, maxX + 1, maxY + 1], w: d.width, h: d.height };
    };
    /** anti-aliased outer edge of one source layer: how dark/light is the colour that sits in its soft edge? */
    window.__fringe = async (b64) => {
      const d = await window.__px(b64);
      const W = d.width, H = d.height, A = (x, y) => (x < 0 || y < 0 || x >= W || y >= H ? 0 : d.data[(y * W + x) * 4 + 3]);
      let n = 0, sum = 0, inN = 0, inSum = 0, hi = 0;
      for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) {
        const i = (y * W + x) * 4, a = d.data[i + 3];
        if (a >= 12 && a <= 200 && (A(x - 1, y) === 0 || A(x + 1, y) === 0 || A(x, y - 1) === 0 || A(x, y + 1) === 0)) {
          n++; const l = lum(d.data[i], d.data[i + 1], d.data[i + 2]); sum += l; if (l > 0.9) hi++;
        } else if (a >= 250) {
          // opaque pixel within 3 px of the soft edge
          let near = false;
          for (let k = -3; k <= 3 && !near; k++) for (let m = -3; m <= 3; m++) { const q = A(x + k, y + m); if (q > 0 && q < 200) { near = true; break; } }
          if (near) { inN++; inSum += lum(d.data[i], d.data[i + 1], d.data[i + 2]); }
        }
      }
      return { edgePixels: n, edgeLum: n ? sum / n : null, innerLum: inN ? inSum / inN : null, bright: n ? hi / n : 0 };
    };
  });
  return { pg, ctx };
}

async function composeSheet(browser, name, bg, cells, cols, cell, dsf, opts = {}) {
  // cells: [{b64, caption}] transparent PNGs drawn over `bg`, 1 image px = 1/dsf css px unless opts.zoom
  const w = cell + 12;
  const html = `<meta charset=utf-8><style>body{margin:0;background:${bg};font:11px sans-serif;color:${['#ffffff', '#1c2630', '#000000', '#ff2d95'].includes(bg) && bg !== '#ffffff' ? '#fff' : '#1b252d'}}
  .g{display:grid;grid-template-columns:${opts.rowLabels ? '120px ' : ''}repeat(${cols},${w}px);gap:4px;padding:8px;align-items:center}.c{display:grid;justify-items:center}.c img{width:${cell}px;height:${cell}px;image-rendering:${opts.pixelated ? 'pixelated' : 'auto'}}.h{text-align:center;font-weight:bold}</style><div class=g>${cells.map((c) => (c.label ? `<div>${c.label}</div>` : c.head ? `<div class=h>${c.head}</div>` : c.blank ? '<div></div>' : `<div class=c><img src="data:image/png;base64,${c.b64}"><span>${c.caption || ''}</span></div>`)).join('')}</div>`;
  const ctx = await browser.newContext({ deviceScaleFactor: dsf, viewport: { width: (opts.rowLabels ? 128 : 0) + cols * (w + 4) + 16, height: 200 } });
  const pg = await ctx.newPage();
  await pg.setContent(html);
  await pg.waitForFunction(() => [...document.images].every((i) => i.complete));
  await pg.screenshot({ path: path.join(OUT, `${name}.png`), fullPage: true });
  console.log('saved', name);
  await ctx.close();
}

async function runLayered(browser) {
  const ana = await analysePage(browser);
  const idsWithLayered = [];

  /* ---------- A. registry capability flag, six skins x six states, transparency, body boxes ---------- */
  const shots = {}; // id -> state -> base64 (transparent, dsf 2, 128x128 window)
  const bodyBoxes = {};
  const ctx = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: CW, height: CW } }); // motion preference left at its default: no reduce
  const pg = await ctx.newPage();
  watch(pg, 'layered-sheet');
  for (const id of SKIN_IDS) {
    shots[id] = {};
    bodyBoxes[id] = {};
    for (const st of STATES) {
      await pg.goto(`${BASE}/floating.html?theme=light&skin=${id}&state=${st}&dock=none`);
      await pg.waitForSelector(`.lsprite[data-skin="${id}"][data-state="${SKIN_STATE_NAME[st]}"]`, { timeout: 5000 });
      await layersReady(pg);
      await pg.waitForTimeout(260); // let the 140 ms fade-in of glow / marker finish
      await freezeAtRest(pg);
      const info = await pg.evaluate(() => {
        const root = document.querySelector('.lsprite');
        const imgs = [...root.querySelectorAll('img')];
        const on = (el) => Number(getComputedStyle(el).opacity);
        return {
          mounted: imgs.length,
          roleOf: Object.fromEntries(imgs.map((i) => [i.dataset.layer, i.className.replace('l l--', '')])),
          body: imgs.filter((i) => i.classList.contains('l--body')).map((i) => ({ id: i.dataset.layer, src: i.getAttribute('src').replace(/-[A-Za-z0-9_-]{8}\./, '.'), r: i.getBoundingClientRect().toJSON() })),
          eyesOpen: on(root.querySelector('.l--eyes-open')),
          eyesClosed: on(root.querySelector('.l--eyes-closed')),
          marker: !!root.querySelector('.l--state'),
          glow: !!root.querySelector('.l--glow'),
          box: root.getBoundingClientRect().toJSON(),
          filters: imgs.map((i) => getComputedStyle(i).filter + getComputedStyle(i).backdropFilter).filter((f) => f !== 'nonenone'),
        };
      });
      bodyBoxes[id][st] = info;
      const buf = await pg.screenshot({ omitBackground: true });
      shots[id][st] = buf.toString('base64');
      const px = await pg.evaluate(() => 0);
      void px;
    }
    idsWithLayered.push(id);
  }
  await ctx.close();

  check('registry: every skin reports a layered set (capability computed from present assets)', idsWithLayered.length === SKIN_IDS.length && SKIN_IDS.length === 6);
  for (const id of SKIN_IDS) {
    const b = bodyBoxes[id];
    check(`mounted layers <= 8 in every state: ${id}`, STATES.every((st) => b[st].mounted <= 8), STATES.map((st) => b[st].mounted).join('/'));
    const ref = JSON.stringify(b.idle.body.map((x) => [x.id, x.src, x.r]));
    const diffs = STATES.filter((st) => JSON.stringify(b[st].body.map((x) => [x.id, x.src, x.r])) !== ref);
    check(`body layers: same files and identical bounding boxes in all six states: ${id}`, diffs.length === 0, diffs.length ? `differs in ${diffs.join(',')}: ${JSON.stringify(b[diffs[0]].body)} vs ${JSON.stringify(b.idle.body)}` : `${b.idle.body.length} body layers, e.g. ${JSON.stringify(b.idle.body[1]?.r ?? b.idle.body[0].r)}`);
    check(`eyes: open in 5 states, closed in paused: ${id}`, STATES.every((st) => (st === 'dnd' ? b[st].eyesOpen === 0 && b[st].eyesClosed === 1 : b[st].eyesOpen === 1 && b[st].eyesClosed === 0)));
    check(`state marker mounted only for non-idle states, glow only for idle: ${id}`, STATES.every((st) => b[st].marker === (st !== 'idle') && b[st].glow === (st === 'idle')));
    check(`no CSS filter on any layer: ${id}`, STATES.every((st) => b[st].filters.length === 0));
  }

  // transparency: nothing painted outside the 96 px sprite box, art centred in the 128 px window
  const boxDev = [(CW - SP) / 2 * 2, (CW - SP) / 2 * 2, (CW + SP) / 2 * 2, (CW + SP) / 2 * 2];
  const bboxes = [];
  for (const id of SKIN_IDS) {
    let worst = 0;
    for (const st of STATES) {
      const r = await ana.pg.evaluate(([b, box]) => window.__outside(b, box), [shots[id][st], boxDev]);
      worst = Math.max(worst, r.outside);
      if (st === 'idle') bboxes.push({ id, bbox: r.bbox.map((v) => v / 2), cx: (r.bbox[0] + r.bbox[2]) / 4, cy: (r.bbox[1] + r.bbox[3]) / 4 });
    }
    check(`window pixels outside the 96 px sprite box are fully transparent (6 states): ${id}`, worst === 0, worst ? `${worst} painted px` : '');
  }
  console.log('art bbox (alpha>8) in the 128 px window, CSS px [x0,y0,x1,y1] / centre:', bboxes.map((b) => `${b.id} [${b.bbox.join(',')}] c=(${b.cx.toFixed(1)},${b.cy.toFixed(1)})`).join(' | '));

  /* contact sheets: 6 skins x 6 states at 96 px (shown inside the 128 px window), three backgrounds */
  for (const bgName of ['light', 'dark', 'vivid']) {
    const cells = [{ blank: true }, ...STATES.map((st) => ({ head: st }))];
    for (const id of SKIN_IDS) {
      cells.push({ label: id });
      for (const st of STATES) cells.push({ b64: shots[id][st], caption: '' });
    }
    await composeSheet(browser, `layered-sheet-${bgName}`, BGS[bgName], cells, 6, CW, 2, { rowLabels: true });
  }

  /* ---------- B. edge halo / fringe: idle at dsf 4 (native @2x pixels) on white, black, vivid ---------- */
  {
    const hctx = await browser.newContext({ deviceScaleFactor: 4, viewport: { width: CW, height: CW } });
    const hp = await hctx.newPage();
    const idle = {};
    for (const id of SKIN_IDS) {
      await hp.goto(`${BASE}/floating.html?theme=light&skin=${id}&state=idle&dock=none`);
      await hp.waitForSelector('.lsprite[data-scale="2"]');
      await layersReady(hp);
      await hp.waitForTimeout(260);
      await freezeAtRest(hp);
      idle[id] = (await hp.screenshot({ omitBackground: true })).toString('base64');
    }
    await hctx.close();
    for (const bgName of ['white', 'black', 'vivid']) {
      await composeSheet(browser, `layered-halo-${bgName}`, BGS[bgName], SKIN_IDS.map((id) => ({ b64: idle[id], caption: id })), 3, 384, 1);
    }
    // numeric edge metrics straight from the delivered @2x layers
    const rows = [];
    for (const id of SKIN_IDS) {
      const layers = JSON.parse(fs.readFileSync(path.join(SKIN_DIR, id, 'layered', 'layers.json'), 'utf8')).layers.filter((l) => ['body', 'accessory', 'highlight'].includes(l.role));
      for (const l of layers) {
        const b64 = fs.readFileSync(path.join(SKIN_DIR, id, 'layered', `${l.file}@2x.png`)).toString('base64');
        const f = await ana.pg.evaluate((x) => window.__fringe(x), b64);
        rows.push({ id, layer: l.id, ...f });
      }
    }
    console.log('edge fringe (soft outer edge colour vs nearby opaque colour; luminance 0..1):');
    for (const r of rows) {
      if (!r.edgePixels) { console.log(`  ${r.id}/${r.layer}: no soft edge pixels`); continue; }
      const ratio = r.innerLum ? r.edgeLum / r.innerLum : NaN;
      const flag = r.edgeLum < 0.12 && r.innerLum > 0.25 ? ' DARK-FRINGE?' : r.bright > 0.5 && r.innerLum < 0.7 ? ' LIGHT-FRINGE?' : '';
      console.log(`  ${r.id}/${r.layer}: edge px ${r.edgePixels}, edgeLum ${r.edgeLum.toFixed(2)}, innerLum ${r.innerLum?.toFixed(2)}, ratio ${ratio.toFixed(2)}${flag}`);
    }
    fs.writeFileSync(path.join(LAYERED_OUT, 'edge-fringe.json'), JSON.stringify(rows, null, 1));
  }

  /* ---------- C. motion: float, shadow, glow, accessory lag, blink timing, only-transform/opacity ---------- */
  {
    const c = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: CW, height: CW } });
    const idle = await c.newPage();
    const paused = await c.newPage();
    watch(idle, 'layered-motion-idle');
    watch(paused, 'layered-motion-paused');
    await idle.goto(`${BASE}/floating.html?theme=light&backdrop=light&skin=night-letter&state=idle&dock=none`);
    await paused.goto(`${BASE}/floating.html?theme=light&backdrop=light&skin=night-letter&state=dnd&dock=none`);
    await idle.waitForSelector('.lsprite');
    await paused.waitForSelector('.lsprite');
    await layersReady(idle);
    await layersReady(paused);
    const watchBlinks = (pg2) =>
      pg2.evaluate(() => {
        window.__blinks = [];
        const root = document.querySelector('.lsprite');
        let t0 = null;
        new MutationObserver(() => {
          const on = root.dataset.blinking === '1';
          const now = performance.now();
          if (on) t0 = now;
          else if (t0 !== null) { window.__blinks.push({ at: t0, dur: now - t0 }); t0 = null; }
        }).observe(root, { attributes: true, attributeFilter: ['data-blinking'] });
      });
    await watchBlinks(idle);
    await watchBlinks(paused);

    // float sampling (4.5 s, every 150 ms): body-role layers together, accessory lags, shadow counter-phase
    const samples = await idle.evaluate(async () => {
      const q = (s) => document.querySelector(s);
      const ty = (el) => parseFloat((getComputedStyle(el).translate || '0px 0px').split(' ')[1] || '0');
      const out = [];
      for (let i = 0; i < 30; i++) {
        out.push({ body: ty(q('.l--body[data-layer="body"]')), back: ty(q('.l--body[data-layer="body-back"]')), hl: ty(q('.l--highlight')), eyes: ty(q('.l--eyes-open')), acc: ty(q('.l--accessory')), shadowOp: Number(getComputedStyle(q('.l--shadow')).opacity), shadowScale: parseFloat(getComputedStyle(q('.l--shadow')).scale || '1'), glow: Number(getComputedStyle(q('.l--glow')).opacity) });
        await new Promise((r) => setTimeout(r, 150));
      }
      return out;
    });
    const range = (k) => [Math.min(...samples.map((x) => x[k])), Math.max(...samples.map((x) => x[k]))];
    const [bmin, bmax] = range('body');
    check('float: body moves within about +-3 px (and really moves)', bmin >= -3.05 && bmax <= 3.05 && bmax - bmin > 4, `range ${bmin.toFixed(2)}..${bmax.toFixed(2)}`);
    check('float: body-back, highlight and eyes move together with the body', samples.every((x) => Math.abs(x.body - x.back) < 0.01 && Math.abs(x.body - x.hl) < 0.01 && Math.abs(x.body - x.eyes) < 0.01));
    check('float: accessory trails the body (never identical while moving)', samples.some((x) => Math.abs(x.body - x.acc) > 0.05) && samples.every((x) => Math.abs(x.body - x.acc) < 1.5), `max lag ${Math.max(...samples.map((x) => Math.abs(x.body - x.acc))).toFixed(2)} px`);
    const mean = (k) => samples.reduce((a, x) => a + x[k], 0) / samples.length;
    const corr = (a, b) => { const ma = mean(a), mb = mean(b); let n = 0, da = 0, db = 0; for (const x of samples) { n += (x[a] - ma) * (x[b] - mb); da += (x[a] - ma) ** 2; db += (x[b] - mb) ** 2; } return n / Math.sqrt(da * db); };
    check('shadow: scale and opacity follow the body in counter-phase (body up = shadow smaller/fainter)', corr('body', 'shadowOp') > 0.95 && corr('body', 'shadowScale') > 0.95, `corr opacity ${corr('body', 'shadowOp').toFixed(3)}, scale ${corr('body', 'shadowScale').toFixed(3)}`);
    const [gmin, gmax] = range('glow');
    check('glow breathes in opacity (0.7..1)', gmax - gmin > 0.12 && gmin >= 0.7 && gmax <= 1.001, `${gmin.toFixed(2)}..${gmax.toFixed(2)}`);
    const timing = await idle.evaluate(() => document.getAnimations().filter((a) => a.animationName === 'ls-float').map((a) => { const t = a.effect.getComputedTiming(); return { dur: t.duration, dir: t.direction, it: t.iterations, delay: t.delay }; }));
    check('float period is 4 s (2 s alternate) on every floating layer', timing.length >= 5 && timing.every((t) => t.dur === 2000 && t.dir === 'alternate' && t.it === Infinity), JSON.stringify(timing[0]) + ` x${timing.length}`);
    const props = await idle.evaluate(() => { const set = new Set(); for (const a of document.getAnimations()) for (const k of a.effect.getKeyframes()) for (const p of Object.keys(k)) if (!['offset', 'easing', 'composite', 'computedOffset'].includes(p)) set.add(p); return [...set]; });
    check('animated properties are only transform-family and opacity', props.length > 0 && props.every((p) => MOTION_PROPS.has(p)), props.join(','));
    const canvases = await idle.locator('canvas').count();
    check('no canvas in the floating window', canvases === 0);

    // blink + idle sleep: watch both pages for 31.5 s without any pointer activity
    const t0 = Date.now();
    await idle.waitForTimeout(Math.max(0, 31500 - 4500));
    const b = await idle.evaluate(() => window.__blinks);
    const pb = await paused.evaluate(() => window.__blinks);
    console.log(`blinks seen on idle page in ${((Date.now() - t0 + 4500) / 1000).toFixed(1)} s: ${b.length}; durations ${b.map((x) => Math.round(x.dur)).join(',')} ms; gaps ${b.slice(1).map((x, i) => Math.round(x.at - b[i].at)).join(',')} ms`);
    check('blink: happens, about 150 ms each', b.length >= 3 && b.every((x) => x.dur >= 120 && x.dur <= 230), `${b.length} blinks`);
    check('blink: random interval 4-7 s (+150 ms)', b.slice(1).every((x, i) => { const gap = x.at - b[i].at; return gap >= 4000 && gap <= 7400; }) && (b.length === 0 || b[0].at > 0));
    check('paused state never blinks', pb.length === 0, `${pb.length} blinks in the same window`);
    // 30 s after the last pointer activity every animation must be paused/finished
    const asleep = await idle.evaluate(() => ({ asleep: document.querySelector('.lsprite').dataset.asleep, anims: document.getAnimations().map((a) => `${a.animationName || a.transitionProperty}:${a.playState}`) }));
    check('after 30 s without pointer activity: no CSS animation is running', asleep.asleep === '1' && asleep.anims.every((a) => !a.endsWith(':running')), JSON.stringify(asleep));
    const blinkAfter = await idle.evaluate(() => window.__blinks.length);
    await idle.waitForTimeout(7500);
    check('after sleeping the blink timer is cancelled too', (await idle.evaluate(() => window.__blinks.length)) === blinkAfter);
    const pausedAnims = await paused.evaluate(() => document.getAnimations().map((a) => `${a.animationName}:${a.playState}`));
    check('paused state: floating is frozen (no running animation)', pausedAnims.every((a) => !a.endsWith(':running')), pausedAnims.join(' '));
    // resume on pointer activity
    await idle.mouse.move(CC + 5, CC + 5);
    await idle.waitForTimeout(200);
    const resumed = await idle.evaluate(() => ({ asleep: document.querySelector('.lsprite').dataset.asleep, running: document.getAnimations().filter((a) => a.playState === 'running').length }));
    check('pointer over the window wakes the animation again', resumed.asleep === '0' && resumed.running > 0, JSON.stringify(resumed));
    // hidden document (synthetic visibilitychange: the native Tauri hide still needs a real-window test)
    await idle.evaluate(() => { Object.defineProperty(document, 'hidden', { configurable: true, get: () => true }); document.dispatchEvent(new Event('visibilitychange')); });
    await idle.waitForTimeout(450); // let the 260 ms tilt transition from the pointer move above finish
    const hid = await idle.evaluate(() => ({ asleep: document.querySelector('.lsprite').dataset.asleep, running: document.getAnimations().filter((a) => a.playState === 'running').length }));
    check('hidden document pauses all animation (synthetic visibilitychange)', hid.asleep === '1' && hid.running === 0, JSON.stringify(hid));
    await idle.evaluate(() => { Object.defineProperty(document, 'hidden', { configurable: true, get: () => false }); document.dispatchEvent(new Event('visibilitychange')); });
    await idle.waitForTimeout(100);
    check('visible again: animation resumes', (await idle.evaluate(() => document.querySelector('.lsprite').dataset.asleep)) === '0');
    await c.close();
  }

  /* ---------- D. blink frame + tilt frames, drag lean ---------- */
  {
    const c = await browser.newContext({ deviceScaleFactor: 4, viewport: { width: CW, height: CW } });
    const pg2 = await c.newPage();
    watch(pg2, 'layered-tilt');
    const grab = async () => (await pg2.screenshot({ omitBackground: true })).toString('base64');
    await pg2.goto(`${BASE}/floating.html?theme=light&skin=night-letter&state=idle&dock=none`);
    await layersReady(pg2);
    await pg2.waitForTimeout(300);
    await freezeAtRest(pg2);
    const rest = await grab();
    // the blink itself is timer-driven (timing proven above); here force the attribute to look at the frame
    await pg2.evaluate(() => (document.querySelector('.lsprite').dataset.blinking = '1'));
    const eyes = await pg2.evaluate(() => ({ open: getComputedStyle(document.querySelector('.l--eyes-open')).opacity, closed: getComputedStyle(document.querySelector('.l--eyes-closed')).opacity }));
    check('blink frame: eyes-open hidden, eyes-closed shown, nothing else changed', eyes.open === '0' && eyes.closed === '1', JSON.stringify(eyes));
    const blink = await grab();
    await pg2.evaluate(() => delete document.querySelector('.lsprite').dataset.blinking);
    // tilt right / left
    const tiltOf = () => pg2.evaluate(() => ({ v: parseFloat(document.querySelector('.lsprite').style.getPropertyValue('--tilt') || '0'), m: getComputedStyle(document.querySelector('.lsprite__rig')).transform, hx: document.querySelector('.lsprite').style.getPropertyValue('--hx') }));
    await pg2.mouse.move(CC + 44, CC);
    await pg2.waitForTimeout(450);
    const right = await tiltOf();
    const rightImg = await grab();
    check('tilt: leans right (0 < angle <= 4 deg) while the pointer is near', right.v > 0 && right.v <= 4.001 && right.m !== 'none', JSON.stringify(right));
    const deg = (m) => { const mm = /matrix\(([^)]+)\)/.exec(m); if (!mm) return 0; const [a, b2] = mm[1].split(',').map(Number); return (Math.atan2(b2, a) * 180) / Math.PI; };
    check('tilt: the rig really rotates by that angle around the pivot', Math.abs(deg(right.m) - right.v) < 0.3, `css rotation ${deg(right.m).toFixed(2)} deg vs ${right.v}`);
    check('tilt: highlight slides against the tilt', parseFloat(right.hx) < 0, right.hx);
    await pg2.mouse.move(CC - 44, CC);
    await pg2.waitForTimeout(450);
    const left = await tiltOf();
    const leftImg = await grab();
    check('tilt: leans left for a pointer on the left', left.v < 0 && left.v >= -4.001, JSON.stringify(left));
    await pg2.evaluate(() => document.documentElement.dispatchEvent(new PointerEvent('pointerleave')));
    await pg2.waitForTimeout(450);
    const back = await tiltOf();
    check('tilt: returns to rest when the pointer leaves', back.v === 0 && Math.abs(deg(back.m)) < 0.05, JSON.stringify(back));
    const tr = await pg2.evaluate(() => getComputedStyle(document.querySelector('.lsprite__rig')).transitionDuration);
    check('tilt: the return is a smooth transition, not a jump', parseFloat(tr) >= 0.2, tr);
    // drag: lean with the movement, bounded
    await pg2.mouse.move(CC, CC);
    await pg2.mouse.down();
    for (let i = 0; i < 6; i++) await pg2.mouse.move(CC + 6 * (i + 1), CC, { steps: 2 });
    const dragR = await tiltOf();
    check('drag: leans in the direction of movement (right), max 4 deg', dragR.v > 0 && dragR.v <= 4.001, JSON.stringify(dragR));
    await pg2.mouse.up();
    await pg2.mouse.move(CC, CC);
    await pg2.mouse.down();
    for (let i = 0; i < 6; i++) await pg2.mouse.move(CC - 6 * (i + 1), CC, { steps: 2 });
    const dragL = await tiltOf();
    check('drag: leans left when dragged left', dragL.v < 0 && dragL.v >= -4.001, JSON.stringify(dragL));
    await pg2.mouse.up();
    await composeSheet(browser, 'layered-blink-tilt', BGS.light, [{ b64: rest, caption: 'rest' }, { b64: blink, caption: 'blink (eyes closed)' }, { b64: rightImg, caption: `tilt right ${right.v.toFixed(1)} deg` }, { b64: leftImg, caption: `tilt left ${left.v.toFixed(1)} deg` }], 4, 256, 2);
    await c.close();
  }

  /* ---------- E. live state changes: body never remounts, markers cross-fade, preload ---------- */
  {
    const c = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', viewport: { width: CW, height: CW } });
    const fl = await c.newPage();
    watch(fl, 'layered-live-float');
    await fl.goto(`${BASE}/floating.html?theme=light&backdrop=light&skin=night-letter&dock=none`);
    await fl.waitForSelector('.lsprite[data-state="idle"]');
    await layersReady(fl);
    await fl.waitForTimeout(500);
    await fl.evaluate(() => {
      const root = document.querySelector('.lsprite');
      window.__tagged = [...root.querySelectorAll('.l--body, .l--highlight, .l--eyes-open, .l--eyes-closed, .l--accessory')].map((el) => ({ el, src: el.getAttribute('src'), id: el.dataset.layer }));
      window.__log = [];
      let max = 0;
      window.__maxMounted = () => max;
      new MutationObserver((muts) => {
        const now = Math.round(performance.now());
        for (const m of muts) {
          for (const n of m.addedNodes) if (n.nodeType === 1) window.__log.push({ t: now, add: (n.querySelector?.('img') || n).dataset?.layer, fade: n.className });
          for (const n of m.removedNodes) if (n.nodeType === 1) window.__log.push({ t: now, rem: (n.querySelector?.('img') || n).dataset?.layer });
        }
        max = Math.max(max, root.querySelectorAll('img').length);
      }).observe(root, { childList: true, subtree: true });
      const imgs = new Set();
      window.__srcChanges = 0;
      new MutationObserver((muts) => { for (const m of muts) if (m.attributeName === 'src' && window.__tagged.some((t) => t.el === m.target) && m.target.getAttribute('src') !== m.oldValue) window.__srcChanges++; }).observe(root, { attributes: true, attributeFilter: ['src'], attributeOldValue: true, subtree: true });
      imgs.clear();
    });
    const panel = await c.newPage();
    watch(panel, 'layered-live-panel');
    await panel.setViewportSize({ width: 420, height: 720 });
    await panel.goto(`${BASE}/panel.html?theme=light&mockSpeed=3`);
    await panel.waitForSelector('text=有什麼可以交給我');
    await panel.getByRole('button', { name: '用範例會議筆記試試' }).click();
    await panel.getByRole('button', { name: '送出' }).click();
    // The floating page is not forced to a state here, so remove the URL override: it reads live state
    await fl.bringToFront();
    const seen = new Set();
    const t1 = Date.now();
    while (Date.now() - t1 < 30000 && !seen.has('approval')) {
      const st = await fl.evaluate(() => document.querySelector('.lsprite')?.dataset.state);
      if (st) seen.add(st);
      await fl.waitForTimeout(100);
    }
    await fl.waitForTimeout(400);
    const res = await fl.evaluate(() => ({
      same: window.__tagged.every((t) => t.el.isConnected && t.el.getAttribute('src') === t.src),
      n: window.__tagged.length,
      log: window.__log,
      maxMounted: window.__maxMounted(),
      steady: document.querySelectorAll('.lsprite img').length,
      srcChanges: window.__srcChanges,
      leaving: document.querySelectorAll('.l-fade--out').length,
    }));
    console.log('live state sequence seen on the sprite:', [...seen].join(' -> '), '| mutation log:', JSON.stringify(res.log));
    check('live: sprite went through more than one state', seen.size >= 2, [...seen].join(','));
    check('live: body, highlight, eyes and accessory DOM nodes survive every state change (no remount, no src swap)', res.same && res.srcChanges === 0, `${res.n} nodes, src changes ${res.srcChanges}`);
    check('live: only glow / state-marker layers are added or removed', res.log.every((e) => ['glow', undefined].includes(e.add ?? e.rem) || /^state-/.test(e.add ?? e.rem ?? '')), JSON.stringify(res.log.map((e) => e.add ?? e.rem)));
    check('live: at most 9 layers mounted during a cross-fade, 8 afterwards, none left fading', res.maxMounted <= 9 && res.steady <= 8 && res.leaving === 0, `max ${res.maxMounted}, steady ${res.steady}`);
    // preload: every file of the skin is already in the browser cache after first paint
    const loaded = await fl.evaluate(() => performance.getEntriesByType('resource').filter((r) => /\.png/.test(r.name)).length);
    check('preload: all 13 layer files (8 base + 5 markers) of the active skin were requested up front', loaded >= 13, `${loaded} png requests`);
    await c.close();
  }

  /* ---------- F. wheel around the 96 px layered sprite; click / drag threshold unchanged ---------- */
  for (const bg of ['light', 'dark']) {
    const c = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', viewport: { width: EW, height: EW } });
    const w = await c.newPage();
    watch(w, `layered-wheel-${bg}`);
    await w.goto(`${BASE}/floating.html?theme=${bg}&backdrop=${bg}&skin=night-letter&dock=none`);
    await w.waitForSelector('.lsprite');
    await layersReady(w);
    await w.waitForTimeout(400);
    const log = () => w.evaluate(() => window.__elfMock.expandedLog);
    await w.mouse.move(2, 2);
    await w.mouse.move(EC, EC);
    await w.waitForTimeout(100);
    await w.mouse.move(2, 2);
    await w.waitForTimeout(500);
    check(`layered: pass-over does not open the wheel (${bg})`, (await log()).length === 0);
    await w.mouse.move(EC, EC);
    await w.waitForSelector('.wheel', { state: 'attached', timeout: 2500 });
    const l1 = await log();
    check(`layered: window grows before the wheel renders (${bg})`, l1.length === 1 && l1[0].expanded === true && l1[0].wheelInDom === false, JSON.stringify(l1));
    await w.waitForTimeout(500);
    await freezeAtRest(w);
    const geo = await w.evaluate(() => {
      const r = (el) => el.getBoundingClientRect().toJSON();
      return { sprite: r(document.querySelector('.float__sprite')), art: r(document.querySelector('.lsprite')), items: [...document.querySelectorAll('.wheel__item')].map((e) => ({ id: e.dataset.item, ...r(e) })) };
    });
    const gap = (a, b2) => Math.max(Math.max(a.left, b2.left) - Math.min(a.right, b2.right), Math.max(a.top, b2.top) - Math.min(a.bottom, b2.bottom));
    let minItems = 1e9, minSprite = 1e9;
    geo.items.forEach((a, i) => { minSprite = Math.min(minSprite, gap(a, geo.sprite)); geo.items.slice(i + 1).forEach((b2) => (minItems = Math.min(minItems, gap(a, b2)))); });
    check(`layered: sprite is ${SP}px and centred in the ${EW}px window while the wheel is open (${bg})`, geo.sprite.width === SP && geo.art.width === SP && Math.abs(geo.sprite.x + SP / 2 - EC) < 0.5 && Math.abs(geo.sprite.y + SP / 2 - EC) < 0.5, JSON.stringify(geo.sprite));
    check(`layered: six wheel items, each >= 44 px in both directions (${bg})`, geo.items.length === 6 && geo.items.every((i) => i.width >= 44 && i.height >= 44));
    check(`layered: >= ${LAYOUT.GAP} px between items and between items and the sprite (${bg})`, minItems >= LAYOUT.GAP && minSprite >= LAYOUT.GAP, `items ${minItems.toFixed(1)} px, sprite ${minSprite.toFixed(1)} px`);
    check(`layered: items inside the window with room for shadows (${bg})`, geo.items.every((i) => i.left >= 16 && i.top >= 16 && i.right <= EW - 16 && i.bottom <= EW - 16), geo.items.map((i) => `${i.id}:${Math.round(i.left)},${Math.round(i.top)}`).join(' '));
    await shot(w, `layered-wheel-open-${bg}`);
    // leave: closes
    await w.mouse.move(2, 2);
    await w.waitForTimeout(900);
    check(`layered: wheel closes after leaving (${bg})`, (await w.locator('.wheel').count()) === 0);
    if (bg === 'light') {
      // click without movement opens the panel (popup in the browser mock); movement > 4 px is a drag, not a click
      await w.mouse.move(EC, EC);
      let popup = null;
      w.once('popup', (p) => (popup = p));
      await w.mouse.down();
      await w.mouse.move(EC + 3, EC); // under the 4 px threshold
      await w.mouse.up();
      await w.waitForTimeout(600);
      check('layered: a press with < 4 px movement is still a click (opens the panel)', !!popup);
      if (popup) await popup.close();
      popup = null;
      await w.mouse.move(2, 2);
      await w.waitForTimeout(400);
      await w.mouse.move(EC, EC);
      w.once('popup', (p) => (popup = p));
      await w.mouse.down();
      await w.mouse.move(EC + 12, EC + 6, { steps: 4 });
      await w.mouse.up();
      await w.waitForTimeout(600);
      check('layered: a press that moves > 4 px is a drag, not a click', !popup);
      check('layered: dragging never opens the wheel', (await w.locator('.wheel').count()) === 0 && (await log()).length === 2 && (await log()).at(-1).expanded === false);
    }
    await c.close();
  }

  /* ---------- G. reduced motion falls back to the flat art ---------- */
  {
    const c = await browser.newContext({ deviceScaleFactor: 2, reducedMotion: 'reduce', viewport: { width: CW, height: CW } });
    const r = await c.newPage();
    watch(r, 'layered-reduced');
    await r.goto(`${BASE}/floating.html?theme=light&backdrop=light&skin=night-letter&state=idle&dock=none`);
    await r.waitForSelector('[data-testid="float-expanded"] img.sprite');
    await r.waitForTimeout(300);
    const info = await r.evaluate(() => ({ layered: document.querySelectorAll('.lsprite').length, flat: document.querySelectorAll('img.sprite').length, src: document.querySelector('img.sprite').getAttribute('src'), anims: document.getAnimations().length, w: document.querySelector('img.sprite').getBoundingClientRect().width }));
    check('reduced motion: flat static art, no layered DOM, no animation', info.layered === 0 && info.flat === 1 && /idle.*\.svg$/.test(info.src) && info.anims === 0, JSON.stringify(info));
    await shot(r, 'layered-reduced-motion-flat');
    await c.close();
  }

  /* ---------- H. 128 px collapsed window bounding box ---------- */
  {
    const c = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: CW, height: CW } });
    const p = await c.newPage();
    watch(p, 'layered-collapsed');
    await p.goto(`${BASE}/floating.html?theme=light&backdrop=light&skin=night-letter&state=idle&dock=none`);
    await layersReady(p);
    await p.waitForTimeout(300);
    await freezeAtRest(p);
    const r = await p.evaluate(() => ({ btn: document.querySelector('.float__sprite').getBoundingClientRect().toJSON(), art: document.querySelector('.lsprite').getBoundingClientRect().toJSON(), vw: innerWidth, vh: innerHeight }));
    check(`collapsed ${CW}x${CW}: viewport matches, ${SP} px sprite button and art box centred`, r.vw === CW && r.vh === CW && r.btn.width === SP && r.art.width === SP && Math.abs(r.btn.x + SP / 2 - CC) < 0.5 && Math.abs(r.btn.y + SP / 2 - CC) < 0.5 && Math.abs(r.art.x - r.btn.x) < 0.5, JSON.stringify(r));
    const nl = bboxes.find((b) => b.id === 'night-letter');
    await p.evaluate(({ bb }) => {
      const box = (x, y, w, h, col) => { const d = document.createElement('div'); d.style.cssText = `position:fixed;left:${x}px;top:${y}px;width:${w}px;height:${h}px;border:1px solid ${col};box-sizing:border-box;pointer-events:none;z-index:9`; document.body.appendChild(d); };
      box(0, 0, innerWidth, innerHeight, '#e00'); // window
      const s = document.querySelector('.float__sprite').getBoundingClientRect();
      box(s.x, s.y, s.width, s.height, '#00e'); // 96 px sprite box
      box(bb[0], bb[1], bb[2] - bb[0], bb[3] - bb[1], '#0a0'); // visible art (alpha > 8)
    }, { bb: nl.bbox });
    await shot(p, 'layered-collapsed-bounds');
    console.log(`collapsed window: red = ${CW}x${CW} window, blue = ${SP} px sprite box centred at ${CC},${CC}; green = night-letter visible art [${nl.bbox.join(',')}] centre (${nl.cx.toFixed(1)}, ${nl.cy.toFixed(1)})`);
    await c.close();
  }

  /* ---------- I. settings: 2.5D cards, calm grid, preview ---------- */
  for (const theme of ['light', 'dark']) {
    const c = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', viewport: { width: 760, height: 1500 } });
    const sp = await c.newPage();
    watch(sp, `layered-settings-${theme}`);
    await sp.goto(`${BASE}/settings.html?theme=${theme}&tab=appearance`);
    await sp.waitForSelector('.skin');
    await sp.locator('.skin[data-skin-id="night-letter"]').click();
    await layersReady(sp);
    await sp.waitForTimeout(500);
    const info = await sp.evaluate(() => ({
      cards: document.querySelectorAll('.skin .lsprite').length,
      animatedCards: [...document.querySelectorAll('.skin .lsprite[data-animate="1"]')].map((e) => e.dataset.skin),
      badges: document.querySelectorAll('[data-testid="badge-layered"]').length,
      previewLayered: document.querySelectorAll('.skinpreview .lsprite').length,
      previewAnimated: document.querySelectorAll('.skinpreview .lsprite[data-animate="1"]').length,
      previewSize: document.querySelector('.skinpreview .lsprite')?.getBoundingClientRect().width,
      badgeText: document.querySelector('[data-testid="badge-layered"]')?.textContent,
    }));
    check(`settings (${theme}): six cards use layered art, 2.5D marker on each`, info.cards === 6 && info.badges === 6 && info.badgeText === '2.5D', JSON.stringify(info));
    check(`settings (${theme}): only the selected card is animated`, info.animatedCards.length === 1 && info.animatedCards[0] === 'night-letter', info.animatedCards.join(','));
    check(`settings (${theme}): six-state preview is layered, 12 sprites at 64 px, all still`, info.previewLayered === 12 && info.previewAnimated === 0 && info.previewSize === 64, `${info.previewLayered} sprites, ${info.previewAnimated} animated, ${info.previewSize}px`);
    await sp.waitForTimeout(300);
    await shot(sp, `layered-settings-${theme}`);
    if (theme === 'light') {
      await sp.locator('.skin[data-skin-id="orbit-courier"]').click();
      await sp.waitForTimeout(300);
      const moved = await sp.evaluate(() => [...document.querySelectorAll('.skin .lsprite[data-animate="1"]')].map((e) => e.dataset.skin));
      check('settings: the animation follows the selection', moved.length === 1 && moved[0] === 'orbit-courier', moved.join(','));
    }
    await c.close();
  }

  /* ---------- J. whole-window transparency proof for the layered sprite (checkerboard) ---------- */
  for (const wheel of [false, true]) {
    const size = wheel ? EW : CW;
    const c = await browser.newContext({ deviceScaleFactor: 1, viewport: { width: size + 40, height: size + 40 } });
    const t = await c.newPage();
    watch(t, 'layered-transparency');
    const board = `background-color:#ff00ff;background-image:linear-gradient(45deg,#00ff00 25%,transparent 25%,transparent 75%,#00ff00 75%),linear-gradient(45deg,#00ff00 25%,transparent 25%,transparent 75%,#00ff00 75%);background-size:20px 20px;background-position:0 0,10px 10px`;
    const frame = (on) => `<body style="margin:0;${board}"><div style="position:absolute;left:20px;top:20px;width:${size}px;height:${size}px">` + (on ? `<iframe id=f src="${BASE}/floating.html?theme=light&dock=none&skin=night-letter" style="border:0;width:${size}px;height:${size}px;background:transparent"></iframe>` : '') + '</div></body>';
    await t.setContent(frame(false));
    const bare = await t.screenshot();
    await t.setContent(frame(true));
    const fr = t.frameLocator('#f');
    await fr.locator('.lsprite').waitFor();
    await t.waitForTimeout(700);
    if (wheel) {
      await t.mouse.move(20 + size / 2, 20 + size / 2);
      await fr.locator('.wheel').waitFor({ state: 'attached' });
      await t.waitForTimeout(300);
    }
    const withFrame = await t.screenshot();
    fs.writeFileSync(path.join(OUT, `layered-transparency-${wheel ? 'wheel-' : ''}checkerboard.png`), withFrame);
    const verdict = await ana.pg.evaluate(async ({ a, b2, size: sz, wheel: wh, L }) => {
      const [A, B] = await Promise.all([window.__px(a), window.__px(b2)]);
      const cx = 20 + sz / 2, cy = 20 + sz / 2;
      const rects = [[cx - L.SPRITE / 2, cy - L.SPRITE / 2, L.SPRITE, L.SPRITE]]; // layered art paints only inside its own 96 px box (no filter, no halo outside)
      if (wh) for (let i = 0; i < 6; i++) { const ang = ((-90 + 60 * i) * Math.PI) / 180; const x = Math.round(L.RX * Math.cos(ang)), y = Math.round(L.RY * Math.sin(ang)); rects.push([cx + x - L.ITEM_W / 2 - 32, cy + y - L.ITEM_H / 2 - 32, L.ITEM_W + 64, L.ITEM_H + 64]); }
      let diff = 0, total = 0;
      for (let y = 0; y < A.height; y++) for (let x = 0; x < A.width; x++) {
        if (rects.some(([rx, ry, rw, rh]) => x >= rx && x < rx + rw && y >= ry && y < ry + rh)) continue;
        total++;
        const i = (y * A.width + x) * 4;
        if (Math.abs(A.data[i] - B.data[i]) + Math.abs(A.data[i + 1] - B.data[i + 1]) + Math.abs(A.data[i + 2] - B.data[i + 2]) > 6) diff++;
      }
      return { diff, total };
    }, { a: bare.toString('base64'), b2: withFrame.toString('base64'), size, wheel, L: LAYOUT });
    check(`layered: transparent outside the sprite box over a checkerboard${wheel ? ' (wheel open)' : ''}`, verdict.diff === 0, `${verdict.diff} of ${verdict.total} outside pixels differ`);
    await c.close();
  }

  await ana.ctx.close();
}

(async () => {
  const want = process.argv.slice(3);
  const on = (k) => want.length === 0 || want.includes(k);
  const browser = await chromium.launch();
  if (on('scenario')) {
    for (const theme of ['light', 'dark']) await runTheme(browser, theme);
    await checkResync(browser);
  }
  if (on('skins')) await runSkins(browser);
  if (on('families')) await runFamilies(browser);
  if (on('wheel')) await runWheel(browser);
  if (on('transparency')) await runTransparency(browser);
  if (on('flow')) await runAppearanceFlow(browser);
  if (on('layered')) await runLayered(browser);
  await browser.close();
  const failed = results.filter((r) => !r.ok);
  console.log(`\nCHECKS: ${results.length - failed.length} passed, ${failed.length} failed`);
  for (const f of failed) console.log(`  FAILED: ${f.name} ${f.detail}`);
  console.log(problems.length ? `PROBLEMS (${problems.length}):\n` + problems.join('\n') : 'No console errors or page errors.');
  if (failed.length) process.exitCode = 1;
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
