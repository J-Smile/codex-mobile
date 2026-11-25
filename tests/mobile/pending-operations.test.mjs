import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { reactive, ref } from 'vue';

const source = readFileSync(new URL('../../apps/mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const methods = new Set(['loadOperations', 'closeOperation', 'readRetryMessages', 'persistRetryMessages']);
const compiled = ts.transpileModule(ast.statements.filter(node => ts.isFunctionDeclaration(node) && methods.has(node.name?.text))
  .map(node => node.getText(ast)).join('\n'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(respond, initial = [['chat', { requestId: 'pending' }]]) {
  const data = new Map();
  const calls = [];
  const state = { retryMessages: reactive(new Map(initial)), unresolvedOperations: ref([]), operationsReady: ref(false), operationsError: ref('') };
  const bindings = { ...state, window: { confirm: () => true }, desktopStorageKey: key => 'fixture-desktop:' + key,
    localStorage: { getItem: key => data.get(key), setItem: (key, value) => data.set(key, value) },
    request: async (url, init = {}) => { const call = { url, method: init.method || 'GET', ...(init.body ? { body: JSON.parse(init.body) } : {}) }; calls.push(call); return respond(call); } };
  return { ...state, calls, data, ...new Function(...Object.keys(bindings), compiled + '\nreturn { ' + [...methods].join(', ') + ' };')(...Object.values(bindings)) };
}

test('刷新保存的操作身份只含目标和 ID，恢复后仍等待核对', async () => {
  const f = fixture(() => ({ operations: [] }));
  f.persistRetryMessages();
  assert.deepEqual(f.readRetryMessages(), [['chat', { requestId: 'pending' }]]);
  assert.equal(f.data.size, 1);
  await f.loadOperations();
  assert.deepEqual(f.unresolvedOperations.value, [{ id: 'pending', target: 'chat', kind: 'message', phase: 'unknown', localOnly: true }]);
  assert.equal(f.operationsReady.value, true);
});

test('服务端未决与本地身份合并，查询失败禁止投递并保留未决项', async () => {
  let fail = false;
  const f = fixture(() => { if (fail) throw new Error('offline'); return { operations: [{ id: 'pending', target: 'chat', kind: 'message', phase: 'dispatching' }] }; });
  await f.loadOperations();
  assert.equal(f.unresolvedOperations.value.length, 1);
  assert.equal(f.unresolvedOperations.value[0].localOnly, undefined);
  fail = true;
  await f.loadOperations();
  assert.equal(f.operationsReady.value, false);
  assert.equal(f.unresolvedOperations.value.length, 1);
  assert.match(f.operationsError.value, /阅读和草稿/);
});

test('人工结案只标记 unknown，不重发；进行中或失败结案保留身份', async () => {
  const unknown = { id: 'pending', target: 'chat', kind: 'message', phase: 'unknown' };
  let fail = true;
  const f = fixture(call => { if (call.method === 'POST' && fail) throw new Error('结案失败'); return { operations: [] }; });
  await f.closeOperation({ ...unknown, phase: 'dispatching' });
  assert.deepEqual(f.calls, []);
  await f.closeOperation(unknown);
  assert.equal(f.retryMessages.has('chat'), true);
  assert.equal(f.operationsError.value, '结案失败');
  fail = false;
  await f.closeOperation(unknown);
  assert.equal(f.retryMessages.has('chat'), false);
  assert.deepEqual(f.calls.filter(call => call.method === 'POST'), Array(2).fill({ url: '/api/operations/pending', method: 'POST', body: { confirm: 'close-unknown' } }));
  assert.deepEqual(f.unresolvedOperations.value, []);
});
