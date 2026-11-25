import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { shellAssets } from '../../mobile/vite.config.ts';

function fixture() {
  const listeners = new Map();
  const entries = new Map();
  const deleted = [];
  const precached = [];
  let online = true;
  let activated = false;
  let cacheReady = Promise.resolve();
  const windows = [], opened = [], posted = [];
  const origin = 'https://mobile.example';
  const key = (request) => typeof request === 'string' ? request : request.url;
  const cache = {
    addAll: async (paths) => { precached.push(...paths); await cacheReady; },
    put: async (request, response) => entries.set(key(request), response),
    match: async (request) => entries.get(key(request)),
  };
  const self = { location: { origin }, addEventListener: (name, handler) => listeners.set(name, handler),
    skipWaiting: async () => { activated = true; }, clients: { claim: async () => {}, matchAll: async () => windows,
      openWindow: async url => { opened.push(url); return { postMessage: (message, ports) => { posted.push(message); ports[0].postMessage(true); } }; } } };
  const caches = { open: async () => cache, keys: async () => ['codex-mobile-shell-v2', 'codex-mobile-shell-new', 'other-app'],
    delete: async (name) => { deleted.push(name); return true; } };
  const requests = [];
  const fetch = async (request, options) => {
    requests.push([request, options]);
    if (!online) throw new TypeError('离线');
    return new Response('新页面');
  };
  const source = fs.readFileSync(new URL('../../public/sw.js', import.meta.url), 'utf8')
    .replace('__BUILD_VERSION__', 'new').replace('__BUILD_ASSETS__', JSON.stringify(['/assets/app-new.js', '/assets/app-new.css']));
  vm.runInNewContext(source, { self, caches, fetch, URL, Response, MessageChannel, setTimeout, clearTimeout });
  async function event(name, url, mode = 'cors') {
    const pending = [];
    let response;
    listeners.get(name)({ request: { url, method: 'GET', mode },
      waitUntil: (promise) => pending.push(promise), respondWith: (promise) => { response = promise; } });
    const result = await response;
    await Promise.all(pending);
    return result;
  }
  async function click(threadId) {
    const pending = [];
    let closed = false;
    listeners.get('notificationclick')({ notification: { data: { threadId }, close: () => { closed = true; } }, waitUntil: promise => pending.push(promise) });
    await Promise.all(pending);
    assert.equal(closed, true);
  }
  return { event, entries, deleted, requests, precached, windows, opened, posted, click, activated: () => activated,
    offline: () => { online = false; }, holdCache: (promise) => { cacheReady = promise; } };
}

test('新页面和脚本缓存完成后才激活，清理旧版本但保留其他应用缓存', async () => {
  const f = fixture();
  let complete;
  f.holdCache(new Promise((resolve) => { complete = resolve; }));
  const installation = f.event('install');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(f.activated(), false);
  assert.ok(f.precached.includes('/assets/app-new.js'));
  assert.ok(f.precached.includes('/assets/app-new.css'));
  complete();
  await installation;
  assert.equal(f.activated(), true);
  await f.event('activate');
  assert.deepEqual(f.deleted, ['codex-mobile-shell-v2']);
});

test('已有手机窗口时直接发送通知目标并聚焦，保留页面与草稿', async () => {
  const f = fixture(), messages = [];
  let focused = 0;
  f.windows.push({ url: 'https://elsewhere.example/', postMessage: () => assert.fail('其他来源不能接收') },
    { url: 'https://mobile.example/?thread=previous', postMessage: (message, ports) => { messages.push(message); ports[0].postMessage(true); }, async focus() { focused++; return this; }, navigate: () => assert.fail('不刷新已有对话') });
  await f.click('target-chat');
  assert.deepEqual(JSON.parse(JSON.stringify(messages)), [{ type: 'codex-open-thread', threadId: 'target-chat' }]);
  assert.equal(focused, 1);
  assert.deepEqual(f.opened, []);
});

