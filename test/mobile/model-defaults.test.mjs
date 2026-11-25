import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';

// Exercise the actual component setup with controlled API responses. NativeSheet
// is not mounted here; its modal lifecycle has a separate focused test suite.
const source = readFileSync(new URL('../../mobile/src/features/settings/ModelSettings.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'model-defaults-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, responder, overrides = {}) {
  const calls = [], cleanup = [], events = [];
  const props = vue.reactive({ thread: null, newThread: true, connected: true, projectId: 'project-a', ...overrides });
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

test('entering a new draft reads its project defaults without opening model settings', async t => {
  const response = deferred();
  const model = fixture(t, () => response.promise, { projectId: 'project A/one' });
  assert.equal(model.app.open.value, false);
  assert.equal(model.app.defaultsLoading.value, true);
  assert.equal(model.calls.length, 1);
  const url = new URL(model.calls[0].url, 'http://localhost');
  assert.equal(url.pathname, '/api/new-thread-settings');
  assert.equal(url.searchParams.get('projectId'), 'project A/one');
  response.resolve({ model: 'configured-model', effort: 'high', recentModels: ['unrelated-recent-model'] });
  await settled();
  assert.equal(model.app.defaultModel.value, 'configured-model');
  assert.equal(model.app.defaultEffort.value, 'high');
  assert.equal(model.app.defaultEffortLabel.value, 'High');
  assert.equal(model.app.defaultsLoading.value, false);
  assert.equal(model.app.defaultsError.value, '');
});

test('a nullable default effort is omitted instead of inventing a reasoning level', async t => {
  const model = fixture(t, () => ({ model: 'configured-model', effort: null }));
  await settled();
  assert.equal(model.app.defaultModel.value, 'configured-model');
  assert.equal(model.app.defaultEffort.value, '');
  assert.equal(model.app.defaultEffortLabel.value, '');
  assert.equal(model.app.defaultsError.value, '');
});

test('missing or failed defaults never fall back to a recent model or previous thread', async t => {
  for (const respond of [
    () => ({ recentModels: ['unrelated-recent-model'] }),
    () => ({ model: '  ', effort: 'high', recentModels: ['unrelated-recent-model'] }),
    () => { throw new Error('默认模型暂时无法读取'); },
  ]) {
    const model = fixture(t, respond, { thread: { id: 'previous-thread', model: 'previous-thread-model', reasoningEffort: 'max' } });
    await settled();
    assert.equal(model.app.defaultModel.value, '');
    assert.equal(model.app.defaultEffort.value, '');
    assert.equal(model.app.defaultsLoading.value, false);
    assert.notEqual(model.app.defaultsError.value, '');
    assert.equal(model.calls.length, 1);
  }
});

test('retry recovers from a defaults read failure using the returned values', async t => {
  const model = fixture(t, (_, __, index) => {
    if (!index) throw new Error('默认模型暂时无法读取');
    return { model: 'recovered-model', effort: 'medium' };
  });
  await settled();
  assert.notEqual(model.app.defaultsError.value, '');
  await model.app.readDefaults();
  assert.equal(model.calls.length, 2);
  assert.equal(model.app.defaultModel.value, 'recovered-model');
  assert.equal(model.app.defaultEffort.value, 'medium');
  assert.equal(model.app.defaultsError.value, '');
});

test('changing draft projects aborts the old request and ignores its late response', async t => {
  const responses = [deferred(), deferred(), deferred()];
  const model = fixture(t, (_, __, index) => responses[index].promise);
  model.props.projectId = 'project-b';
  await settled();
  assert.equal(model.calls[0].signal.aborted, true);
  assert.equal(new URL(model.calls[1].url, 'http://localhost').searchParams.get('projectId'), 'project-b');
  responses[1].resolve({ model: 'project-b-model', effort: 'high' });
  await settled();
  responses[0].resolve({ model: 'stale-project-a-model', effort: 'low' });
  await settled();
  assert.equal(model.app.defaultModel.value, 'project-b-model');
  assert.equal(model.app.defaultEffort.value, 'high');
  model.props.projectId = undefined;
  await settled();
  assert.equal(model.app.defaultModel.value, '');
  assert.equal(model.calls[2].url, '/api/new-thread-settings');
  responses[2].resolve({ model: 'user-default-model', effort: null });
  await settled();
  assert.equal(model.app.defaultModel.value, 'user-default-model');
});

test('leaving new-thread mode aborts the defaults read and discards stale results', async t => {
  const response = deferred();
  const model = fixture(t, () => response.promise);
  model.props.newThread = false;
  await settled();
  assert.equal(model.calls[0].signal.aborted, true);
  assert.equal(model.app.defaultsLoading.value, false);
  response.resolve({ model: 'stale-default-model', effort: 'high' });
  await settled();
  assert.equal(model.app.defaultModel.value, '');
  assert.equal(model.app.defaultEffort.value, '');
  assert.equal(model.app.defaultsError.value, '');
});

test('new-thread mode cannot load or save per-thread settings even with a previous thread present', async t => {
  const model = fixture(t, () => ({ model: 'configured-model', effort: 'high' }), { thread: { id: 'previous-thread', model: 'previous-thread-model' } });
  await settled();
  model.app.open.value = true;
  model.app.model.value = 'attempted-override';
  model.app.effort.value = 'max';
  await settled();
  await model.app.save();
  assert.equal(model.calls.length, 1);
  assert.equal(new URL(model.calls[0].url, 'http://localhost').pathname, '/api/new-thread-settings');
  assert.notEqual(model.calls[0].method, 'POST');
  assert.equal(model.app.defaultModel.value, 'configured-model');
});

test('existing settings retry a failed read and preserve a nullable desktop effort when switching model', async t => {
  const model = fixture(t, (_, options, index) => {
    if (!index) throw new Error('等待实时连接');
    return options.method === 'POST' ? { applied: true } : { model: 'model-a', effort: null, mode: 'default', recentModels: ['model-b'] };
  }, { newThread: false, thread: { id: 'chat', model: 'model-a' } });
  model.app.open.value = true;
  await settled();
  assert.equal(model.app.loaded.value, false);
  assert.equal(model.app.error.value, '等待实时连接');
  await model.app.readSettings();
  assert.equal(model.app.loaded.value, true);
  model.app.model.value = 'model-b';
  await model.app.save();
  assert.deepEqual(JSON.parse(model.calls.at(-1).body), { model: 'model-b', effort: null });
  assert.equal(model.app.open.value, false);
  assert.equal(model.events[0][1].model, 'model-b');
});

test('a rejected model apply keeps the sheet and edited values for retry', async t => {
  const model = fixture(t, (_, options) => options.method === 'POST' ? { applied: false } : { model: 'model-a', effort: 'high', mode: 'default', recentModels: [] },
    { newThread: false, thread: { id: 'chat' } });
  model.app.open.value = true;
  await settled();
  model.app.model.value = 'model-b';
  await model.app.save();
  assert.equal(model.app.open.value, true);
  assert.equal(model.app.model.value, 'model-b');
  assert.equal(model.app.error.value, '模型设置尚未应用，请重试。');
  assert.deepEqual(model.events, []);
});
