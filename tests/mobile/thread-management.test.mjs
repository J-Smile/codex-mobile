import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';

const source = readFileSync(new URL('../../apps/mobile/src/features/threads/ThreadManagement.vue', import.meta.url), 'utf8');
const compiled = ts.transpileModule(compileScript(parse(source).descriptor, { id: 'management' }).content,
  { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
function fixture(t, respond) {
  const props = vue.reactive({ open: true, connected: true, thread: { id: 'chat', title: '原名' },
    capabilities: { threadRename: true, threadPin: true, threadArchive: true, threadDelete: true } });
  const calls = [], events = [], cleanup = [], module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => name === 'vue'
    ? { ...vue, useId: () => 'management', onBeforeUnmount: callback => cleanup.push(callback) }
    : name === '../../api' ? { threadUrl: (id, action) => `/api/threads/${id}/${action}`,
      request: async (url, options) => { calls.push({ url, input: JSON.parse(options.body) }); return respond(calls.at(-1)); } } : {}, module, module.exports);
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit: (...event) => events.push(event) }));
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  return { props, calls, events, app };
}

test('管理聊天保留目标、明确删除确认，并等待官方回执', async t => {
  const f = fixture(t, async call => ({ threadId: 'chat', ...call.input, deleted: call.url.endsWith('delete') }));
  f.app.title.value = '新名称';
  await f.app.apply('rename');
  assert.deepEqual(f.calls[0], { url: '/api/threads/chat/manage?action=rename', input: { title: '新名称' } });
  await f.app.apply('delete');
  assert.equal(f.calls.length, 1);
  f.app.confirmingDelete.value = true;
  await f.app.apply('delete');
  assert.equal(f.calls[1].input.confirm, 'delete-thread');
  assert.equal(f.events.at(-1)[1].deleted, true);
});

test('管理操作重复点击只提交一次，切聊天后的回执不关闭新面板', async t => {
  let resolve;
  const f = fixture(t, () => new Promise(done => { resolve = done; }));
  const pending = f.app.apply('pin');
  await f.app.apply('pin');
  assert.equal(f.calls.length, 1);
  f.props.thread = { id: 'other', title: '另一聊天' };
  await vue.nextTick();
  resolve({ threadId: 'chat', pinned: true });
  await pending;
  assert.deepEqual(f.events, [['changed', { threadId: 'chat', pinned: true }]]);
  assert.equal(f.app.title.value, '另一聊天');
});

test('失败保留名称，断开禁止写入，恢复后可提交恢复归档', async t => {
  let fail = true;
  const f = fixture(t, async () => { if (fail) throw new Error('官方暂时不可用'); return { threadId: 'chat', archived: false }; });
  f.app.title.value = '保留的名称';
  await f.app.apply('rename');
  assert.equal(f.app.title.value, '保留的名称');
  assert.match(f.app.error.value, /官方暂时/);
  f.props.connected = false;
  await f.app.apply('archive');
  assert.equal(f.calls.length, 1);
  f.props.connected = true;
  f.props.thread.archived = true;
  fail = false;
  await f.app.apply('archive');
  assert.deepEqual(f.calls.at(-1).input, { archived: false });
});
