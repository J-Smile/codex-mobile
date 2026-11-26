import test from 'node:test';
import assert from 'node:assert/strict';
import { createRenderer, nextTick, ref } from 'vue';
import { useAppUpdate } from '../../apps/mobile/src/useAppUpdate.ts';

const settle = async () => { await new Promise((resolve) => setImmediate(resolve)); await nextTick(); };

function fixture(t, { version = 'first', blocked = false, installing = false, registrationPending = false, secure = true } = {}) {
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
  const window = Object.assign(new EventTarget(), { isSecureContext: secure, location: { reload: () => reloads++ } });
  function worker(version, state) {
    return Object.assign(new EventTarget(), { state, version,
      postMessage(message, [port]) {
        assert.deepEqual(message, { type: 'codex-mobile-build' });
        port.postMessage({ version: this.version });
        port.close();
      },
    });
  }
  const registration = Object.assign(new EventTarget(), { active: worker('first', 'activated'), waiting: null, installing: null,
    update: async () => {
      if (registration.waiting?.version === latest || registration.installing || latest === 'first') return;
      registration.installing = worker(latest, 'installing');
      registration.dispatchEvent(new Event('updatefound'));
      if (!installing) complete();
    },
  });
  function complete(state = 'installed') {
    const candidate = registration.installing;
    registration.installing = null;
    registration.waiting = state === 'installed' ? candidate : null;
    candidate.state = state;
    candidate.dispatchEvent(new Event('statechange'));
  }
  let finishRegistration;
  const registrationReady = registrationPending ? new Promise(resolve => { finishRegistration = () => resolve(registration); }) : Promise.resolve(registration);
  const navigator = { serviceWorker: { register: (...args) => { registrations.push(args); return registrationReady; } } };
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
    registration, complete, finishRegistration,
    publish: (value) => { latest = value; }, setSaved: (value) => { saved = value; } };
}

test('注册和新资源安装完成之前不能保存或刷新，版本确认后才允许切换', async (t) => {
  const f = fixture(t, { version: 'second', registrationPending: true, installing: true });
  await settle();
  f.update.reload();
  assert.equal(f.reloads(), 0);
  assert.equal(f.saves(), 0);
  f.finishRegistration();
  await settle();
  assert.equal(f.registration.installing.state, 'installing');
  f.update.reload();
  assert.equal(f.reloads(), 0);
  f.complete();
  await settle();
  assert.equal(f.reloads(), 1);
  assert.equal(f.saves(), 1);
});

test('安装失败保留旧页面，重复事件不能绕过失败，后续检查可以重试', async (t) => {
  const f = fixture(t, { version: 'second', installing: true });
  await settle();
  f.complete('redundant');
  await settle();
  f.update.reload();
  f.document.dispatchEvent(new Event('focusout'));
  await new Promise(resolve => setTimeout(resolve, 5));
  assert.equal(f.reloads(), 0);
  assert.equal(f.saves(), 0);
  const checking = f.update.check();
  await settle();
  f.complete();
  await checking;
  assert.equal(f.reloads(), 1);
});

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
  f.publish('second');
  finish(Response.json({ version: 'second' }));
  await first;
  assert.equal(f.reloads(), 1);
});

test('版本不匹配不刷新，后台取消后迟到安装不能刷新，回前台重新核对', async (t) => {
  const f = fixture(t, { version: 'second', installing: true });
  await settle();
  f.registration.installing.version = 'outdated';
  f.complete();
  await settle();
  assert.equal(f.reloads(), 0);
  f.document.hidden = true;
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  f.registration.waiting.version = 'second';
  f.update.reload();
  assert.equal(f.reloads(), 0);
  f.document.hidden = false;
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.equal(f.reloads(), 1);
});

test('等待安装时销毁会撤销监听，安装完成不能保存或刷新', async (t) => {
  const f = fixture(t, { version: 'second', installing: true });
  await settle();
  f.app.unmount();
  f.complete();
  await settle();
  assert.equal(f.reloads(), 0);
  assert.equal(f.saves(), 0);
});

test('安装超时结束本轮等待，旧请求完成不会刷新，下一轮可使用已安装资源', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture(t, { version: 'second', installing: true });
  await settle();
  t.mock.timers.tick(30_000);
  await settle();
  f.complete();
  await settle();
  f.update.reload();
  assert.equal(f.reloads(), 0);
  await f.update.check();
  assert.equal(f.reloads(), 1);
  assert.equal(f.saves(), 1);
});

test('Worker 更新请求失败仍保留页面，恢复后允许重试', async (t) => {
  const f = fixture(t);
  await settle();
  const updateWorker = f.registration.update;
  f.publish('second');
  f.registration.update = async () => { throw new TypeError('下载 Worker 失败'); };
  await f.update.check();
  f.update.reload();
  assert.equal(f.reloads(), 0);
  f.registration.update = updateWorker;
  await f.update.check();
  assert.equal(f.reloads(), 1);
});

test('安全刷新前候选被替换时不能使用旧回执，失败检查不再沿用先前就绪状态', async (t) => {
  const f = fixture(t, { version: 'second', blocked: true });
  await settle();
  f.registration.waiting.state = 'redundant';
  f.busy.value = false;
  await settle();
  assert.equal(f.reloads(), 0);
  globalThis.fetch = async () => { throw new TypeError('断网'); };
  await f.update.check();
  f.update.reload();
  assert.equal(f.reloads(), 0);
  assert.equal(f.saves(), 0);
});

test('没有 Service Worker 的 HTTP 直连沿用在线刷新与草稿门禁', async (t) => {
  const f = fixture(t, { version: 'second', secure: false, blocked: true });
  await settle();
  assert.equal(f.registrations.length, 0);
  assert.equal(f.reloads(), 0);
  f.busy.value = false;
  await settle();
  assert.equal(f.saves(), 1);
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
