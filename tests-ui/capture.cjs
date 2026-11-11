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
  await float.setViewportSize({ width: 300, height: 110 });
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
  await f2.setViewportSize({ width: 300, height: 110 });
  for (const s of states) {
    await f2.goto(`${BASE}/floating.html?theme=${theme}&backdrop=1&state=${s}&dock=none`);
    await f2.waitForSelector('[data-testid="float-expanded"]');
    await f2.waitForTimeout(450);
    await shot(f2, `floating-${s}-${theme}`);
  }
  await f2.setViewportSize({ width: 120, height: 110 });
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

(async () => {
  const browser = await chromium.launch();
  for (const theme of ['light', 'dark']) await runTheme(browser, theme);
  await checkResync(browser);
  await browser.close();
  console.log(problems.length ? `PROBLEMS (${problems.length}):\n` + problems.join('\n') : 'No console errors or page errors.');
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
