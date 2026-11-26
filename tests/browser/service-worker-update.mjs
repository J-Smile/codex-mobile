import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { chromium } from 'playwright-core';

// 使用正式构建的 Worker 与真实 Chromium；两个合成版本不接触实际聊天。
const output = path.resolve(process.argv[2] || 'test-results/browser');
fs.mkdirSync(output, { recursive: true });
const workerSource = fs.readFileSync('dist/mobile/sw.js', 'utf8');
// 旧版本夹具逐字取自公开发布提交，保留完整 Worker 协议；仅构建常量在合成服务中替换。
const legacySource = fs.readFileSync(new URL('./fixtures/service-worker-v0.3.28-e8484f4.js', import.meta.url));
const legacyProvenance = {
  version: 'v0.3.28', commit: 'e8484f4ec2d565e0b4811448b62bb62ea4f23d51', path: 'shared/public/sw.js',
  gitBlob: 'bd8b892b5b726ef02b93f9c1dffbd44b8a779aa4',
  sha256: '587d78e0ea2fc3674a9b6f82e9a718bc35e0d8af4b01f9be94017e41e4dd2170',
};
assert.equal(createHash('sha256').update(legacySource).digest('hex'), legacyProvenance.sha256, '旧版Worker夹具必须与固定发布提交一致');
let version = 'first';
let networkUnavailable = false;
const blockedRequests = [];
const assetRequests = [];
const shellCache = build => 'codex-mobile-shell-' + (build === 'legacy-v0.3.28' ? '' : 'v2-') + build;
const html = () => `<!doctype html><html><head><meta charset="utf-8"></head><body>
  <textarea aria-label="草稿"></textarea><p>合成版本 ${version}</p>
  <script type="module" src="/assets/main-${version}.js"></script></body></html>`;
