import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { computed, ref } from 'vue';
import { projectPreviewCount, recentThreads } from '../../apps/mobile/src/features/threads/useThreadCatalog.ts';

const source = fs.readFileSync(new URL('../../apps/mobile/src/App.vue', import.meta.url), 'utf8').match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const methods = new Set(['pathKey', 'threadProjectId', 'updateThread', 'threadsInProject']);
const compiled = ts.transpileModule(ast.statements.filter(node => ts.isFunctionDeclaration(node) && methods.has(node.name?.text))
  .map(node => node.getText(ast)).join('\n'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture() {
  const projects = ref([{ projectId: 'A', label: '项目 A', path: 'D:/work/A' }, { projectId: 'B', label: '项目 B', path: 'D:/work/B' }]);
  const selectedThread = ref({ id: 'chat', title: '工作树聊天', projectId: 'A', cwd: 'D:/work/A', status: { type: 'active' } });
  const threads = ref([{ ...selectedThread.value }]);
  const latestByProject = computed(() => new Map(projects.value.map(project => [project.projectId,
    recentThreads(threads.value.filter(thread => thread.projectId === project.projectId), projectPreviewCount)])));
  const bindings = {
    projects, selectedThread, selectedProjectId: ref('A'), threads, knownThreads: threads, latestByProject, search: ref(''),
    recentThreads, projectPreviewCount,
    updateCatalogThread: thread => {
      const current = threads.value.find(entry => entry.id === thread.id);
      if (current) Object.assign(current, thread);
    },
  };
  return { ...bindings, ...new Function(...Object.keys(bindings), compiled + '\nreturn {threadProjectId,updateThread,threadsInProject};')(...Object.values(bindings)) };
}

test('工作树实时快照保留桌面已知项目，历史快照不清空列表的执行状态', () => {
  const f = fixture();
  f.updateThread({ id: 'chat', title: '工作树聊天', cwd: 'C:/Users/example/.codex/worktrees/merge/A' });
  assert.equal(f.selectedProjectId.value, 'A');
  assert.equal(f.selectedThread.value.projectId, 'A');
  assert.deepEqual(f.threads.value[0].status, { type: 'active' });
  assert.equal(f.threadsInProject(f.projects.value[0]).length, 1);
});

test('官方项目 ID 决定归属，空值、缺字段和不可用 ID 都不按目录推断', () => {
  const f = fixture();
  assert.equal(f.threadProjectId({ id: 'official', projectId: 'A', cwd: 'D:/work/B' }), 'A');
  assert.equal(f.threadProjectId({ id: 'old', cwd: 'D:/work/A' }), 'other');
  assert.equal(f.threadProjectId({ id: 'nested', cwd: 'D:/work/A/src' }), 'other');
  assert.equal(f.threadProjectId({ id: 'prefix', cwd: 'D:/work/A-v2' }), 'other');
  assert.equal(f.threadProjectId({ id: 'assigned', projectId: 'B', cwd: 'D:/work/A' }), 'B');
  assert.equal(f.threadProjectId({ id: 'unassigned', projectId: null, cwd: 'D:/work/A' }), 'other');
  assert.equal(f.threadProjectId({ id: 'unavailable', projectId: 'C', cwd: 'D:/work/A' }), 'other');
  f.updateThread({ id: 'chat', title: '明确移出项目', projectId: null, cwd: 'D:/work/A' });
  assert.equal(f.selectedProjectId.value, 'other');
  assert.equal(f.threadsInProject(f.projects.value[0]).length, 0);
});

test('目录相同且聊天目录重叠的官方项目按各自 ID 隔离', () => {
  const f = fixture();
  f.projects.value = [
    { projectId: 'native-a', label: '项目 A', path: 'D:/work/shared' },
    { projectId: 'native-b', label: '项目 B', path: 'D:/work/shared' },
  ];
  f.threads.value = [
    { id: 'in-a', title: '属于 A', projectId: 'native-a', cwd: 'D:/work/shared/src' },
    { id: 'in-b', title: '属于 B', projectId: 'native-b', cwd: 'D:/work/shared/src' },
    { id: 'null-id', title: '明确无项目', projectId: null, cwd: 'D:/work/shared' },
    { id: 'missing-id', title: '未提供项目', cwd: 'D:/work/shared/src' },
    { id: 'unknown-id', title: '其他项目', projectId: 'native-c', cwd: 'D:/work/shared' },
  ];
  assert.deepEqual(f.threadsInProject(f.projects.value[0]).map(thread => thread.id), ['in-a']);
  assert.deepEqual(f.threadsInProject(f.projects.value[1]).map(thread => thread.id), ['in-b']);
  assert.deepEqual(f.threads.value.slice(2).map(f.threadProjectId), ['other', 'other', 'other']);
});
