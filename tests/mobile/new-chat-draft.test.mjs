import { submissionFixture, emptyOperations } from '../fixtures/submission.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import ts from 'typescript';
import { computed, effectScope, reactive, ref } from 'vue';
import { ApiError } from '../../apps/mobile/src/api.ts';

// Execute the page's actual draft bindings and creation handler with HTTP/navigation doubles.
const source = readFileSync(new URL('../../apps/mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const declarations = new Set(['retryMessages', 'pendingSends', 'sending', 'activeProject', 'draftKey', 'draft', 'modelSelection', 'creationSource', 'createdDraft', 'firstTurnPending', 'sendDisabledReason']);
const selected = ast.statements.filter((node) => {
  if (ts.isFunctionDeclaration(node)) return ['createChat', 'restoreCreation', 'submissionReason', 'send', 'sendMessage', 'applySnapshot'].includes(node.name?.text);
  return ts.isVariableStatement(node) && node.declarationList.declarations.some((entry) => declarations.has(entry.name.getText(ast)));
}).map((node) => node.getText(ast)).join('\n');
const compiled = ts.transpileModule(selected, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, respond = async () => ({ threadId: 'created-chat' })) {
  const requests = [], cleared = [], opened = [];
  const state = {
    authenticated: ref(true), resumableTurn: ref(),
    projects: ref([{ projectId: 'project-a', label: '项目 A', path: '/project-a' }]),
    selectedProjectId: ref('project-a'), selectedId: ref(''), newChat: ref(true),
    drafts: reactive({ 'new:project-a': '检查手机布局' }),
    modelDrafts: reactive({}), operationsReady: ref(true), unresolvedOperations: ref([]),
    imagesUploading: ref(false), imageErrors: ref(false), streamStatus: ref('connected'),
    active: ref(false), queueState: ref(null), pendingMessages: ref([]),
    turns: ref([]), historyCursor: ref(null), changedFileCount: ref(0), interactions: ref([]),
    error: ref(''), threads: ref([]), capabilities: ref({create:false, images:true}), notice:ref(''),
  };
  const images = ref([{ id: 'a'.repeat(32) + '.png', name: '手机截图.png', url: '/api/images/test' }]);
  const bindings = {
    readRetryMessages: () => [],
    ...state, computed, reactive, ref, crypto: webcrypto, ApiError, persistRetryMessages() {}, loadOperations:async()=>{},
    draftImages: images, hasMessage: computed(() => !!images.value.length || !!state.drafts[state.newChat.value ? 'new:project-a' : state.selectedId.value]),
    voice: { busy: ref(false) }, nextTick: async()=>{}, resizeComposer() {}, scrollBottom() {}, reconcilePending() {},
    location: { origin: 'http://localhost' }, threadUrl: (id, route) => '/api/threads/' + id + '/' + route,
    messageQueue: { acceptSnapshot() {} }, mergeTurns: (previous, next) => next, updateThread() {}, historyGap: null, historyRequest: null,
    attachments: { readyImages: () => [...images.value], clear: (key, ids) => { cleared.push({ key, ids }); } },
    request: async (url, options) => {
      const call = { url, options, body: JSON.parse(options.body) };
      requests.push(call);
      return respond(call, requests.length);
    },
    openThread: (thread) => { opened.push(thread); state.newChat.value = false; state.selectedId.value = thread.id; },
    loadThreads: async () => {},
  };
  const submissions = submissionFixture(t, (call, init) => call.url.startsWith('/api/operations') ? emptyOperations(call.url) : bindings.request(call.url, init));
  submissions.operationsReady.value = true;
  Object.assign(bindings, { submissions, recoveredCreations: submissions.recoveredCreations, operationsReady: submissions.operationsReady, unresolvedOperations: submissions.unresolvedOperations });
  const scope = effectScope();
  const app = scope.run(() => new Function(...Object.keys(bindings), compiled
    + '\nreturn { createChat, restoreCreation, recoveredCreations, send, applySnapshot, draft, draftKey, modelSelection, sending, creationSource, createdDraft, firstTurnPending };')(...Object.values(bindings)));
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

test('纯图新建携带图片和明确选择，首轮确认只清理本次提交内容', async t => {
  const app = fixture(t, async()=>({threadId:'created-chat',firstTurnAccepted:false}));
  app.capabilities.value.create = true; app.creationSource.value = 'chosen-source';
  app.draft.value = '';
  app.modelSelection.value = { model: 'selected-model', thinking: 'high' };
  await app.createChat();
  assert.equal(app.requests[0].body.prompt, '');
  assert.deepEqual(app.requests[0].body.images, ['a'.repeat(32) + '.png']);
  assert.equal(app.requests[0].body.model, 'selected-model');
  assert.equal(app.requests[0].body.thinking, 'high');
  assert.deepEqual(app.cleared, []);
  app.drafts['new:project-a'] = '之后补充的内容';
  app.modelDrafts['new:project-a'] = { model: 'next-model' };
  const snapshot = { thread: {id:'created-chat'}, turns:[], syncMode:'desktop-poll', firstTurnPending:true };
  app.applySnapshot(snapshot);
  assert.deepEqual(app.cleared, []);
  app.applySnapshot({...snapshot, firstTurnPending:false});
  assert.deepEqual(app.cleared, [{key:'new:project-a',ids:['a'.repeat(32) + '.png']}]);
  assert.equal(app.drafts['new:project-a'], '之后补充的内容');
  assert.deepEqual(app.modelDrafts['new:project-a'], {model:'next-model'});
  assert.equal(app.createdDraft.value, null);
});

test('已有聊天文图发送透传模型，接受后清空本次草稿', async t => {
  const app = fixture(t, async()=>({accepted:true}));
  app.newChat.value = false; app.selectedId.value = 'chat';
  app.capabilities.value = {send:true, images:true, modelSelection:true, native:true};
  app.draft.value = '检查图片';
  app.modelSelection.value = {thinking:'high'};
  await app.send();
  assert.equal(app.requests.length, 1);
  assert.equal(app.requests[0].body.model, undefined);
  assert.equal(app.requests[0].body.thinking, 'high');
  assert.equal(app.requests[0].body.mode, 'now');
  assert.equal(app.requests[0].body.images.length, 1);
  assert.equal(app.draft.value, '');
  assert.deepEqual(app.modelSelection.value, {});
  assert.deepEqual(app.cleared, [{key:'chat',ids:['a'.repeat(32) + '.png']}]);
});

test('图片发送未知时保留草稿与模型并阻止重复请求', async t => {
  const failure = new ApiError('回执丢失',503); failure.outcome = 'unknown';
  const app = fixture(t, async()=>{throw failure;});
  app.newChat.value = false; app.selectedId.value = 'chat';
  app.capabilities.value = {send:true, images:true, native:true};
  app.draft.value = '检查图片'; app.modelSelection.value = {model:'selected-model'};
  await app.send(); await app.send();
  assert.equal(app.requests.length, 1);
  assert.equal(app.draft.value, '检查图片');
  assert.deepEqual(app.modelSelection.value, {model:'selected-model'});
  assert.deepEqual(app.cleared, []);
});

test('投递前失败允许重试，当前连接没有图片能力则保留图片', async t => {
  const failure = new ApiError('连接已断开',503); failure.outcome = 'not-dispatched';
  const app = fixture(t, async(_, count)=>{if(count === 1) throw failure; return {accepted:true};});
  app.newChat.value = false; app.selectedId.value = 'chat';
  app.capabilities.value = {send:true, native:true};
  await app.send(); assert.equal(app.requests.length, 0);
  app.capabilities.value.images = true;
  await app.send(); assert.deepEqual(app.cleared, []);
  await app.send(); assert.equal(app.requests.length, 2);
  assert.equal(app.cleared.length, 1);
});

test('新建回执迟到时不抢走后续导航，并留下查看原聊天的恢复入口', async t => {
  let complete;
  const app = fixture(t, () => new Promise(resolve => { complete = resolve; }));
  app.capabilities.value.create = true; app.creationSource.value = 'chosen-source';
  const pending = app.createChat();
  app.newChat.value = false; app.selectedId.value = 'another-chat';
  complete({ threadId: 'created-chat', firstTurnAccepted: false });
  await pending;
  assert.deepEqual(app.opened, []);
  assert.equal(app.selectedId.value, 'another-chat');
  assert.equal(app.drafts['new:project-a'], '检查手机布局');
  assert.equal(app.recoveredCreations.value[0].receipt.threadId, 'created-chat');
  app.restoreCreation(app.recoveredCreations.value[0]);
  assert.equal(app.selectedId.value, 'created-chat');
  assert.equal(app.firstTurnPending.value, true);
  assert.equal(app.drafts['new:project-a'], '检查手机布局');
});

test('查看已创建聊天后首轮未确认，返回同项目也不能再次创建', async t => {
  const app = fixture(t, async () => ({ threadId: 'created-chat', firstTurnAccepted: false }));
  app.capabilities.value.create = true; app.creationSource.value = 'chosen-source';
  await app.createChat();
  assert.equal(app.recoveredCreations.value.length, 1);
  app.newChat.value = true;
  app.draft.value = '后来编辑的草稿';
  await app.createChat();
  assert.equal(app.requests.length, 1);
  assert.equal(app.draft.value, '后来编辑的草稿');
  app.restoreCreation(app.recoveredCreations.value[0]);
  app.applySnapshot({ thread: { id: 'created-chat' }, turns: [], syncMode: 'desktop-poll', firstTurnPending: false });
  assert.equal(app.recoveredCreations.value.length, 0);
  assert.equal(app.drafts['new:project-a'], '后来编辑的草稿');
});

test('首轮字段缺失或无法核对时不清草稿图片和创建身份', async t => {
  const app = fixture(t, async () => ({ threadId: 'created-chat', firstTurnAccepted: false }));
  app.capabilities.value.create = true; app.creationSource.value = 'chosen-source';
  await app.createChat();
  app.applySnapshot({ thread: { id: 'created-chat' }, turns: [], syncMode: 'desktop-poll', capabilities: { send: false }, syncError: '账本暂时不可读' });
  assert.equal(app.firstTurnPending.value, true);
  assert.equal(app.recoveredCreations.value.length, 1);
  assert.equal(app.drafts['new:project-a'], '检查手机布局');
  assert.deepEqual(app.cleared, []);
  assert.ok(app.createdDraft.value);
});
