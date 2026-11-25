import test from 'node:test';
import assert from 'node:assert/strict';
import { createRenderer, nextTick, ref } from 'vue';
import { useAppUpdate } from '../../mobile/src/useAppUpdate.ts';

const settle = async () => { await new Promise((resolve) => setImmediate(resolve)); await nextTick(); };

function fixture(t, { version = 'first', blocked = false } = {}) {
  const keys = ['document', 'window', 'navigator', 'fetch'];
  const original = new Map(keys.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const document = Object.assign(new EventTarget(), {
    hidden: false, activeElement: null,
    querySelector: (selector) => selector === 'meta[name="app-version"]' ? { content: 'first' } : null,
  });
  const calls = [];
  const registrations = [];
  let reloads = 0;
  let saves = 0;
  let latest = version;
  let saved = true;
  const window = Object.assign(new EventTarget(), { isSecureContext: true, location: { reload: () => reloads++ } });
  const navigator = { serviceWorker: { register: async (...args) => { registrations.push(args); return { update: async () => {} }; } } };
  const fetch = async (url, options) => { calls.push([url, options]); return Response.json({ version: latest }); };
  for (const [key, value] of Object.entries({ document, window, navigator, fetch })) Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
  const busy = ref(blocked);
  let update;
  const renderer = createRenderer({
    createComment: () => ({}), createElement: () => ({}), createText: () => ({}),
    insert() {}, remove() {}, setText() {}, setElementText() {}, patchProp() {}, parentNode: () => null, nextSibling: () => null,
  });
  const app = renderer.createApp({ setup() {
    update = useAppUpdate({ enabled: true, canReload: () => !busy.value, beforeReload: () => { saves++; return saved; } });
    return () => null;
  } });
  app.mount({});
  t.after(() => {
    app.unmount();
    for (const [key, descriptor] of original) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  return { app, update, document, window, calls, registrations, busy, reloads: () => reloads, saves: () => saves,
    publish: (value) => { latest = value; }, setSaved: (value) => { saved = value; } };
}

test('同版本不刷新，回前台绕过缓存检查新构建并保留状态后只刷新一次', async (t) => {
  const f = fixture(t);
  await settle();
  assert.equal(f.reloads(), 0);
  assert.deepEqual(f.registrations[0], ['/sw.js', { updateViaCache: 'none' }]);
  assert.equal(f.calls[0][1].cache, 'no-store');
  f.document.hidden = true;
  f.publish('second');
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.equal(f.calls.length, 1);
  f.document.hidden = false;
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.equal(f.reloads(), 1);
  assert.equal(f.saves(), 1);
  f.window.dispatchEvent(new Event('focus'));
  await settle();
  assert.equal(f.reloads(), 1);
});

test('发送或录音期间延后更新，结束后保存草稿再刷新', async (t) => {
  const f = fixture(t, { version: 'second', blocked: true });
  await settle();
  assert.ok(f.update.available.value);
  assert.equal(f.reloads(), 0);
  f.busy.value = false;
  await settle();
  assert.equal(f.saves(), 1);
  assert.equal(f.reloads(), 1);
});

test('输入聚焦时保留页面，退出输入后更新；草稿保存失败不丢弃页面', async (t) => {
  const f = fixture(t, { version: 'second' });
  f.document.activeElement = { matches: () => true };
  await settle();
  assert.equal(f.reloads(), 0);
  f.setSaved(false);
  f.document.activeElement = null;
  f.update.reload();
  assert.equal(f.reloads(), 0);
  assert.match(f.update.error.value, /草稿/);
  f.setSaved(true);
  f.update.reload();
  assert.equal(f.reloads(), 1);
});

test('纯文本可编辑区聚焦期间不刷新，失焦后保存草稿并更新', async (t) => {
  const f = fixture(t, { version: 'second' });
  f.document.activeElement = { matches: () => false, isContentEditable: true };
  await settle();
  assert.equal(f.reloads(), 0);
  assert.equal(f.saves(), 0);
  f.document.activeElement = null;
  f.document.dispatchEvent(new Event('focusout'));
  await new Promise(resolve => setTimeout(resolve, 5));
  await settle();
  assert.equal(f.saves(), 1);
  assert.equal(f.reloads(), 1);
});

test('离线或无效版本不刷新，恢复网络后可重试，重复检查合并', async (t) => {
  const f = fixture(t);
  await settle();
  globalThis.fetch = async () => { throw new TypeError('网络不可用'); };
  await f.update.check();
  assert.equal(f.update.error.value, '');
  assert.equal(f.reloads(), 0);
  globalThis.fetch = async () => Response.json({});
  await f.update.check();
  assert.equal(f.reloads(), 0);
  let finish;
  let requests = 0;
  globalThis.fetch = async () => { requests++; return await new Promise((resolve) => { finish = resolve; }); };
  const first = f.update.check();
  void f.update.check();
  assert.equal(requests, 1);
  finish(Response.json({ version: 'second' }));
  await first;
  assert.equal(f.reloads(), 1);
});

test('销毁时取消更新请求，迟到响应不能刷新已关闭的页面', async (t) => {
  const f = fixture(t);
  await settle();
  let finish;
  let signal;
  globalThis.fetch = (_, options) => { signal = options.signal; return new Promise((resolve) => { finish = resolve; }); };
  const checking = f.update.check();
  f.app.unmount();
  assert.ok(signal.aborted);
  finish(Response.json({ version: 'second' }));
  await checking;
  assert.equal(f.reloads(), 0);
});
