import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import ts from 'typescript';
import { computed, effectScope, reactive, ref } from 'vue';
import { ApiError } from '../../mobile/src/api.ts';

// Execute the page's actual draft bindings and creation handler with HTTP/navigation doubles.
const source = readFileSync(new URL('../../mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const declarations = new Set(['retryMessages', 'pendingSends', 'sending', 'activeProject', 'draftKey', 'draft', 'creationSource', 'createdDraft', 'firstTurnPending']);
const selected = ast.statements.filter((node) => {
  if (ts.isFunctionDeclaration(node)) return node.name?.text === 'createChat';
  return ts.isVariableStatement(node) && node.declarationList.declarations.some((entry) => declarations.has(entry.name.getText(ast)));
}).map((node) => node.getText(ast)).join('\n');
const compiled = ts.transpileModule(selected, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, respond = async () => ({ threadId: 'created-chat' })) {
  const requests = [], cleared = [], opened = [];
  const state = {
    projects: ref([{ projectId: 'project-a', label: '项目 A', path: '/project-a' }]),
    selectedProjectId: ref('project-a'), selectedId: ref(''), newChat: ref(true),
    drafts: reactive({ 'new:project-a': '检查手机布局' }),
    error: ref(''), threads: ref([]), capabilities: ref({create:false}), notice:ref(''),
  };
  const images = ref([{ id: 'a'.repeat(32) + '.png', name: '手机截图.png' }]);
  const bindings = {
    readRetryMessages: () => [],
    ...state, computed, reactive, ref, crypto: webcrypto, ApiError, persistRetryMessages() {}, loadOperations:async()=>{},
    attachments: { readyImages: () => [...images.value], clear: (key, ids) => { cleared.push({ key, ids }); } },
    request: async (url, options) => {
      const call = { url, options, body: JSON.parse(options.body) };
      requests.push(call);
      return respond(call, requests.length);
    },
    openThread: (thread) => { opened.push(thread); state.newChat.value = false; state.selectedId.value = thread.id; },
    loadThreads: async () => {},
  };
  const scope = effectScope();
  const app = scope.run(() => new Function(...Object.keys(bindings), compiled
    + '\nreturn { createChat, draft, draftKey, sending, creationSource, createdDraft, firstTurnPending };')(...Object.values(bindings)));
  t.after(() => scope.stop());
  return { ...state, ...app, requests, cleared, opened, images };
}

test('新建未验收时不请求、不清空草稿与图片、不产生临时对话', async t => {
  const app = fixture(t);
  await app.createChat();
  app.draft.value = '修改后的草稿';
  await app.createChat();
  assert.equal(app.requests.length, 0);
  assert.equal(app.draft.value, '修改后的草稿');
  assert.equal(app.images.value.length, 1);
  assert.deepEqual(app.cleared, []);
  assert.deepEqual(app.opened, []);
  assert.equal(app.error.value, '');
});

test('新建只绑定明确来源和官方项目，正式 ID 与首轮确认分开且保留草稿', async t => {
  const app = fixture(t, async()=>({threadId:'created-chat',firstTurnAccepted:false}));
  app.capabilities.value.create=true; app.creationSource.value='chosen-source';
  await app.createChat();
  assert.equal(app.requests.length,1);
  assert.equal(app.requests[0].body.sourceThreadId,'chosen-source');
  assert.equal(app.requests[0].body.projectId,'project-a');
  assert.equal(app.requests[0].body.model,undefined);
  assert.equal(app.selectedId.value,'created-chat');
  assert.equal(app.firstTurnPending.value,true);
  assert.equal(app.drafts['new:project-a'],'检查手机布局');
  assert.equal(app.capabilities.value.send,false);
});

test('创建结果未知时改写文本也不重复创建，投递前失败才允许新意图重试', async t => {
  const error = new ApiError('回执丢失',503); error.outcome='unknown';
  const app = fixture(t, async()=>{throw error;});
  app.capabilities.value.create=true; app.creationSource.value='chosen-source';
  await app.createChat(); app.draft.value='新的文字'; await app.createChat();
  assert.equal(app.requests.length,1); assert.equal(app.opened.length,0);
  assert.equal(app.draft.value,'新的文字');
});
