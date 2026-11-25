import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';

const source = readFileSync(new URL('../../apps/mobile/src/features/threads/ThreadBrowser.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'thread-browser-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, respond) {
  const requests = [], cleanup = [];
  const module = { exports: {} };
  new Function('require', 'module', 'exports', 'ResizeObserver', compiled)(name => name === 'vue'
    ? { ...vue, onBeforeUnmount: callback => cleanup.push(callback) }
    : name === '../../api' ? { request: (url, options) => { requests.push({ url, ...options }); return respond(url, options, requests.length); } } : {},
  module, module.exports, class { observe() {} disconnect() {} });
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup({ project: { projectId: 'project', label: '项目' } }, { expose() {}, emit() {} }));
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  return { app, requests };
}
const entries = (start, count = 30) => Array.from({ length: count }, (_, index) => ({ id: 'chat-' + (start + index), title: '聊天 ' + (start + index) }));
async function settled() { await vue.nextTick(); await setImmediate(); await vue.nextTick(); }
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

test('手机历史只在滑近底部时续载，渲染窗口随滚动移动且保留已读聊天', async t => {
  const { app, requests } = fixture(t, async (_, __, count) => ({ threads: entries((count - 1) * 30), nextCursor: count < 3 ? 'cursor-' + count : null }));
  await settled();
  const element = { scrollTop: 0, clientHeight: 520, get scrollHeight() { return app.entries.value.length * 52; } };
  app.list.value = element;
  await settled();
  assert.equal(requests.length, 1);
  assert.equal(app.entries.value.length, 30);
  assert.ok(app.visibleEntries.value.length <= 20);
  element.scrollTop = 1200;
  app.onScroll();
  await settled();
  assert.equal(requests.length, 2);
  assert.equal(new URL(requests[1].url, 'http://local').searchParams.get('cursor'), 'cursor-1');
  assert.equal(app.entries.value.length, 60);
  assert.ok(app.visibleEntries.value.length <= 21);
  assert.ok(app.start.value > 0);
  element.scrollTop = 2800;
  app.onScroll();
  await settled();
  assert.equal(app.entries.value.length, 90);
  assert.equal(app.nextCursor.value, null);
  element.scrollTop = 0;
  app.onScroll();
  await settled();
  assert.equal(app.visibleEntries.value[0].id, 'chat-0');
  assert.equal(requests.length, 3);
});

test('续载失败保留已读内容与游标，重试继续追加而不跳回顶部', async t => {
  const { app, requests } = fixture(t, async (_, __, count) => {
    if (count === 2) throw new Error('网络暂时断开');
    return { threads: entries(count === 1 ? 0 : 30), nextCursor: count === 1 ? 'next' : null };
  });
  await settled();
  await app.load();
  assert.equal(app.error.value, '网络暂时断开');
  assert.equal(app.entries.value.length, 30);
  assert.equal(app.nextCursor.value, 'next');
  assert.equal(app.retryFromStart.value, false);
  await app.load(app.retryFromStart.value);
  assert.equal(app.error.value, '');
  assert.equal(app.entries.value.length, 60);
  assert.ok(requests.slice(1).every(r => new URL(r.url, 'http://local').searchParams.get('cursor') === 'next'));
});

test('已读完全部历史后刷新失败，重试重新读取第一页而非无效续载', async t => {
  const { app, requests } = fixture(t, async (_, __, count) => {
    if (count === 2) throw new Error('刷新暂时失败');
    return { threads: entries(count === 1 ? 0 : 100, 3), nextCursor: null };
  });
  await settled();
  await app.load(true);
  assert.equal(app.error.value, '刷新暂时失败');
  assert.equal(app.entries.value[0].id, 'chat-0');
  assert.equal(app.nextCursor.value, null);
  assert.equal(app.retryFromStart.value, true);
  await app.load(app.retryFromStart.value);
  assert.equal(requests.length, 3);
  assert.equal(app.error.value, '');
  assert.deepEqual(app.entries.value.map(entry => entry.id), ['chat-100', 'chat-101', 'chat-102']);
  assert.ok(requests.every(entry => !new URL(entry.url, 'http://local').searchParams.has('cursor')));
});

test('搜索取消旧续载，迟到结果不能混进新搜索，清空后从头读取', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const old = deferred();
  const { app, requests } = fixture(t, async (url, _, count) => {
    if (count === 1) return { threads: entries(0), nextCursor: 'old-next' };
    if (count === 2) return old.promise;
    return { threads: [{ id: url.includes('query=') ? 'found' : 'initial', title: '结果' }], nextCursor: null };
  });
  await settled();
  const loadingOld = app.load();
  app.query.value = '标题';
  await settled();
  assert.equal(requests[1].signal.aborted, true);
  t.mock.timers.tick(250);
  await settled();
  old.resolve({ threads: entries(30), nextCursor: 'stale-next' });
  await loadingOld;
  assert.deepEqual(app.entries.value.map(t => t.id), ['found']);
  assert.equal(app.nextCursor.value, null);
  app.query.value = '';
  await settled();
  t.mock.timers.tick(250);
  await settled();
  assert.deepEqual(app.entries.value.map(t => t.id), ['initial']);
  assert.equal(new URL(requests.at(-1).url, 'http://local').searchParams.has('cursor'), false);
});
