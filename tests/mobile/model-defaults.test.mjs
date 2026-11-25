import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';

// Exercise the actual component setup with controlled API responses. NativeSheet
// is not mounted here; its modal lifecycle has a separate focused test suite.
const source = readFileSync(new URL('../../apps/mobile/src/features/settings/ModelSettings.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'model-defaults-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, responder, overrides = {}) {
  const calls = [], cleanup = [], events = [];
  const props = vue.reactive({ thread: null, newThread: true, connected: true, selection: {}, ...overrides });
  const api = {
    threadUrl: (id, route) => '/api/threads/' + id + '/' + route,
    async request(url, options = {}) {
      calls.push({ url, ...options });
      return responder(url, options, calls.length - 1);
    },
  };
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => name === 'vue'
    ? { ...vue, onBeforeUnmount(callback) { cleanup.push(callback); } }
    : name === '../../api' ? api : {}, module, module.exports);
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit: (...args) => events.push(args) }));
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  return { props, app, calls, events };
}

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}
async function settled() { await vue.nextTick(); await setImmediate(); await vue.nextTick(); }

test('新建默认不读取 CLI 设置，也不把最近模型当成默认值', async t => {
  const model = fixture(t, () => ({ recentModels: ['recent-model'], efforts: ['low', 'high'] }));
  assert.equal(model.calls.length, 0);
  assert.equal(model.app.modelLabel.value, '桌面默认');
  model.app.open.value = true;
  await settled();
  assert.equal(model.calls[0].url, '/api/model-options');
  assert.equal(model.app.model.value, '');
  assert.equal(model.app.effort.value, '');
  model.app.save();
  assert.deepEqual(model.events, [['update:selection', {}]]);
  assert.equal(model.calls.length, 1);
});

test('选择模型仅更新本次草稿，不改写真实聊天元数据', async t => {
  const model = fixture(t, () => ({ recentModels: ['model-b'], efforts: ['low', 'high'] }),
    { newThread: false, thread: { id: 'chat', model: 'model-a', reasoningEffort: 'medium' } });
  model.app.open.value = true;
  await settled();
  model.app.model.value = 'model-b';
  model.app.effort.value = 'high';
  model.app.save();
  assert.deepEqual(model.events, [['update:selection', { model: 'model-b', thinking: 'high' }]]);
  assert.equal(model.props.thread.model, 'model-a');
  assert.equal(model.props.thread.reasoningEffort, 'medium');
  assert.equal(model.calls.length, 1);
  assert.equal(model.app.open.value, false);
});

test('只选择强度时不携带模型覆盖', async t => {
  const model = fixture(t, () => ({ recentModels: [], efforts: ['high'] }));
  model.app.open.value = true;
  await settled();
  model.app.effort.value = 'high';
  model.app.save();
  assert.deepEqual(model.events, [['update:selection', { thinking: 'high' }]]);
});

test('选项读取失败可重试，已有草稿选择保留', async t => {
  const model = fixture(t, (_, __, index) => {
    if (!index) throw new Error('桌面连接暂时不可用');
    return { recentModels: [], efforts: ['high'] };
  }, { selection: { model: 'custom/model', thinking: 'high' } });
  model.app.open.value = true;
  await settled();
  model.app.save();
  assert.equal(model.app.error.value, '桌面连接暂时不可用');
  assert.deepEqual(model.events, []);
  await model.app.readOptions();
  assert.equal(model.app.customModel.value, true);
  model.app.save();
  assert.deepEqual(model.events, [['update:selection', { model: 'custom/model', thinking: 'high' }]]);
});

test('关闭面板取消请求，迟到的目录不修改组件状态', async t => {
  const response = deferred();
  const model = fixture(t, () => response.promise);
  model.app.open.value = true;
  await settled();
  model.app.open.value = false;
  await settled();
  assert.equal(model.calls[0].signal.aborted, true);
  response.resolve({ recentModels: ['stale-model'], efforts: ['high'] });
  await settled();
  assert.equal(model.app.loaded.value, false);
  assert.deepEqual(model.app.recentModels.value, []);
});

test('断连或无效的模型和强度不能提交草稿选择', async t => {
  const model = fixture(t, () => ({ recentModels: [], efforts: ['high'] }));
  model.app.open.value = true;
  await settled();
  model.props.connected = false;
  model.app.save();
  assert.deepEqual(model.events, []);
  model.props.connected = true;
  model.app.model.value = 'bad model';
  model.app.save();
  assert.equal(model.app.error.value, '请输入有效的模型 ID。');
  model.app.model.value = 'valid-model';
  model.app.effort.value = 'unsupported';
  model.app.save();
  assert.equal(model.app.error.value, '请选择当前桌面支持的思考强度。');
  assert.deepEqual(model.events, []);
});
