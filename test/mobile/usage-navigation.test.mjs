import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { computed, effectScope, nextTick, reactive, ref, shallowRef, watch } from 'vue';

const source = fs.readFileSync(new URL('../../mobile/src/features/usage/UsagePanel.vue', import.meta.url), 'utf8').match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1];
const ast = ts.createSourceFile('UsagePanel.ts', source, ts.ScriptTarget.Latest, true);
const script = ast.statements.filter(node => !ts.isImportDeclaration(node)).map(node => node.getText(ast)).join('\n');
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
const settle = async () => { await nextTick(); await new Promise(resolve => setImmediate(resolve)); await nextTick(); };

function fixture(t, respond) {
  const props = reactive({ visible: false, initialThreadId: '', initialTitle: '', returnToChat: false });
  const cleanup = [], calls = [];
  const bindings = {
    computed, nextTick, ref, shallowRef, watch,
    defineProps: () => props, defineEmits: () => () => {}, defineExpose: () => {},
    onMounted: () => {}, onBeforeUnmount: fn => cleanup.push(fn),
    document: { visibilityState: 'visible', querySelector: () => null, removeEventListener() {} },
    ApiError: class extends Error {}, threadUrl: (id, path) => '/api/threads/' + id + '/' + path,
    request: async (url, { signal }) => {
      calls.push({ url, signal });
      if (respond) return respond(url);
      return url.startsWith('/api/usage/threads?')
        ? { status: 'ready', items: [{ threadId: 'T1', title: '测试对话' }], nextCursor: 'next' }
        : { status: 'ready', scope: { kind: 'local' } };
    },
  };
  const scope = effectScope();
  const view = scope.run(() => new Function(...Object.keys(bindings), compiled
    + '\nreturn { tab, period, search, scroller, entries, threadId, backLabel, listError, listLoading, listLoaded, selectTab, openThread, back, changePeriod, changeSearch, loadThreads };')(...Object.values(bindings)));
  view.scroller.value = { scrollTop: 0 };
  t.after(() => { cleanup.forEach(fn => fn()); scope.stop(); });
  return { props, ...view, calls };
}

test('重新进入用量保留栏目、范围、搜索、列表和位置，仍刷新当前数据', async t => {
  const f = fixture(t);
  f.props.visible = true;
  await settle();
  f.period.value = '30d';
  f.search.value = '测试';
  f.selectTab('threads');
  await settle();
  f.scroller.value.scrollTop = 180;
  f.props.visible = false;
  await settle();
  const requests = f.calls.length;
  f.scroller.value.scrollTop = 0;
  f.props.visible = true;
  await settle();
  assert.equal(f.tab.value, 'threads');
  assert.equal(f.period.value, '30d');
  assert.equal(f.search.value, '测试');
  assert.equal(f.entries.value[0].threadId, 'T1');
  assert.equal(f.scroller.value.scrollTop, 180);
  assert.ok(f.calls.length > requests);
});

test('全局用量与聊天入口分别保留状态，同一对话 ID 不混淆返回层级', async t => {
  const f = fixture(t);
  f.props.visible = true;
  await settle();
  f.selectTab('threads');
  await settle();
  f.scroller.value.scrollTop = 120;
  f.openThread({ threadId: 'T1', title: '测试对话' });
  await settle();
  assert.equal(f.backLabel.value, '返回用量对话列表');
  f.props.visible = false;
  await settle();
  f.props.initialThreadId = 'T1';
  f.props.returnToChat = true;
  f.props.visible = true;
  await settle();
  assert.equal(f.backLabel.value, '返回原对话');
  assert.equal(f.back(), false);
  f.selectTab('daily');
  f.props.visible = false;
  await settle();
  f.props.initialThreadId = '';
  f.props.returnToChat = false;
  f.props.visible = true;
  await settle();
  assert.equal(f.threadId.value, 'T1');
  assert.equal(f.back(), true);
  await settle();
  assert.equal(f.threadId.value, '');
  assert.equal(f.tab.value, 'threads');
  assert.equal(f.scroller.value.scrollTop, 120);
  assert.equal(f.back(), false);
  f.props.visible = false;
  await settle();
  f.props.initialThreadId = 'T1';
  f.props.visible = true;
  await settle();
  assert.equal(f.tab.value, 'daily');
});

test('对话用量读取失败后停止加载，新搜索清理旧错误并恢复结果', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let reads = 0;
  const f = fixture(t, async url => {
    if (!url.startsWith('/api/usage/threads?')) return { status: 'ready', scope: { kind: 'local' } };
    if (++reads === 1) throw new Error('对话列表暂时不可用');
    return { status: 'ready', items: [{ threadId: 'found', title: '匹配标题' }], nextCursor: null };
  });
  f.props.visible = true;
  await settle();
  f.selectTab('threads');
  await settle();
  assert.equal(f.listError.value, '对话列表暂时不可用');
  assert.equal(f.listLoading.value, false);
  assert.equal(f.entries.value.length, 0);
  f.search.value = '匹配';
  f.changeSearch();
  assert.equal(f.listError.value, '');
  assert.equal(f.listLoaded.value, false);
  t.mock.timers.tick(250);
  await settle();
  assert.equal(f.listLoading.value, false);
  assert.equal(f.listLoaded.value, true);
  assert.equal(f.entries.value[0].threadId, 'found');
  assert.equal(new URL(f.calls.at(-1).url, 'http://local').searchParams.get('search'), '匹配');
});
