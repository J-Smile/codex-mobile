import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import { threadContext } from '../../apps/mobile/src/features/composer/composerContext.ts';

const source = readFileSync(new URL('../../apps/mobile/src/features/composer/ComposerContext.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'context-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
async function settled() { await vue.nextTick(); await setImmediate(); await vue.nextTick(); }

function fixture(t, initialProps, read) {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const calls = [];
  const events = [];
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => name === 'vue' ? { ...vue, useId: () => 'context-test' }
    : name === './composerContext' ? { threadContext }
    : name === '../../api' ? { request: (url, options) => {
      calls.push({ url, ...options });
      return read(url, options);
    } } : {}, module, module.exports);
  const scope = vue.effectScope();
  const props = vue.reactive({ open: false, threads: [], ...initialProps });
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit: (...args) => events.push(args) }));
  t.after(() => scope.stop());
  return { app, props, calls, events, scope };
}

const file = { id: '/fixture/UI [review].vue', kind: 'file', label: 'UI [review].vue', description: 'UI [review].vue', mention: '[UI \\[review\\].vue](</fixture/UI [review].vue>)' };
const references = { mode: 'references', plugins: [], browserTabs: [], files: [file], pluginMessage: '当前连接未开放插件上下文。', browserMessage: '当前连接未开放浏览器上下文。' };

test('上下文读取失败后可以按原搜索与项目重试，等待期间不残留上次错误', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const calls = [];
  const module = { exports: {} };
  const catalog = { plugins: [{ id: 'pdf', kind: 'plugin', label: 'PDF', description: '文档', mention: '[PDF](plugin://pdf@test)' }], browserTabs: [], files: [] };
  new Function('require', 'module', 'exports', compiled)(name => name === 'vue' ? { ...vue, useId: () => 'context-test' }
    : name === './composerContext' ? { threadContext }
    : name === '../../api' ? { request: async (url, options) => {
      calls.push({ url, ...options });
      if (calls.length === 1) throw new Error('上下文暂时无法读取');
      return catalog;
    } } : {}, module, module.exports);
  const scope = vue.effectScope();
  const props = vue.reactive({ open: false, projectId: 'project-a', threads: [] });
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit() {} }));
  t.after(() => scope.stop());
  app.query.value = 'pdf';
  props.open = true;
  await vue.nextTick();
  assert.equal(app.loading.value, true);
  t.mock.timers.tick(150);
  await settled();
  assert.equal(app.error.value, '上下文暂时无法读取');
  assert.equal(app.loading.value, false);
  app.retry.value++;
  await vue.nextTick();
  assert.equal(app.error.value, '');
  assert.equal(app.loading.value, true);
  t.mock.timers.tick(150);
  await settled();
  assert.equal(calls.length, 2);
  assert.equal(calls[1].url, calls[0].url);
  assert.equal(new URL(calls[1].url, 'http://local').searchParams.get('projectId'), 'project-a');
  assert.equal(new URL(calls[1].url, 'http://local').searchParams.get('query'), 'pdf');
  assert.deepEqual(app.catalog.value, catalog);
  assert.equal(app.loading.value, false);
  assert.equal(app.error.value, '');
});

test('文件引用模式沿用真实来源项目，已有聊天查询只传目标聊天且不展示插件和浏览器类别', async t => {
  const { app, props, calls } = fixture(t, { projectId: 'project-a', sourceThreadId: 'source-a' }, async () => references);
  app.query.value = 'review';
  props.open = true;
  await vue.nextTick();
  t.mock.timers.tick(150);
  await settled();
  assert.deepEqual(Object.fromEntries(new URL(calls[0].url, 'http://local').searchParams), {
    query: 'review', projectId: 'project-a', sourceThreadId: 'source-a',
  });
  assert.deepEqual(app.sections.value.map(section => section.id), ['files']);
  assert.deepEqual(app.sections.value[0].items, [file]);
  props.sourceThreadId = 'source-b';
  await vue.nextTick();
  assert.equal(calls[0].signal.aborted, true);
  assert.deepEqual(app.catalog.value.files, [], '来源改变立即清除旧文件候选');
  t.mock.timers.tick(150);
  await settled();
  assert.equal(new URL(calls[1].url, 'http://local').searchParams.get('sourceThreadId'), 'source-b');
  props.projectId = 'project-b';
  await vue.nextTick();
  assert.deepEqual(app.catalog.value.files, []);
  t.mock.timers.tick(150);
  await settled();
  assert.deepEqual(Object.fromEntries(new URL(calls[2].url, 'http://local').searchParams), {
    query: 'review', projectId: 'project-b', sourceThreadId: 'source-b',
  });
  props.threadId = 'target-chat';
  await vue.nextTick();
  t.mock.timers.tick(150);
  await settled();
  assert.deepEqual(Object.fromEntries(new URL(calls[3].url, 'http://local').searchParams), { query: 'review', threadId: 'target-chat' });
});

