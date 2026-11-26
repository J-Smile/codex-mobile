import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';

const source = readFileSync(new URL('../../apps/mobile/src/features/settings/HookReview.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'hook-review-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
const configuration = { mode: 'read-only', pendingReview: null, sources: [], issues: [] };

function fixture(t, request, desktopSupported = true) {
  const cleanup = [], calls = [];
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => {
    if (name === 'vue') return { ...vue, useId: () => 'hooks-test', onBeforeUnmount: callback => cleanup.push(callback) };
    if (name === '../../api') return { request: async (url, options) => { calls.push(url); return request(url, options); }, threadUrl: (id, action) => `/api/threads/${id}/${action}` };
    return {};
  }, module, module.exports);
  const props = vue.reactive({ threadId: 'isolated-chat', desktopSupported });
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit() {} }));
  const dispose = () => { cleanup.forEach(callback => callback()); scope.stop(); };
  t.after(dispose);
  return { app, calls, props, dispose };
}

test('打开电脑失败清除旧成功提示，重试打开不会重新读取钩子', async t => {
  let attempts = 0;
  const { app, calls } = fixture(t, async url => {
    if (url.endsWith('/hooks')) return configuration;
    if (++attempts === 2) throw new Error('电脑暂时无法打开对话');
    return { ok: true };
  });
  app.show();
  await setImmediate();
  await app.openDesktop();
  assert.equal(app.openedDesktop.value, true);
  await app.openDesktop();
  assert.equal(app.openedDesktop.value, false);
  assert.equal(app.desktopError.value, '电脑暂时无法打开对话');
  assert.equal(app.error.value, '');
  assert.deepEqual(app.data.value, configuration);
  await app.openDesktop();
  assert.equal(app.openedDesktop.value, true);
  assert.equal(app.desktopError.value, '');
  assert.equal(calls.filter(url => url.endsWith('/hooks')).length, 1);
  assert.equal(calls.filter(url => url.endsWith('/open')).length, 3);
});

test('读取钩子和打开电脑各自保留错误，重新读取只恢复配置', async t => {
  let reads = 0;
  const { app } = fixture(t, async url => {
    if (url.endsWith('/open')) throw new Error('无法打开电脑上的对话');
    if (++reads === 1) throw new Error('配置暂时无法读取');
    return configuration;
  });
  app.show();
  await setImmediate();
  await app.openDesktop();
  assert.equal(app.error.value, '配置暂时无法读取');
  assert.equal(app.desktopError.value, '无法打开电脑上的对话');
  await app.load();
  assert.equal(app.error.value, '');
  assert.equal(app.desktopError.value, '无法打开电脑上的对话');
  assert.deepEqual(app.data.value, configuration);
});

test('缺少桌面导航能力仍可读取钩子，不发送打开请求', async t => {
  const { app, calls } = fixture(t, async () => configuration, false);
  app.show();
  await setImmediate();
  assert.deepEqual(app.data.value, configuration);
  await app.openDesktop();
  assert.deepEqual(calls, ['/api/threads/isolated-chat/hooks']);
  assert.equal(app.openedDesktop.value, false);
});

for (const finish of ['close', 'switch', 'dispose']) {
  test(`钩子面板${finish}取消等待，迟到读取和导航不回写`, async t => {
    const pending = new Map();
    const { app, props, dispose } = fixture(t, (url, options) => new Promise(resolve => {
      pending.set(url.split('/').pop(), { resolve, signal: options.signal });
    }));
    app.show();
    await setImmediate();
    const opening = app.openDesktop();
    assert.equal(app.openingDesktop.value, true);
    if (finish === 'close') app.open.value = false;
    else if (finish === 'switch') props.threadId = 'another-chat';
    else dispose();
    await setImmediate();
    assert.equal(pending.get('hooks').signal.aborted, true);
    assert.equal(pending.get('open').signal.aborted, true);
    pending.get('hooks').resolve(configuration);
    pending.get('open').resolve({ ok: true });
    await opening;
    await setImmediate();
    assert.equal(app.data.value, null);
    assert.equal(app.openedDesktop.value, false);
    assert.equal(app.desktopError.value, '');
  });
}

test('关闭重开后旧导航失败不能覆盖新导航状态', async t => {
  const pending = [];
  const { app } = fixture(t, async url => url.endsWith('/hooks') ? configuration : new Promise((resolve, reject) => pending.push({ resolve, reject })));
  app.show();
  await setImmediate();
  const previous = app.openDesktop();
  app.open.value = false;
  await setImmediate();
  app.show();
  await setImmediate();
  const current = app.openDesktop();
  pending[0].reject(new Error('旧导航失败'));
  await previous;
  assert.equal(app.openingDesktop.value, true);
  assert.equal(app.desktopError.value, '');
  pending[1].resolve({ ok: true });
  await current;
  assert.equal(app.openedDesktop.value, true);
  assert.equal(app.openingDesktop.value, false);
});