const server = http.createServer((request, response) => {
  const url = new URL(request.url, 'http://127.0.0.1');
  if (url.pathname.startsWith('/assets/')) assetRequests.push({ version, path: url.pathname });
  if (networkUnavailable) {
    blockedRequests.push({ version, path: url.pathname });
    request.socket.destroy();
    return;
  }
  response.setHeader('cache-control', 'no-store');
  if (url.pathname === '/sw.js') {
    response.setHeader('content-type', 'text/javascript');
    return response.end((version === 'legacy-v0.3.28' ? legacySource.toString('utf8') : workerSource)
      .replace(/const buildVersion = [^;]+;/, `const buildVersion = '${version}';`)
      .replace(/const cacheName = [^;]+;/, `const cacheName = '${shellCache(version)}';`)
      .replace(/const relayWeb = [^;]+;/, 'const relayWeb = false;')
      .replace(/const buildAssets = [^;]+;/, `const buildAssets = ${JSON.stringify([`/assets/main-${version}.js`, `/assets/lazy-${version}.js`])};`));
  }
  if (url.pathname === `/assets/main-${version}.js`) {
    response.setHeader('content-type', 'text/javascript');
    return response.end(`window.buildVersion = '${version}'; window.loadFeature = () => import('/assets/lazy-${version}.js');`);
  }
  if (url.pathname === `/assets/lazy-${version}.js`) {
    response.setHeader('content-type', 'text/javascript');
    return response.end(`export const version = '${version}';`);
  }
  if (url.pathname.startsWith('/assets/')) { response.writeHead(404); return response.end('已替换的合成旧资源'); }
  if (url.pathname.startsWith('/api/')) {
    response.setHeader('content-type', 'application/json');
    return response.end(JSON.stringify({ synthetic: true }));
  }
  response.setHeader('content-type', url.pathname === '/startup.js' ? 'text/javascript' : 'text/html');
  response.end(url.pathname === '/' ? html() : '');
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const context = await browser.newContext({ serviceWorkers: 'allow', viewport: { width: 390, height: 844 } });
const report = { passed: [], errors: [] };

async function verifyLegacyMigration() {
  const legacyContext = await browser.newContext({ serviceWorkers: 'allow', viewport: { width: 390, height: 844 } });
  const evidence = { source: legacyProvenance, stages: [] };
  const legacyCache = shellCache('legacy-v0.3.28');
  const currentCache = shellCache('current-migration');
  async function record(page, stage) {
    const data = await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration();
      const names = await caches.keys();
      return { active: registration.active?.state, waiting: registration.waiting?.state,
        build: window.buildVersion, draft: document.querySelector('textarea').value,
        pending: localStorage.getItem('codex-mobile-pending-v2'),
        caches: Object.fromEntries(await Promise.all(names.map(async name => [name,
          (await (await caches.open(name)).keys()).map(request => new URL(request.url).pathname)]))) };
    });
    evidence.stages.push({ stage, ...data });
    fs.writeFileSync(path.join(output, 'service-worker-legacy-migration.json'), JSON.stringify(evidence, null, 2));
    return data;
  }
  try {
    version = 'legacy-v0.3.28';
    const page = await legacyContext.newPage();
    page.on('pageerror', error => report.errors.push(error.message));
    await page.goto(origin);
    await page.waitForFunction(() => window.buildVersion === 'legacy-v0.3.28');
    await page.evaluate(async () => {
      await navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' });
      await navigator.serviceWorker.ready;
    });
    await page.waitForFunction(() => !!navigator.serviceWorker.controller);
    await page.getByRole('textbox', { name: '草稿' }).fill('旧版首次升级仍保留草稿');
    await page.evaluate(() => localStorage.setItem('codex-mobile-pending-v2', JSON.stringify([
      ['legacy-fixture-chat', { requestId: 'legacy-operation-0001' }],
    ])));
    const initial = await record(page, 'legacy-active');
    assert.deepEqual(Object.keys(initial.caches), [legacyCache]);

    version = 'current-migration';
    const nextWorker = legacyContext.waitForEvent('serviceworker');
    const installation = await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration();
      const finished = new Promise((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('旧版首次升级安装超时')), 10000);
        registration.addEventListener('updatefound', () => {
          const worker = registration.installing;
          worker.addEventListener('statechange', () => {
            if (['installed', 'activated', 'redundant'].includes(worker.state)) {
              clearTimeout(timeout);
              resolve(worker.state);
            }
          });
        }, { once: true });
      });
      await registration.update();
      return finished;
    });
    const installed = await nextWorker;
    const legacyRegistration = await page.evaluateHandle(() => navigator.serviceWorker.getRegistration());
    await page.waitForFunction(registration => !registration.installing, legacyRegistration);
    evidence.installation = installation;
    const before = await record(page, 'current-installed-legacy-still-active');
    assert.equal(before.active, 'activated');
    assert.equal(before.waiting, 'installed', '首次迁移同样必须等待旧页面退出');
    assert.ok(before.caches[legacyCache]);
    assert.ok(before.caches[currentCache].includes('/assets/lazy-current-migration.js'));
    const installedAssetRequests = assetRequests.length;
    assert.equal(await page.evaluate(async () => (await window.loadFeature()).version), 'legacy-v0.3.28', '旧页面按原活动Worker读取旧哈希缓存');
    assert.equal(await page.getByRole('textbox', { name: '草稿' }).inputValue(), initial.draft);
    assert.equal(await page.evaluate(() => localStorage.getItem('codex-mobile-pending-v2')), initial.pending);
    report.passed.push('v0.3.28首次升级等待旧页面退出，旧哈希资源、草稿和未决身份保留');

    const newer = await legacyContext.newPage();
    newer.on('pageerror', error => report.errors.push(error.message));
    await newer.goto(origin);
    await newer.waitForFunction(() => window.buildVersion === 'current-migration');
    const beforeOffline = await record(newer, 'new-page-before-offline');
    assert.equal(beforeOffline.waiting, 'installed');
    assert.ok(beforeOffline.caches[currentCache].includes('/assets/lazy-current-migration.js'));
    assert.ok(!assetRequests.slice(installedAssetRequests).some(request => request.path === '/assets/lazy-current-migration.js'),
      '安装结束到首次离线读取前不能先从网络取得新延迟模块来伪造迁移通过');
    networkUnavailable = true;
    await legacyContext.setOffline(true);
    const lazy = await newer.evaluate(async () => {
      try { return { version: (await window.loadFeature()).version }; }
      catch (error) { return { error: error.message }; }
    });
    evidence.offlineLazy = lazy;
    evidence.blockedRequests = blockedRequests.filter(request => request.version === 'current-migration');
    await record(newer, 'new-page-offline-first-lazy');
    await newer.screenshot({ path: path.join(output, 'service-worker-legacy-new-page.png') });
    assert.equal(lazy.error, undefined, 'v0.3.28活动Worker下，新页面首次离线读取已预缓存的新延迟模块应成功');
    assert.equal(lazy.version, 'current-migration');
    networkUnavailable = false;
    await legacyContext.setOffline(false);
    await newer.evaluate(() => fetch('/api/threads').then(response => response.json()));
    assert.equal(await newer.evaluate(async () => !!await caches.match('/api/threads')), false);
    assert.equal(await newer.evaluate(() => localStorage.getItem('codex-mobile-pending-v2')), initial.pending);
    report.passed.push('v0.3.28首次迁移的新页面离线延迟加载、API不缓存和身份隔离通过');
    await page.close();
    await newer.close();
    const deadline = Date.now() + 10000;
    let remaining;
    do {
      remaining = await installed.evaluate(() => caches.keys());
      if (!remaining.includes(legacyCache)) break;
      await new Promise(resolve => setTimeout(resolve, 50));
    } while (Date.now() < deadline);
    assert.deepEqual(remaining, [currentCache]);
    report.passed.push('v0.3.28旧页面全部退出后当前Worker激活并清理旧缓存');

    // 迁移结束后再升级一次，旧布局不应重建，现代缓存也不应重复装入新资源。
    const migrated = await legacyContext.newPage();
    await migrated.goto(origin);
    const migratedRegistration = await migrated.evaluateHandle(() => navigator.serviceWorker.getRegistration());
    await migrated.waitForFunction(registration => {
      return !!navigator.serviceWorker.controller && registration.active?.state === 'activated'
        && !registration.waiting && !registration.installing;
    }, migratedRegistration);
    version = 'after-migration';
    await migrated.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration();
      const installed = new Promise((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('迁移后的升级安装超时')), 10000);
        registration.addEventListener('updatefound', () => {
          const worker = registration.installing;
          worker.addEventListener('statechange', () => {
            if (worker.state === 'redundant') { clearTimeout(timeout); reject(new Error('迁移后的升级安装失败')); }
            if (worker.state === 'installed') { clearTimeout(timeout); resolve(); }
          });
        }, { once: true });
      });
      await registration.update();
      await installed;
    });
    const afterMigration = await record(migrated, 'modern-upgrade-after-migration');
    assert.equal(afterMigration.waiting, 'installed');
    assert.equal(afterMigration.caches[legacyCache], undefined);
    assert.ok(afterMigration.caches[shellCache(version)].includes('/assets/lazy-after-migration.js'));
    assert.ok(!afterMigration.caches[currentCache].includes('/assets/lazy-after-migration.js'));
    report.passed.push('首次迁移结束后旧布局不再创建，后续现代版本之间不复制资源');
  } catch (error) {
    report.errors.push(error.message);
    throw error;
  } finally {
    networkUnavailable = false;
    await legacyContext.close();
  }
}

