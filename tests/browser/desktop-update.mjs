import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';

// 真实桌面构建页面只连接合成 Tauri 边界，不启动安装器或读取用户配置。
const root = path.resolve('dist/desktop');
const output = path.resolve(process.argv[2] || 'test-results/browser');
fs.mkdirSync(output, { recursive: true });
const errors = [], results = [];
const server = http.createServer((request, response) => {
  const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
  const file = path.resolve(root, '.' + (pathname === '/' ? '/index.html' : pathname));
  if (!file.startsWith(root + path.sep)) { response.writeHead(403); response.end(); return; }
  try {
    response.setHeader('content-type', ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png' })[path.extname(file)] || 'application/octet-stream');
    response.end(fs.readFileSync(file));
  } catch { response.writeHead(404); response.end(); }
});
const browser = await chromium.launch({ headless: true });
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = 'http://127.0.0.1:' + server.address().port;
const context = await browser.newContext({ viewport: { width: 1120, height: 820 }, reducedMotion: 'reduce' });
await context.route('**/*', route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
await context.addInitScript(() => {
  const status = {
    version: '0.3.28', service: { running: true, error: null },
    sync: { state: 'connected', message: '模拟已连接' }, runtime: { ready: true, message: '隔离测试' },
    relay: { state: 'disabled', url: '', name: '', hasToken: false, linkExpiresAt: null, phones: [] },
    desktopLaunch: { supported: true, enabled: false, message: '首次启用后重开一次 Codex，以后可按任意顺序打开。关闭仅撤销后续启动配置。' },
    config: { port: 9989, bind: '127.0.0.1', publicUrl: '', autoStart: false },
    update: { phase: 'idle', message: '准备检查' },
    updateSettings: { version: '0.3.28', mode: 'automatic' }, release: null,
  };
  const fixture = window.updateFixture = { status, calls: [], requests: [], resolve: null, reject: null, failure: '', startup: false, clipboard: '' };
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: async text => { fixture.clipboard = text; } } });
  window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
    fixture.calls.push(command);
    fixture.requests.push({ command, args });
    if (fixture.failure === command) { fixture.failure = ''; throw new Error('合成保存失败，请重试。'); }
    if (command === 'status') return structuredClone(status);
    if (command === 'get_autostart') return fixture.startup;
    if (command === 'set_autostart') return fixture.startup = args.enabled;
    if (command === 'set_desktop_launch') { status.desktopLaunch.enabled = args.enabled; return structuredClone(status); }
    if (command === 'configure_relay') {
      status.relay.url = args.url.trim().replace(/\/$/, '');
      status.relay.name = args.name.trim() || 'Codex 电脑';
      status.relay.state = status.relay.url ? 'unlinked' : 'disabled';
      return structuredClone(status);
    }
    if (command === 'save_config') { status.config = args.config; return structuredClone(status); }
    if (command === 'start_service' || command === 'stop_service') {
      status.service.running = command === 'start_service';
      return structuredClone(status);
    }
    if (command === 'retry_sync') { status.sync = { state: 'connecting', message: '正在连接…' }; return structuredClone(status); }
    if (command === 'link_relay_desktop') {
      status.service.running = true;
      status.relay.linkExpiresAt = Date.now() + 60_000;
      return { link: status.relay.url + '/?invite=synthetic-cloud-pairing', expiresAt: status.relay.linkExpiresAt };
    }
    if (command === 'check_update') {
      status.update = { phase: 'checking', stage: 'checking', message: '正在检查正式版本' };
      return new Promise((resolve, reject) => { fixture.resolve = resolve; fixture.reject = reject; });
    }
    throw new Error('未预期的桌面命令：' + command);
  } };
});
const page = await context.newPage();
page.setDefaultTimeout(10_000);
page.on('pageerror', error => errors.push(error.message));
try {
  await page.clock.install();
  await page.goto(origin);
  assert.equal(await page.locator('#open-codex, #open-codex-action, footer').count(), 0);
  assert.equal(await page.locator('#retry-sync').isVisible(), false);
  await page.clock.runFor(300);
  await page.screenshot({ path: path.join(output, 'desktop-simplified-home.png') });
  for (const syncState of ['waiting', 'history-only', 'error']) {
    await page.evaluate(syncState => { window.updateFixture.status.sync = { state: syncState, message: '合成连接状态' }; }, syncState);
    await page.clock.runFor(2500);
    assert.equal(await page.locator('#retry-sync').isVisible(), true);
  }
  await page.locator('#retry-sync').click();
  assert.equal(await page.locator('#retry-sync').isVisible(), false);
  await page.evaluate(() => { window.updateFixture.status.sync = { state: 'connected', message: '模拟已连接' }; });
  await page.clock.runFor(2500);
  assert.equal(await page.locator('#sync-state').innerText(), '已同步');
  assert.equal(await page.evaluate(() => window.updateFixture.calls.filter(value => value === 'retry_sync').length), 1);
  results.push({ id: 'HOST-009', scenario: 'one-recovery-action-only-when-disconnected', status: 'passed' });

  await page.locator('#open-settings').click();
  assert.equal(await page.locator('#relay-url').isVisible(), true);
  assert.equal(await page.locator('#service-settings').getAttribute('open'), null);
  assert.equal(await page.locator('#advanced-settings, #lan-settings, #copy-diagnostics, #key, #rotate-key').count(), 0);
  assert.doesNotMatch(await page.locator('#preferences-dialog').innerText(), /故障排查|局域网连接|高级设置/);
  assert.equal(await page.locator('#legacy-relay-token').isVisible(), false);
  await page.locator('#relay-url').fill('https://mobile.example.com/');
  await page.locator('#relay-name').fill('  测试电脑  ');
  await page.clock.runFor(2500);
  assert.equal(await page.locator('#relay-name').inputValue(), '  测试电脑  ', '轮询不覆盖配置草稿');
  await page.evaluate(() => { window.updateFixture.failure = 'configure_relay'; });
  await page.locator('#save-relay').click();
  assert.match(await page.locator('#notice').innerText(), /合成保存失败/);
  assert.equal(await page.locator('#relay-url').inputValue(), 'https://mobile.example.com/');
  assert.equal(await page.locator('#save-relay').isEnabled(), true);
  await page.locator('#save-relay').click();
  assert.equal(await page.locator('#relay-url').inputValue(), 'https://mobile.example.com');
  assert.equal(await page.locator('#relay-name').inputValue(), '测试电脑');
  await page.locator('#close-settings').click();
  await page.clock.runFor(100);
  await page.locator('#open-settings').click();
  assert.equal(await page.locator('#relay-url').inputValue(), 'https://mobile.example.com');
  await page.locator('#relay-url').fill('');
  await page.locator('#save-relay').click();
  assert.equal(await page.evaluate(() => window.updateFixture.status.relay.state), 'disabled');
  await page.locator('#close-settings').click();
  await page.clock.runFor(100);
  await page.locator('#link-phone').click();
  assert.equal(await page.locator('#preferences-dialog').isVisible(), true);
  assert.equal(await page.locator('#relay-url').evaluate(el => document.activeElement === el), true);
  assert.equal(await page.evaluate(() => window.updateFixture.calls.includes('link_relay_desktop')), false);
  results.push({ id: 'HOST-010', scenario: 'connection-config-draft-failure-retry-normalization-disable', status: 'passed' });

  await page.evaluate(() => { window.updateFixture.failure = 'set_autostart'; });
  await page.locator('#launch-on-login').click();
  assert.equal(await page.locator('#launch-on-login').isChecked(), false);
  await page.locator('#launch-on-login').check();
  assert.equal(await page.locator('#launch-on-login').isChecked(), true);
  assert.equal(await page.evaluate(() => window.updateFixture.status.config.autoStart), false, '系统自启不修改服务自启');
  await page.evaluate(() => { window.updateFixture.failure = 'set_desktop_launch'; });
  await page.locator('#desktop-launch').click();
  assert.equal(await page.locator('#desktop-launch').isChecked(), false);
  await page.locator('#desktop-launch').check();
  assert.equal(await page.locator('#desktop-launch').isChecked(), true);
  assert.match(await page.locator('#notice').innerText(), /首次启用/);
  await page.locator('#desktop-launch').uncheck();
  assert.equal(await page.locator('#desktop-launch').isChecked(), false);
  assert.equal(await page.evaluate(() => window.updateFixture.status.sync.state), 'connected');
  assert.match(await page.locator('#notice').innerText(), /当前连接和任务继续/);
  results.push({ id: 'HOST-011', scenario: 'persistent-launch-enable-disable-and-failure-restoration', status: 'passed' });
  await page.locator('#theme').selectOption('dark');
  assert.equal(await page.locator('html').getAttribute('data-theme'), 'dark');
  assert.equal(await page.evaluate(() => localStorage.getItem('codex-mobile-desktop-theme')), 'dark');
  await page.emulateMedia({ colorScheme: 'light' });
  assert.equal(await page.locator('html').getAttribute('data-theme'), 'dark');
  await page.clock.runFor(300);
  await page.screenshot({ path: path.join(output, 'desktop-settings-dark.png') });
  await page.locator('#theme').selectOption('system');
  assert.equal(await page.locator('html').getAttribute('data-theme'), 'light');
  assert.equal(await page.evaluate(() => localStorage.getItem('codex-mobile-desktop-theme')), null);
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
  await page.locator('#theme').selectOption('light');
  results.push({ id: 'HOST-010', scenario: 'startup-failure-reverts-theme-follows-user-choice', status: 'passed' });

  for (const viewport of [{ width: 1000, height: 840 }, { width: 720, height: 580 }, { width: 390, height: 844 }, { width: 844, height: 390 }]) {
    await page.setViewportSize(viewport);
    await page.locator('#relay-url').scrollIntoViewIfNeeded();
    assert.equal(await page.locator('#preferences-dialog').evaluate(el => el.scrollWidth <= el.clientWidth), true);
    assert.equal(await page.locator('.preferences-body').evaluate(el => el.scrollWidth <= el.clientWidth), true);
    await page.locator('#save-relay').scrollIntoViewIfNeeded();
    assert.equal(await page.locator('#save-relay').evaluate(el => {
      const rect = el.getBoundingClientRect();
      return rect.width >= 44 && rect.height >= 48 && document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2) === el;
    }), true);
    await page.locator('#desktop-launch').scrollIntoViewIfNeeded();
    assert.equal(await page.locator('#desktop-launch-settings').evaluate(el => el.scrollWidth <= el.clientWidth), true);
    await page.clock.runFor(300);
    await page.screenshot({ path: path.join(output, `desktop-settings-${viewport.width}x${viewport.height}.png`) });
  }
  results.push({ id: 'UI-007', scenario: 'desktop-settings-four-viewports-no-clipped-actions', status: 'passed' });
  await page.setViewportSize({ width: 1000, height: 840 });
  await page.locator('#service-settings > summary').click();
  await page.locator('#port').fill('5432');
  await page.locator('#bind').selectOption('0.0.0.0');
  await page.locator('#auto-start').check();
  await page.locator('#save-service').click();
  assert.deepEqual(await page.evaluate(() => window.updateFixture.status.config), { port: 5432, bind: '0.0.0.0', publicUrl: '', autoStart: true });
  await page.locator('#settings-service').click();
  assert.equal(await page.locator('#settings-service').innerText(), '启动服务');
  await page.locator('#settings-service').click();
  assert.equal(await page.locator('#settings-service').innerText(), '停止服务');
  await page.locator('#service-settings > summary').click();
  assert.deepEqual(await page.evaluate(() => window.updateFixture.calls.filter(command => ['pairing', 'rotate_key', 'diagnostics'].includes(command))), []);
  await page.locator('#close-settings').click();
  await page.clock.runFor(100);
  await page.waitForFunction(() => document.activeElement === document.querySelector('#open-settings'));
  assert.equal(await page.locator('#open-settings').evaluate(el => document.activeElement === el), true);
  await page.locator('#open-settings').click();
  results.push({ id: 'HOST-010', scenario: 'service-and-dialog-focus-without-lan-or-diagnostics-ui', status: 'passed' });

  await page.evaluate(() => { window.updateFixture.status.relay.hasToken = true; });
  await page.clock.runFor(2500);
  await page.locator('#legacy-relay-token > summary').click();
  await page.locator('#relay-url').fill('https://mobile.example.com');
  await page.locator('#relay-token').fill('synthetic-legacy-token');
  await page.locator('#save-relay').click();
  assert.equal(await page.locator('#relay-token').inputValue(), '');
  assert.equal(await page.evaluate(() => window.updateFixture.requests.filter(r => r.command === 'configure_relay').at(-1).args.token), 'synthetic-legacy-token');
  await page.evaluate(() => { window.updateFixture.status.relay.hasToken = false; });
  await page.clock.runFor(2500);
  results.push({ id: 'HOST-010', scenario: 'existing-legacy-key-remains-editable-and-clears-after-save', status: 'passed' });
  await page.locator('#close-settings').click();
  await page.clock.runFor(100);
  await page.locator('#link-phone').click();
  assert.equal(await page.locator('#relay-invite').isVisible(), true);
  assert.equal(await page.locator('#relay-qr').evaluate(el => el.width), 240);
  await page.locator('#copy-relay').click();
  assert.equal(await page.evaluate(() => window.updateFixture.clipboard), 'https://mobile.example.com/?invite=synthetic-cloud-pairing');
  assert.equal(await page.evaluate(() => window.updateFixture.calls.filter(command => command === 'link_relay_desktop').length), 1);
  await page.clock.runFor(65_000);
  await page.locator('#relay-invite').waitFor({ state: 'hidden' });
  assert.equal(await page.locator('#relay-invite').isVisible(), false);
  assert.equal(await page.locator('#copy-relay').isVisible(), false);
  assert.match(await page.locator('#notice').innerText(), /二维码已过期/);
  assert.deepEqual(await page.evaluate(() => window.updateFixture.calls.filter(command => ['pairing', 'rotate_key', 'diagnostics'].includes(command))), []);
  results.push({ id: 'HOST-011', scenario: 'cloud-qr-copy-and-expiry-survive-removing-lan-tools', status: 'passed' });
  await page.locator('#open-settings').click();
  const check = page.locator('#check-update');
  assert.equal(await page.locator('#update-settings, #token-form, #token-help').count(), 0);
  assert.doesNotMatch(await page.locator('#preferences-dialog').innerText(), /GitHub token|更新来源/);
  await page.clock.runFor(300);
  await page.screenshot({ path: path.join(output, 'desktop-public-update.png') });
  results.push({ id: 'UPDATE-011', scenario: 'public-update-without-source-or-token-settings', status: 'passed' });
  await check.click();
  assert.equal(await check.isDisabled(), true);
  await check.evaluate(element => element.click());
  for (const [stage, message] of [
    ['downloading', '正在下载安装包 · 1.0 / 2.0 MB'],
    ['verifying', '正在校验安装包'],
    ['extracting', '正在解压并检查运行文件'],
  ]) {
    await page.evaluate(({ stage, message }) => {
      window.updateFixture.status.update = { phase: 'checking', stage, message, downloadedBytes: 1048576, totalBytes: 2097152 };
    }, { stage, message });
    await page.clock.runFor(2500);
    await page.waitForFunction(message => document.querySelector('#update-status').textContent === message, message);
    assert.equal(await page.locator('#notice').innerText(), message);
    assert.equal(await check.isDisabled(), true);
    assert.equal(await page.locator('#apply-update').isVisible(), false);
  }
  assert.equal(await page.evaluate(() => window.updateFixture.calls.filter(value => value === 'check_update').length), 1);
  await page.clock.runFor(300);
  await page.screenshot({ path: path.join(output, 'desktop-update-extracting.png') });
  results.push({ id: 'UPDATE-002', scenario: 'manual-check-polls-while-busy', status: 'passed' });

  await page.evaluate(() => {
    const fixture = window.updateFixture;
    fixture.status.update = { phase: 'ready', version: '0.3.29', message: '更新已下载并通过校验。' };
    fixture.resolve(structuredClone(fixture.status));
  });
  await page.locator('#apply-update').waitFor({ state: 'visible' });
  assert.equal(await check.isEnabled(), true);
  assert.equal(await page.locator('#apply-update').innerText(), '安装 v0.3.29 并重启');
  results.push({ id: 'UPDATE-004', scenario: 'only-ready-exposes-install', status: 'passed' });

  await check.click();
  await page.evaluate(() => {
    const fixture = window.updateFixture;
    fixture.status.update = { phase: 'error', message: '合成安装包校验失败' };
    fixture.reject({ message: '合成安装包校验失败' });
  });
  await page.clock.runFor(2500);
  await page.waitForFunction(() => document.querySelector('#update-status').textContent === '合成安装包校验失败');
  assert.equal(await check.isEnabled(), true);
  assert.equal(await page.locator('#apply-update').isVisible(), false);
  await check.click();
  await page.evaluate(() => {
    const fixture = window.updateFixture;
    fixture.status.update = { phase: 'current', message: '当前已是最新正式版本。' };
    fixture.resolve(structuredClone(fixture.status));
  });
  await page.waitForFunction(() => document.querySelector('#check-update').disabled === false);
  assert.equal(await page.evaluate(() => window.updateFixture.calls.filter(value => value === 'check_update').length), 3);
  assert.equal(await page.locator('#update-status').innerText(), '当前已是最新正式版本。');
  results.push({ id: 'UPDATE-003', scenario: 'failure-releases-check-and-can-retry', status: 'passed' });
  await page.evaluate(() => { window.updateFixture.status.updateSettings.mode = 'paused'; window.updateFixture.status.update.message = ''; });
  await page.clock.runFor(2500);
  assert.equal(await check.isEnabled(), true);
  assert.equal(await page.locator('#update-status').innerText(), '自动升级已暂停，可手动检查更新。');
  await page.evaluate(() => { window.updateFixture.status.updateSettings.mode = 'source'; });
  await page.clock.runFor(2500);
  assert.equal(await check.isDisabled(), true);
  results.push({ id: 'UPDATE-003', scenario: 'paused-check-remains-available-source-check-disabled', status: 'passed' });

  assert.deepEqual(errors, [], '桌面更新页面不能产生未处理异常');
} catch (error) {
  results.push({ status: 'failed', error: error.stack || String(error) });
  throw error;
} finally {
  await page.clock.runFor(300);
  await page.screenshot({ path: path.join(output, 'desktop-update-result.png') });
  const calls = await page.evaluate(() => window.updateFixture?.calls || []);
  fs.writeFileSync(path.join(output, 'desktop-update-report.json'), JSON.stringify({ environment: 'Docker Chromium; synthetic Tauri invoke only', results, errors, calls }, null, 2));
  await browser.close();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
console.log(`Docker desktop update regression: ${results.length} passed; artifacts: ${output}`);