test('升级后旧页面无法接收定位消息时导航到通知对话', async () => {
  const f = fixture(), navigated = [];
  let focused = false;
  f.windows.push({ url: 'https://mobile.example/?thread=previous',
    async focus() { focused = true; return this; },
    postMessage() { assert.equal(focused, true); },
    async navigate(url) { navigated.push(url); return this; } });
  await f.click('target-after-update');
  assert.equal(new URL(navigated[0]).searchParams.get('thread'), 'target-after-update');
});

test('没有手机窗口时带对话参数启动，并向可能被复用的窗口发送定位', async () => {
  const f = fixture();
  f.windows.push({ url: 'https://mobile.example/pair', focus: () => assert.fail('不能进入配对管理页') });
  await f.click('target /对话');
  assert.equal(new URL(f.opened[0]).searchParams.get('thread'), 'target /对话');
  assert.deepEqual(JSON.parse(JSON.stringify(f.posted)), [{ type: 'codex-open-thread', threadId: 'target /对话' }]);
});

test('测试通知只打开应用，不覆盖当前对话', async () => {
  const f = fixture();
  let focused = false;
  f.windows.push({ url: 'https://mobile.example/?thread=previous', focus: async () => { focused = true; }, postMessage: () => assert.fail('测试通知没有对话目标') });
  await f.click(undefined);
  assert.equal(focused, true);
  assert.deepEqual(f.opened, []);
});

test('版本、聊天和附件不进入离线缓存，导航重新验证并支持带聊天参数的离线入口', async () => {
  const f = fixture();
  assert.equal(await f.event('fetch', 'https://mobile.example/version.json'), undefined);
  assert.equal(await f.event('fetch', 'https://mobile.example/api/threads/chat/history'), undefined);
  assert.equal(await f.event('fetch', 'https://mobile.example/relay/account'), undefined);
  assert.equal(await f.event('fetch', 'https://mobile.example/api/threads/chat/attachment?path=image'), undefined);
  assert.equal(await f.event('fetch', 'https://mobile.example/pair', 'navigate'), undefined);
  assert.equal(await f.event('fetch', 'https://mobile.example/pair/', 'navigate'), undefined);
  assert.equal(f.requests.length, 0);
  const page = await f.event('fetch', 'https://mobile.example/?thread=chat', 'navigate');
  assert.equal(await page.text(), '新页面');
  assert.equal(f.requests[0][1].cache, 'no-cache');
  f.entries.set('/', new Response('离线页面'));
  f.offline();
  const offline = await f.event('fetch', 'https://mobile.example/?thread=other', 'navigate');
  assert.equal(await offline.text(), '离线页面');
  assert.equal(f.entries.has('https://mobile.example/version.json'), false);
});

test('更新只预缓存启动图和相应样式，不预取 App 的可选功能', () => {
  const chunk = (fileName, imports = [], dynamicImports = [], css = []) => ({ type: 'chunk', fileName, imports, dynamicImports, viteMetadata: { importedCss: new Set(css) } });
  const entry = { ...chunk('main.js', [], ['app.js', 'vue.js'], ['page.css']), isEntry: true };
  const chunks = [entry, chunk('app.js', ['vue.js'], ['scanner.js'], ['app.css']), chunk('vue.js'), chunk('scanner.js', [], [], ['scanner.css'])];
  assert.deepEqual(shellAssets(Object.fromEntries(chunks.map(c => [c.fileName, c]))).sort(), ['/app.css', '/app.js', '/main.js', '/page.css', '/vue.js']);
});

test('带哈希的按需资源只请求一次，已用过的资源离线仍可读取', async () => {
  const f = fixture();
  const url = 'https://mobile.example/assets/scan-fixture.js';
  await f.event('fetch', url);
  await f.event('fetch', url);
  f.offline();
  assert.equal(await (await f.event('fetch', url)).text(), '新页面');
  assert.equal(f.requests.length, 1);
});