test('文件读取失败和重试等待均可选择已加载聊天，保留原聊天ID并排除当前聊天', async t => {
  const target = { id: 'target-chat', title: 'UI [review]', cwd: '/fixture/target' };
  const { app, props, calls, events } = fixture(t, {
    threadId: 'current-chat', threads: [{ id: 'current-chat', title: 'review当前聊天' }, target, { id: 'unrelated', title: '其他聊天' }],
  }, async () => { throw new Error('文件列表读取失败'); });
  app.query.value = 'REVIEW';
  props.open = true;
  await vue.nextTick();
  assert.equal(app.loading.value, true);
  assert.deepEqual(app.sections.value.map(section => section.id), ['files']);
  assert.deepEqual(app.sections.value[0].items, [threadContext(target)]);
  t.mock.timers.tick(150);
  await settled();
  assert.equal(app.error.value, '文件列表读取失败');
  assert.deepEqual(app.sections.value[0].items, [threadContext(target)]);
  app.select(app.sections.value[0].items[0]);
  assert.deepEqual(events, [['select', '[UI \\[review\\]](codex://threads/target-chat)']]);
  app.retry.value++;
  await vue.nextTick();
  assert.equal(app.error.value, '');
  assert.equal(app.loading.value, true);
  assert.deepEqual(app.sections.value[0].items, [threadContext(target)]);
  t.mock.timers.tick(150);
  await settled();
  assert.equal(calls[0].url, calls[1].url);
  assert.deepEqual(app.sections.value.map(section => section.id), ['files']);
});

for (const change of ['query', 'threadId', 'projectId', 'sourceThreadId', 'close']) {
  test(`引用候选${change}变更取消旧请求，迟到成功和失败不能污染当前结果`, async t => {
    const pending = [];
    const { app, props, calls } = fixture(t, { projectId: 'project-a', sourceThreadId: 'source-a' }, () =>
      new Promise((resolve, reject) => pending.push({ resolve, reject })));
    app.query.value = 'review';
    props.open = true;
    await vue.nextTick();
    t.mock.timers.tick(150);
    await settled();
    pending[0].resolve(references);
    await settled();
    assert.deepEqual(app.catalog.value.files, [file]);

    app.retry.value++;
    await vue.nextTick();
    assert.deepEqual(app.catalog.value.files, []);
    t.mock.timers.tick(150);
    await settled();
    if (change === 'query') app.query.value = 'new-file';
    else if (change === 'close') props.open = false;
    else props[change] = 'changed-target';
    await vue.nextTick();
    assert.equal(calls[1].signal.aborted, true);
    assert.deepEqual(app.catalog.value.files, []);
    assert.equal(app.error.value, '');
    assert.equal(app.loading.value, change !== 'close');
    pending[1].resolve(references);
    await settled();
    assert.deepEqual(app.catalog.value.files, [], '被取消的成功回执不能重新填充旧文件候选');

    if (change === 'close') {
      t.mock.timers.tick(150);
      await settled();
      assert.equal(calls.length, 2, '关闭后不继续查找');
      props.open = true;
      await vue.nextTick();
    }
    t.mock.timers.tick(150);
    await settled();
    const current = { ...references, files: [{ ...file, id: '/fixture/new.vue', label: 'new.vue' }] };
    app.retry.value++;
    await vue.nextTick();
    assert.equal(calls[2].signal.aborted, true);
    t.mock.timers.tick(150);
    await settled();
    pending[3].resolve(current);
    await settled();
    pending[2].reject(new Error('旧目标的迟到错误'));
    await settled();
    assert.deepEqual(app.catalog.value, current);
    assert.equal(app.error.value, '');
    assert.equal(app.loading.value, false);
  });
}

test('关闭搜索防抖和卸载在途读取均结束当前寿命，不额外请求或写回结果', async t => {
  let finish;
  const { app, props, calls, scope } = fixture(t, {}, () => new Promise(resolve => { finish = resolve; }));
  app.query.value = 'review';
  props.open = true;
  await vue.nextTick();
  t.mock.timers.tick(149);
  props.open = false;
  await vue.nextTick();
  t.mock.timers.tick(1);
  await settled();
  assert.equal(calls.length, 0);
  assert.equal(app.loading.value, false);
  props.open = true;
  await vue.nextTick();
  t.mock.timers.tick(150);
  await settled();
  scope.stop();
  assert.equal(calls[0].signal.aborted, true);
  finish(references);
  await settled();
  assert.deepEqual(app.catalog.value.files, []);
  assert.equal(app.error.value, '');
});
