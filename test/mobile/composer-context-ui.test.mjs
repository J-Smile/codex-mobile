import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import { threadContext } from '../../mobile/src/features/composer/composerContext.ts';

const source = readFileSync(new URL('../../mobile/src/features/composer/ComposerContext.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'context-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
async function settled() { await vue.nextTick(); await setImmediate(); await vue.nextTick(); }

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
