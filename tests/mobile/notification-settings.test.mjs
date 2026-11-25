import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { effectScope, nextTick, onScopeDispose, ref, watch } from 'vue';

const source = fs.readFileSync(new URL('../../apps/mobile/src/features/settings/useNotifications.ts', import.meta.url), 'utf8').replace(/^import .*;$/gm, '');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t) {
  const authenticated = ref(true), calls = [];
  const listeners = new Map(), opened = [];
  let subscription = null, permissions = 0, removed = 0;
  const registered = { active: true, pushManager: {
    getSubscription: async () => subscription,
    subscribe: async () => subscription = { endpoint: 'https://web.push.apple.com/test', toJSON: () => ({ endpoint: 'https://web.push.apple.com/test', keys: {} }), unsubscribe: async () => { removed++; subscription = null; } },
  } };
  const bindings = {
    exports: {}, ref, watch, onScopeDispose, relayDesktop: () => '', relayAccount: () => '',
    window: { isSecureContext: true, PushManager: {}, Notification: {} },
    navigator: { serviceWorker: { getRegistration: async () => registered,
      addEventListener: (name, handler) => listeners.set(name, handler), removeEventListener: name => listeners.delete(name) } },
    Notification: { requestPermission: () => { permissions++; return Promise.resolve('granted'); } }, atob,
    request: async (url, options = {}) => { calls.push([url, options.body ? JSON.parse(options.body) : null]); return url.endsWith('/key') ? { publicKey: 'AA' } : { enabled: false, error: '' }; },
  };
  const useNotifications = new Function(...Object.keys(bindings), compiled + '\nreturn exports.useNotifications;')(...Object.values(bindings));
  const scope = effectScope();
  const app = scope.run(() => useNotifications(() => authenticated.value, threadId => opened.push(threadId)));
  t.after(() => scope.stop());
  return { ...app, calls, authenticated, permissions: () => permissions, removed: () => removed, opened, listeners, dispose: () => scope.stop(),
    receive: (data, ports = []) => listeners.get('message')?.({ data, ports }) };
}

test('列表页无需选中对话即可开启所有通知，测试和关闭也不提交对话编号', async t => {
  const f = fixture(t);
  await nextTick();
  await f.toggle();
  assert.equal(f.enabled.value, true);
  assert.equal(f.permissions(), 1);
  assert.match(f.hint, /自动包含新对话/);
  assert.deepEqual(f.calls.find(([url]) => url.endsWith('/subscribe'))[1], { subscription: { endpoint: 'https://web.push.apple.com/test', keys: {} } });
  await f.sendTest();
  assert.equal(f.notice.value, '测试通知已发送。');
  assert.deepEqual(f.calls.find(([url]) => url.endsWith('/test'))[1], { endpoint: 'https://web.push.apple.com/test' });
  await f.toggle();
  assert.equal(f.enabled.value, false);
  assert.equal(f.removed(), 1);
  assert.deepEqual(f.calls.find(([url]) => url.endsWith('/unsubscribe'))[1], { endpoint: 'https://web.push.apple.com/test' });
});

test('退出登录清理全局开关，未登录不会发起订阅', async t => {
  const f = fixture(t);
  await nextTick();
  await f.toggle();
  f.authenticated.value = false;
  await nextTick();
  assert.equal(f.enabled.value, false);
  const count = f.calls.length;
  await f.toggle();
  assert.equal(f.calls.length, count);
});

test('通知点击打开指定对话，多次点击同一对话仍可定位，销毁后移除监听', async t => {
  const f = fixture(t);
  const acknowledgements = [];
  f.receive({ type: 'codex-open-thread', threadId: 'not-the-previous-chat' }, [{ postMessage: value => acknowledgements.push(value) }]);
  await nextTick();
  f.receive({ type: 'codex-open-thread', threadId: 'not-the-previous-chat' });
  await nextTick();
  assert.deepEqual(f.opened, ['not-the-previous-chat', 'not-the-previous-chat']);
  assert.deepEqual(acknowledgements, [true]);
  f.receive({ type: 'other-event', threadId: 'ignore' });
  f.receive({ type: 'codex-open-thread', threadId: null });
  await nextTick();
  assert.equal(f.opened.length, 2);
  f.dispose();
  assert.equal(f.listeners.has('message'), false);
});

test('登录过程中保留最新通知目标，在初始化地址恢复后定位', async t => {
  const f = fixture(t);
  f.authenticated.value = false;
  f.receive({ type: 'codex-open-thread', threadId: 'first-notice' });
  await nextTick();
  f.receive({ type: 'codex-open-thread', threadId: 'latest-notice' });
  await nextTick();
  assert.deepEqual(f.opened, []);
  f.authenticated.value = true;
  f.opened.push('restored-old-url');
  await nextTick();
  assert.deepEqual(f.opened, ['restored-old-url', 'latest-notice']);
});
