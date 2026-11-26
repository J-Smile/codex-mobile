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
    config: { port: 9989, bind: '127.0.0.1', publicUrl: '', autoStart: false },
    update: { phase: 'idle', message: '准备检查' },
    updateSettings: { version: '0.3.28', mode: 'automatic' }, release: null,
  };
  const fixture = window.updateFixture = { status, calls: [], resolve: null, reject: null };
  window.__TAURI_INTERNALS__ = { invoke: async command => {
    fixture.calls.push(command);
    if (command === 'status') return structuredClone(status);
    if (command === 'pairing') return { accessKey: 'synthetic-local-pairing-only' };
    if (command === 'get_autostart') return false;
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
  await page.locator('#open-settings').click();
  const check = page.locator('#check-update');
  assert.equal(await page.locator('#update-settings, #token-form, #token-help').count(), 0);
  assert.doesNotMatch(await page.locator('#preferences-dialog').innerText(), /GitHub token|更新来源/);
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
  await page.screenshot({ path: path.join(output, 'desktop-update-result.png') });
  const calls = await page.evaluate(() => window.updateFixture?.calls || []);
  fs.writeFileSync(path.join(output, 'desktop-update-report.json'), JSON.stringify({ environment: 'Docker Chromium; synthetic Tauri invoke only', results, errors, calls }, null, 2));
  await browser.close();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
console.log(`Docker desktop update regression: ${results.length} passed; artifacts: ${output}`);
