/**
 * Drives the mock demo scenario with Playwright and saves screenshots.
 * Usage: node tests-ui/capture.cjs [baseUrl]   (default http://localhost:4173)
 * Needs `vite preview` (or dev server) running. Playwright is pre-installed globally.
 */
const { chromium } = require('playwright');
const path = require('node:path');
const fs = require('node:fs');

const BASE = process.argv[2] || 'http://localhost:4173';
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
  await f2.setViewportSize({ width: 96, height: 96 });
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
    const ctx = await browser.newContext({ deviceScaleFactor: 2, reducedMotion: 'reduce', viewport: { width: 96, height: 96 } });
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
    await contactSheet(browser, files, 6, `skins-sheet-${bg}`, bg === 'light' ? '#d6dde3' : '#1c2630', 96);
  }

  // docked (collapsed) form for every skin, left and right, idle + awaiting
  for (const bg of ['light', 'dark']) {
    const ctx = await browser.newContext({ deviceScaleFactor: 4, reducedMotion: 'reduce', viewport: { width: 96, height: 96 } });
    const pg = await ctx.newPage();
    const files = [];
    for (const id of SKIN_IDS) {
      for (const [dock, st] of [['left', 'idle'], ['left', 'awaiting'], ['right', 'idle']]) {
        await pg.goto(`${BASE}/floating.html?theme=${bg}&backdrop=${bg}&skin=${id}&state=${st}&dock=${dock}`);
        await pg.waitForSelector('[data-testid="float-collapsed"] img');
        await pg.waitForFunction(() => [...document.images].every((i) => i.complete && i.naturalWidth > 0));
        const file = path.join(SK_OUT, `${id}-docked-${dock}-${st}-${bg}.png`);
        await pg.screenshot({ path: file, clip: { x: dock === 'left' ? 0 : 48, y: 24, width: 48, height: 48 } });
        files.push({ src: file, caption: `${id.split('-')[0]} ${dock[0]}/${st[0]}` });
      }
    }
    await ctx.close();
    await contactSheet(browser, files, 6, `skins-docked-sheet-${bg}`, bg === 'light' ? '#d6dde3' : '#1c2630', 96);
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

/** Real hover interactions on the floating entry (viewport = expanded 280x280 window). */
async function runWheel(browser) {
  for (const [theme, bg] of [['light', 'light'], ['dark', 'dark'], ['light', 'dark'], ['dark', 'light']]) {
    const ctx = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: 280, height: 280 } });
    const pg = await ctx.newPage();
    watch(pg, `wheel-${theme}-${bg}`);
    await pg.goto(`${BASE}/floating.html?theme=${theme}&backdrop=${bg}&dock=none`);
    await pg.waitForSelector('[data-testid="float-expanded"]');
    await pg.waitForFunction(() => document.documentElement.getAttribute('data-skin'));
    await pg.waitForTimeout(500); // store sync
    const log = () => pg.evaluate(() => window.__elfMock.expandedLog);

    // passing over the sprite (< 250 ms) must not open
    await pg.mouse.move(10, 10);
    await pg.mouse.move(140, 140);
    await pg.waitForTimeout(100);
    await pg.mouse.move(10, 10);
    await pg.waitForTimeout(500);
    check(`pass-over does not open (${theme}/${bg})`, (await log()).length === 0 && (await pg.locator('.wheel').count()) === 0);

    // rest on the sprite
    await pg.mouse.move(140, 140);
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
      check('all wheel items inside the 280x280 window', items.every((i) => i.left >= 0 && i.top >= 0 && i.right <= 280 && i.bottom <= 280));
      check('sprite centred at (140,140) while wheel open', Math.abs(sprite.x + sprite.width / 2 - 140) < 0.5 && Math.abs(sprite.y + sprite.height / 2 - 140) < 0.5, JSON.stringify(sprite));
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
      await pg.mouse.move(140, 140);
      await pg.waitForSelector('.wheel', { state: 'attached', timeout: 3000 }).catch(async (e) => { console.log('DEBUG phase', await pg.locator('[data-testid=float-expanded]').getAttribute('data-wheel'), JSON.stringify(await log())); throw e; });
      await pg.mouse.move(2, 2);
      await pg.waitForTimeout(150);
      check('wheel still open 150 ms after leaving', (await pg.locator('.wheel').count()) === 1);
      await pg.waitForTimeout(600);
      check('wheel closed after leaving', (await pg.locator('.wheel').count()) === 0);

      // Escape closes immediately
      await pg.mouse.move(140, 140);
      await pg.waitForSelector('.wheel', { state: 'attached' });
      await pg.keyboard.press('Escape');
      await pg.waitForTimeout(250);
      check('Escape closes the wheel', (await pg.locator('.wheel').count()) === 0);
      await pg.mouse.move(2, 2);
      await pg.waitForTimeout(400);

      // drag never opens it
      await pg.mouse.move(140, 140);
      await pg.mouse.down();
      await pg.mouse.move(150, 150, { steps: 4 });
      await pg.waitForTimeout(600);
      check('pressing/dragging the sprite never opens the wheel', (await pg.locator('.wheel').count()) === 0);
      await pg.mouse.up();
      await pg.mouse.move(2, 2);
      await pg.waitForTimeout(300);

      // DnD toggles in place
      await pg.mouse.move(140, 140);
      await pg.waitForSelector('.wheel', { state: 'attached' });
      const dnd = pg.locator('.wheel__item[data-item="dnd"]');
      const before = await dnd.getAttribute('aria-pressed');
      const b = await dnd.boundingBox();
      await pg.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
      await pg.mouse.click(b.x + b.width / 2, b.y + b.height / 2);
      await pg.waitForTimeout(500);
      await pg.mouse.move(140, 140);
      await pg.waitForSelector('.wheel', { state: 'attached', timeout: 3000 }).catch(async (e) => { console.log('DEBUG2 phase', await pg.locator('[data-testid=float-expanded]').getAttribute('data-wheel'), JSON.stringify(await log())); throw e; });
      const after = await pg.locator('.wheel__item[data-item="dnd"]').getAttribute('aria-pressed');
      check('勿擾 toggles in place', before !== after, `${before} -> ${after}`);
      await pg.waitForTimeout(200);
      await shot(pg, `wheel-dnd-on-${theme}-on-${bg}`);
    }
    await ctx.close();
  }

  // collapsed 96x96 window: sprite centred; docked hover undocks first
  const ctx = await browser.newContext({ deviceScaleFactor: 2, reducedMotion: 'reduce', viewport: { width: 96, height: 96 } });
  const pg = await ctx.newPage();
  watch(pg, 'wheel-dock');
  await pg.goto(`${BASE}/floating.html?theme=light&backdrop=light&dock=none`);
  await pg.waitForSelector('[data-testid="float-expanded"]');
  const sb = await pg.locator('.float__sprite').boundingBox();
  check('sprite centred in the collapsed 96x96 window', Math.abs(sb.x + sb.width / 2 - 48) < 0.5 && Math.abs(sb.y + sb.height / 2 - 48) < 0.5 && sb.width === 64, JSON.stringify(sb));
  await pg.goto(`${BASE}/floating.html?theme=light&backdrop=light&dock=left`);
  await pg.waitForSelector('[data-testid="float-collapsed"]');
  const db = await pg.locator('.dock').boundingBox();
  const strip = await pg.locator('.dock__strip').boundingBox();
  check('docked: visible strip 12x48, hit area 44 wide', strip.width === 12 && strip.height === 48 && db.width === 44, `strip ${strip.width}x${strip.height}, hit ${db.width}x${db.height}`);
  await pg.mouse.move(6, 48);
  await pg.waitForSelector('[data-testid="float-expanded"]', { timeout: 2000 });
  check('hovering a docked sprite undocks it (wheel needs a second rest)', (await pg.locator('.wheel').count()) === 0);
  await ctx.close();

  // live values: seeded mock with an approval waiting (second window drives the demo)
  const lctx = await browser.newContext({ deviceScaleFactor: 2, timezoneId: 'Asia/Taipei', locale: 'zh-TW', reducedMotion: 'reduce', viewport: { width: 280, height: 280 } });
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
  await f.mouse.move(140, 140);
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
      const size = wheel ? 280 : 96;
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
        async ({ a, b, size, wheel }) => {
          const load = (d) => new Promise((r) => { const i = new Image(); i.onload = () => r(i); i.src = 'data:image/png;base64,' + d; });
          const [A, B] = await Promise.all([load(a), load(b)]);
          const px = (img) => { const c = document.createElement('canvas'); c.width = img.width; c.height = img.height; const x = c.getContext('2d'); x.drawImage(img, 0, 0); return x.getImageData(0, 0, img.width, img.height); };
          const pa = px(A), pb = px(B);
          // allowed (painted) regions in page coords: sprite 64x64 centred; for the wheel, the item cards
          const cx = 20 + size / 2, cy = 20 + size / 2;
          const rects = [[cx - 44, cy - 44, 88, 88]]; // sprite + its drop-shadow/halo
          if (wheel) for (const [x, y] of [[0,-92],[87,-46],[87,46],[0,92],[-87,46],[-87,-46]]) rects.push([cx + x - 42 - 32, cy + y - 28 - 32, 84 + 64, 56 + 64]); // card + its 24px-blur drop shadow
          let diff = 0, total = 0;
          for (let y = 0; y < pa.height; y++) for (let x = 0; x < pa.width; x++) {
            if (rects.some(([rx, ry, rw, rh]) => x >= rx && x < rx + rw && y >= ry && y < ry + rh)) continue;
            total++;
            const i = (y * pa.width + x) * 4;
            if (Math.abs(pa.data[i] - pb.data[i]) + Math.abs(pa.data[i+1] - pb.data[i+1]) + Math.abs(pa.data[i+2] - pb.data[i+2]) > 6) diff++;
          }
          return { diff, total };
        },
        { a: bare.toString('base64'), b: withFrame.toString('base64'), size, wheel },
      );
      check(`transparent outside art (${theme}${wheel ? ', wheel open' : ''})`, verdict.diff === 0, `${verdict.diff} of ${verdict.total} outside pixels differ`);
      await ctx.close();
    }
  }
  // computed backgrounds before/after mount
  const ctx = await browser.newContext({ viewport: { width: 96, height: 96 } });
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
  await float.setViewportSize({ width: 96, height: 96 });
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