try {
  const page = await context.newPage();
  page.on('pageerror', error => report.errors.push(error.message));
  await page.goto(origin);
  await page.waitForFunction(() => window.buildVersion === 'first');
  await page.evaluate(async () => {
    await navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' });
    await navigator.serviceWorker.ready;
  });
  await page.waitForFunction(() => !!navigator.serviceWorker.controller);
  await page.getByRole('textbox', { name: '草稿' }).fill('保留的合成草稿');
  await page.evaluate(() => localStorage.setItem('codex-mobile-pending-v2', JSON.stringify([['fixture-chat', { requestId: 'saved-operation-0001' }]])));
  version = 'second';
  const nextWorker = context.waitForEvent('serviceworker');
  const installation = await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.getRegistration();
    const finished = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('合成升级安装超时')), 10000);
      registration.addEventListener('updatefound', () => {
        const worker = registration.installing;
        worker.addEventListener('statechange', () => {
          if (['installed', 'activated', 'redundant'].includes(worker.state)) {
            clearTimeout(timeout);
            resolve(worker.state);
          }
        });
      }, { once: true });
    });
    await registration.update();
    return finished;
  });
  const installed = await nextWorker;
  const currentRegistration = await page.evaluateHandle(() => navigator.serviceWorker.getRegistration());
  await page.waitForFunction(registration => !registration.installing, currentRegistration);
  const before = await page.evaluate(async () => ({
    waiting: !!(await navigator.serviceWorker.getRegistration()).waiting,
    caches: await caches.keys(), pending: localStorage.getItem('codex-mobile-pending-v2'),
  }));
  fs.writeFileSync(path.join(output, 'service-worker-before-reload.json'), JSON.stringify({ installation, ...before }, null, 2));
  assert.equal(before.waiting, true, '旧页面尚有草稿时，新 Worker 必须等待，不可提前接管并删除旧缓存');
  assert.ok(before.caches.includes(shellCache('first')));
  assert.equal(await page.evaluate(async () => !!await (await caches.open('codex-mobile-shell-v2-first')).match('/assets/lazy-second.js')), false);
  report.passed.push('现代版本不向旧现代缓存复制新资源，待激活缓存独立持有新构建');
  assert.equal(await page.evaluate(async () => (await window.loadFeature()).version), 'first');
  assert.equal(await page.getByRole('textbox', { name: '草稿' }).inputValue(), '保留的合成草稿');
  report.passed.push('旧页面保持控制与延迟加载，草稿及未决操作身份不变');

  const newer = await context.newPage();
  newer.on('pageerror', error => report.errors.push(error.message));
  await newer.goto(origin);
  await newer.waitForFunction(() => window.buildVersion === 'second');
  networkUnavailable = true;
  await context.setOffline(true);
  assert.equal(await newer.evaluate(async () => (await window.loadFeature()).version), 'second');
  networkUnavailable = false;
  await context.setOffline(false);
  assert.equal(await newer.evaluate(() => localStorage.getItem('codex-mobile-pending-v2')), before.pending);
  report.passed.push('同源新页面在旧 Worker 下读取已预缓存的新资源，离线延迟加载可用');
  await newer.evaluate(() => fetch('/api/threads').then(r => r.json()));
  assert.equal(await newer.evaluate(async () => !!await caches.match('/api/threads')), false);
  report.passed.push('聊天 API 不进入新旧离线缓存');
  await page.screenshot({ path: path.join(output, 'service-worker-old-page.png') });
  await page.close();
  assert.equal(await newer.evaluate(() => caches.has('codex-mobile-shell-v2-first')), true, '另一个受控页面仍打开时不能提前清理');
  assert.equal(await newer.evaluate(async () => !!(await navigator.serviceWorker.getRegistration()).waiting), true);
  await newer.close();
  await installed.evaluate(async () => {
    if (self.registration.active?.state === 'activated') return;
    await new Promise(resolve => self.addEventListener('activate', event => event.waitUntil(Promise.resolve().then(resolve)), { once: true }));
  });
  // activate 的缓存清理属于 waitUntil，等待它完成后再核对。
  const deadline = Date.now() + 10000;
  let remaining;
  do {
    remaining = await installed.evaluate(() => caches.keys());
    if (!remaining.includes(shellCache('first'))) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  } while (Date.now() < deadline);
  assert.deepEqual(remaining, [shellCache('second')]);
  report.passed.push('旧页面全部退出后激活新 Worker 并清理旧缓存');
  await verifyLegacyMigration();
  assert.deepEqual(report.errors, []);
} finally {
  fs.writeFileSync(path.join(output, 'service-worker-update-report.json'), JSON.stringify(report, null, 2));
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
console.log(JSON.stringify(report));
