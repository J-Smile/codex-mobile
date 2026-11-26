import { submissionFixture, emptyOperations } from '../fixtures/submission.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import ts from 'typescript';
import { computed, effectScope, isReactive, nextTick, reactive, ref, shallowRef, watch } from 'vue';
import { threadUrl } from '../../apps/mobile/src/api.ts';
import { mergeStreamUpdate } from '../../apps/mobile/src/features/conversation/useThreadStream.ts';
import { useVoiceInput } from '../../apps/mobile/src/features/composer/useVoiceInput.ts';

// 直接编译页面的相关脚本与切换监听，验证真实接线，不另写一份历史或发送实现。
const source = readFileSync(new URL('../../apps/mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const methods = new Set(['mergeTurns', 'cancelHistory', 'reconcilePending', 'applySnapshot', 'applyUpdate', 'loadOlder', 'activateConversation', 'openProjects', 'send', 'sendMessage', 'resumeTurn', 'scrollBottom', 'onScroll', 'canReloadApp', 'preserveBeforeReload', 'submissionReason']);
const declarations = new Set(['turns', 'view', 'historyRequest', 'historyGap', 'restoringFrame', 'bottomScroll', 'bottomScrollKey', 'visibleConnectionError', 'projectError', 'visibleError', 'directSubagentInputError', 'subagentInputRejected', 'retryMessages', 'pendingSends', 'sending', 'draftKey', 'draft', 'hasMessage', 'active', 'resumableTurn', 'composerAction', 'conversationHistory', 'firstTurnPending', 'createdDraft', 'sendDisabledReason']);
const selected = ast.statements.filter(node => {
  if (ts.isFunctionDeclaration(node)) return methods.has(node.name?.text);
  if (ts.isVariableStatement(node)) return node.declarationList.declarations.some(entry => declarations.has(entry.name.getText(ast)));
  return ts.isExpressionStatement(node) && ts.isCallExpression(node.expression)
    && node.expression.expression.getText(ast) === 'watch'
    && ['selectedId', '[view, authenticated]'].includes(node.expression.arguments[0]?.getText(ast));
}).map(node => node.getText(ast)).join('\n');
const compiled = ts.transpileModule(selected, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, request, voice = { clear() {}, busy: ref(false) }) {
  const queueSnapshots = [];
  const state = {
    creationSource: ref('source'),
    capabilities: ref({ send: true, create: true, resume: true, interrupt: true, images: true }), operationsReady: ref(true), unresolvedOperations: ref([]),
    modelSelection: ref({}), modelDrafts: reactive({}),
    authenticated: ref(true), selectedId: ref('child'), selectedProjectId: ref('project'),
    selectedThread: ref({ id: 'child', title: '子对话' }), threads: ref([]),
    historyCursor: ref(null), changedFileCount: ref(undefined), loadingHistory: ref(false), historyError: ref(''),
    drafts: reactive({ child: '未发送的草稿' }), draftImages: ref([]), stopping: ref(false), streamStatus: ref('connected'), imagesUploading: ref(false), imageErrors: ref(false), queueState: ref(null),
    voice, composer: ref(null),
    pendingMessages: ref([]), interactions: ref([]), error: ref(''), connectionError: ref(''), catalogError: ref(''),
    newChat: ref(false), navigationOpen: ref(false), usageOpen: ref(false), stickToBottom: ref(false), newOutput: ref(false),
    delegationOpen: ref(false), contextOpen: ref(false), sendMenu: ref(false), chatMenu: ref(false),
    composerMenu: ref(false), notice: ref(''),
    booting: ref(false), settingsOpen: ref(false), updateSettingsOpen: ref(false), goalOpen: ref(false), modeBusy: ref(false),
    scanOpen: ref(false), projectPickerOpen: ref(false), queueBusy: ref(false),
  };
  state.scroller = ref({ scrollTop: 0, clientHeight: 600, get scrollHeight() { return state.turns.value.length * 240; } });
  const bindings = {
    readRetryMessages: () => [], persistRetryMessages() {}, loadOperations: async () => {}, ApiError: class ApiError extends Error {},
    ...state, knownThreads: state.threads, request, computed, reactive, ref, shallowRef, nextTick, watch, threadUrl, mergeStreamUpdate, props: {},
    location: { origin: 'https://example.test' },
    messageQueue: { acceptSnapshot: (...args) => queueSnapshots.push(args) },
    notifications: { busy: ref(false) }, desktopUpgrade: { reconnecting: ref(false) }, attachments: {
      hasPendingUploads: ref(false), canReload: ref(true), saveDrafts: () => true, readyImages: () => state.draftImages.value,
      clear: (threadId, ids) => {
        if (threadId === state.selectedId.value) state.draftImages.value = state.draftImages.value.filter(image => !ids.includes(image.id));
      },
    }, document: { querySelector: () => null },
    createChat: () => assert.fail('空输入不应创建对话'),
    useConversationHistory: options => options,
    updateThread: thread => { state.selectedThread.value = thread; },
    saveDrafts: () => true, saveModelDrafts: () => true,
    updateAddress() {}, alignNavigation() {}, resizeComposer() {}, preloadMarkdownPreview: async () => {},
    clearConversation: () => { state.selectedId.value = ''; state.turns.value = []; },
  };
  const submissions = submissionFixture(t, (call, init) => call.url.startsWith('/api/operations') ? emptyOperations(call.url) : request(call.url, init));
  submissions.operationsReady.value = true;
  Object.assign(bindings, { submissions, operationsReady: submissions.operationsReady, unresolvedOperations: submissions.unresolvedOperations });
  const scope = effectScope();
  const app = scope.run(() => new Function(...Object.keys(bindings), compiled
    + '\nreturn { ' + [...methods].join(', ') + ', turns, sending, pendingSends, draft, hasMessage, composerAction, resumableTurn, visibleError, projectError, subagentInputRejected, capture: conversationHistory.capture };')(...Object.values(bindings)));
  state.turns = app.turns;
  t.after(() => { app.cancelHistory(); scope.stop(); });
  return { ...app, ...state, queueSnapshots, attachments: bindings.attachments };
}

function turns(from, through, prefix = 'turn-') {
  return Array.from({ length: through - from + 1 }, (_, index) => ({ id: prefix + (from + index), status: 'completed', items: [] }));
}
const ids = entries => entries.map(entry => entry.id);
const page = (from, through, nextCursor) => ({ thread: { id: 'parent', title: '父对话' }, turns: turns(from, through), nextCursor });

test('原生通道忙时仍可追加文字，不使用 CLI 队列且图片草稿保留', async t => {
  const calls=[];
  const app=fixture(t,async(url,options)=>{calls.push(JSON.parse(options.body));return {queued:false};});
  app.capabilities.value={send:true,native:true,interrupt:false};
  app.turns.value=[{id:'busy',status:'inProgress',items:[]}];
  assert.equal(app.composerAction.value.label,'发送追加消息');
  assert.equal(app.composerAction.value.disabled,false);
  await app.send();
  assert.equal(calls[0].mode,'now');assert.equal(app.pendingMessages.value.length,0);
  app.draft.value='带图片';app.draftImages.value=[{id:'image',name:'test.png'}];
  await app.send();assert.equal(calls.length,1);assert.equal(app.draftImages.value.length,1);
  assert.equal(app.draft.value,'带图片');
});
const snapshot = (from, through, nextCursor) => ({ capabilities: { send: true, resume: true, interrupt: true }, ...page(from, through, nextCursor), connection: 'live' });
const saved = () => ({ threadId: 'parent', projectId: 'project', thread: { id: 'parent', title: '父对话' },
  turns: turns(3, 5), historyCursor: 'before-3', scrollTop: 582, stickToBottom: false, newOutput: false });
async function settled() { await setImmediate(); await nextTick(); }
function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

test('聊天正文按快照替换保持浅引用，追加和结束状态仍更新，旧阅读快照不被修改', async t => {
  const app = fixture(t, () => assert.fail('本地合并不发出请求'));
  const previous = { id: 'older', status: 'completed', items: [{ id: 'old-answer', type: 'agentMessage', text: '历史正文' }] };
  const current = { id: 'current', status: 'inProgress', items: [{ id: 'answer', type: 'agentMessage', text: '开始' }] };
  app.applySnapshot({ ...snapshot(1, 2, null), turns: [previous, current] });
  const reading = app.capture();
  assert.equal(app.turns.value[0], previous);
  assert.equal(isReactive(app.turns.value[1].items[0]), false);
  assert.equal(app.composerAction.value.disabled, false);
  app.applyUpdate({ connection: 'live', textAppends: [{ turnId: 'current', itemId: 'answer', offset: 2, text: '，继续' }] });
  await nextTick();
  assert.equal(app.turns.value[0], previous);
  assert.equal(app.turns.value[1].items[0].text, '开始，继续');
  assert.equal(reading.turns[1].items[0].text, '开始');
  app.draft.value = '';
  app.applyUpdate({ connection: 'live', thread: { id: 'child' }, turns: [{ id: 'current', status: 'completed', itemOrder: ['answer'] }], updates: [] });
  await nextTick();
  assert.equal(app.turns.value[1].status, 'completed');
  assert.equal(app.composerAction.value.disabled, true);
});

test('语音启动、听写或结束等待时离开聊天会释放识别，重进同一聊天可发送且保留草稿', async t => {
  for (const phase of ['starting', 'listening', 'stopping']) {
    for (const destination of ['projects', 'usage']) {
      const scope = effectScope();
      t.after(() => scope.stop());
      let recognition;
      class Recognition {
        aborted = false;
        constructor() { recognition = this; }
        start() {}
        stop() {}
        abort() { this.aborted = true; }
      }
      let app;
      const voice = scope.run(() => useVoiceInput({
        browser: { SpeechRecognition: Recognition },
        contextKey: () => app.selectedId.value,
        readDraft: () => app.draft.value,
        writeDraft: value => { app.draft.value = value; },
      }));
      app = fixture(t, () => assert.fail('恢复输入不应发出请求'), voice);
      voice.start();
      if (phase !== 'starting') recognition.onstart();
      if (phase === 'stopping') voice.stop();
      const lateResult = recognition.onresult;
      assert.equal(voice.phase.value, phase);
      assert.equal(app.composerAction.value.disabled, true);
      if (destination === 'projects') app.openProjects();
      else app.usageOpen.value = true;
      await nextTick();
      assert.equal(recognition.aborted, true);
      assert.equal(voice.phase.value, 'idle');
      await app.activateConversation('child', 'project');
      assert.equal(app.composerAction.value.disabled, false);
      assert.equal(app.canReloadApp(), true);
      assert.equal(app.draft.value, '未发送的草稿');
      app.draft.value += '，继续输入';
      lateResult({ results: [[{ transcript: '迟到的识别' }]] });
      assert.equal(app.draft.value, '未发送的草稿，继续输入');
    }
  }
});

test('目录同步失败只在项目页提示，实时聊天继续显示真实的操作错误', t => {
  const app = fixture(t, () => assert.fail('仅检查提示状态'));
  app.catalogError.value = '项目读取失败';
  app.connectionError.value = '已恢复的旧连接错误';
  assert.equal(app.projectError.value, '项目读取失败');
  assert.equal(app.visibleError.value, '');
  app.error.value = 'direct app-server input is not allowed for multi-agent v2 sub-agents';
  assert.equal(app.subagentInputRejected.value, true);
  assert.equal(app.visibleError.value, '');
  app.streamStatus.value = 'disconnected';
  assert.equal(app.visibleError.value, '已恢复的旧连接错误');
});



test('已完成、主动停止、运行中、旧轮次和断线状态不发起恢复', async t => {
  const app = fixture(t, () => assert.fail('不应发送恢复请求'));
  app.draft.value = '';
  for (const status of ['completed', 'interrupted', 'inProgress']) {
    app.turns.value = [{ id: 'old', status: 'failed', items: [] }, { id: 'turn', status, items: [] }];
    assert.equal(app.resumableTurn.value, undefined);
    if (status !== 'inProgress') assert.equal(app.composerAction.value.disabled, true);
    await app.resumeTurn('turn');
    await app.send();
  }
  app.turns.value = [{ id: 'old', status: 'failed', items: [] }, { id: 'latest', status: 'failed', items: [] }];
  await app.resumeTurn('old');
  app.streamStatus.value = 'disconnected';
  assert.equal(app.composerAction.value.disabled, true);
  await app.resumeTurn('latest');
  await app.send();
});

test('未开放的异常继续入口保持禁用，输入新消息后仍能正常发送', async t => {
  const calls = [];
  const app = fixture(t, async (url, options) => { calls.push({ url, body: JSON.parse(options.body) }); return { accepted: true }; });
  app.capabilities.value = { send: true, native: true, resume: false, images: true };
  app.turns.value = [{ id: 'failed-turn', status: 'failed', items: [] }];
  app.draft.value = '';
  assert.equal(app.composerAction.value.disabled, true);
  await app.send();
  assert.equal(calls.length, 0);
  app.draft.value = '新的明确消息';
  assert.equal(app.composerAction.value.disabled, false);
  await app.send();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].body.prompt, '新的明确消息');
});

test('异常任务有文字或图片时按原规则发送或排队，不触发空输入恢复', async t => {
  const image = { id: 'c'.repeat(32) + '.png', url: '/api/images/photo.png', name: '截图.png', size: 12, type: 'image/png' };
  for (const queued of [false, true]) {
    for (const imagesOnly of [false, true]) {
      const calls = [];
      const app = fixture(t, async (url, options) => { calls.push({ url, body: JSON.parse(options.body) }); return {}; });
      app.turns.value = [{ id: 'failed-turn', status: 'failed', items: [] }];
      app.draft.value = imagesOnly ? '' : '请先检查报错原因';
      app.draftImages.value = imagesOnly ? [image] : [];
      app.queueState.value = { messages: queued ? [{ id: 'waiting' }] : [] };
      assert.equal(app.composerAction.value.disabled, false);
      assert.equal(app.composerAction.value.label, queued ? '加入队列' : '发送消息');
      await app.send();
      assert.equal(calls.length, 1);
      assert.equal(calls[0].url, '/api/threads/child/messages');
      assert.equal(calls[0].body.prompt, imagesOnly ? '' : '请先检查报错原因');
      assert.deepEqual(calls[0].body.images, imagesOnly ? [image.id] : []);
      assert.equal(calls[0].body.mode, queued ? 'queue' : 'now');
      assert.equal(app.draft.value, '');
      assert.deepEqual(app.draftImages.value, []);
    }
  }
});

test('新对话、语音识别和附件未就绪时，空输入发送不能恢复旧任务', async t => {
  const app = fixture(t, () => assert.fail('不应发送恢复请求'));
  app.draft.value = '';
  app.turns.value = [{ id: 'failed-turn', status: 'failed', items: [] }];
  for (const state of [app.newChat, app.voice.busy, app.imagesUploading, app.imageErrors]) {
    state.value = true;
    assert.equal(app.composerAction.value.disabled, true);
    await app.send();
    state.value = false;
  }
  app.selectedId.value = '';
  assert.equal(app.composerAction.value.disabled, true);
  await app.send();
});

test('返回父对话补齐多页间隔，保留阅读位置、原游标和补页期间的实时输出', async t => {
  const first = deferred();
  const requests = [];
  const app = fixture(t, async (url, options) => {
    requests.push({ url, signal: options.signal });
    if (requests.length === 1) return first.promise;
    if (requests.length === 2) return page(6, 10, 'before-6');
    return page(1, 5, null);
  });
  await app.activateConversation('parent', 'project', saved());
  app.applySnapshot(snapshot(16, 20, 'before-16'));
  assert.equal(app.scroller.value.scrollTop, 582);
  assert.equal(app.historyCursor.value, 'before-3');
  assert.deepEqual(ids(app.capture().turns), ids(turns(3, 5)));
  app.applyUpdate({ connection: 'live', thread: { id: 'parent', title: '父对话' },
    turns: [{ id: 'turn-20', status: 'completed', itemOrder: ['answer'] }, { id: 'turn-21', status: 'inProgress', itemOrder: ['latest'] }],
    updates: [{ turnId: 'turn-20', item: { id: 'answer', type: 'agentMessage', text: '实时最终结果' } },
      { turnId: 'turn-21', item: { id: 'latest', type: 'agentMessage', text: '新的实时输出' } }] });
  app.applyUpdate({ connection: 'live', textAppends: [{ turnId: 'turn-21', itemId: 'latest', offset: '新的实时输出'.length, text: '🙂' }] });
  first.resolve(page(11, 15, 'before-11'));
  await settled();
  assert.deepEqual(requests.map(entry => new URL(entry.url, 'https://example.test').searchParams.get('cursor')), ['before-16', 'before-11', 'before-6']);
  assert.deepEqual(ids(app.turns.value), ids(turns(3, 21)));
  assert.equal(app.turns.value.find(turn => turn.id === 'turn-20').items[0].text, '实时最终结果');
  assert.equal(app.turns.value.at(-1).items[0].text, '新的实时输出🙂');
  assert.equal(app.selectedThread.value.id, 'parent');
  assert.equal(app.historyCursor.value, 'before-3');
  assert.equal(app.scroller.value.scrollTop, 582);
  assert.equal(app.stickToBottom.value, false);
  assert.equal(app.loadingHistory.value, false);
  assert.deepEqual(ids(app.capture().turns), ids(turns(3, 21)));
});

test('重叠快照不额外补页，普通加载更早消息仍保持位置并使用原分页游标', async t => {
  const requests = [];
  const app = fixture(t, async url => { requests.push(url); return page(1, 2, null); });
  await app.activateConversation('parent', 'project', saved());
  app.applySnapshot(snapshot(5, 9, 'before-5'));
  await settled();
  assert.equal(requests.length, 0);
  assert.equal(app.historyCursor.value, 'before-3');
  const top = app.scroller.value.scrollTop;
  await app.loadOlder();
  assert.deepEqual(ids(app.turns.value), ids(turns(1, 9)));
  assert.match(requests[0], /cursor=before-3$/);
  assert.equal(app.historyCursor.value, null);
  assert.equal(app.scroller.value.scrollTop, top + 2 * 240);
});

test('切回聊天第一帧已有缓存正文，文件数只根据所属聊天的服务器摘要更新', async t => {
  const app = fixture(t, () => assert.fail('打开缓存不需要先请求历史'));
  const cached = { ...saved(), changedFileCount: 7 };
  const switching = app.activateConversation('parent', 'project', cached);
  assert.deepEqual(ids(app.turns.value), ids(cached.turns));
  assert.equal(app.changedFileCount.value, 7);
  await switching;
  app.applyUpdate({ connection: 'live', thread: { id: 'parent' }, turns: [], updates: [], changedFileCount: 8 });
  assert.equal(app.capture().changedFileCount, 8);
  await app.activateConversation('other', 'project');
  app.applySnapshot(snapshot(6, 10, 'before-6'));
  assert.equal(app.changedFileCount.value, undefined);
});

test('补页途中离开再回到同一父对话，旧响应与下一帧回调都不能写入新视图', async t => {
  const pending = deferred();
  let signal;
  const app = fixture(t, (_, options) => { signal = options.signal; return pending.promise; });
  await app.activateConversation('parent', 'project', saved());
  app.applySnapshot(snapshot(16, 20, 'before-16'));
  const frame = app.capture();
  await app.activateConversation('other', 'project');
  assert.equal(signal.aborted, true);
  await app.activateConversation('parent', 'project', { ...frame, scrollTop: 271 });
  pending.resolve(page(11, 15, 'before-11'));
  await settled();
  assert.deepEqual(ids(app.turns.value), ids(turns(3, 5)));
  assert.equal(app.historyCursor.value, 'before-3');
  assert.equal(app.scroller.value.scrollTop, 271);
  assert.equal(app.loadingHistory.value, false);
  assert.equal(app.error.value, '');
});

test('补页失败保留已读缓存，重连快照沿未完成游标继续，重复轮次保持顺序', async t => {
  const requests = [];
  let failed = false;
  const app = fixture(t, async url => {
    const cursor = new URL(url, 'https://example.test').searchParams.get('cursor');
    requests.push(cursor);
    if (cursor === 'before-16') return page(11, 15, 'before-11');
    if (cursor === 'before-11' && !failed) { failed = true; throw new Error('补页暂时失败'); }
    if (cursor === 'before-11') return page(6, 10, 'before-6');
    return page(1, 5, null);
  });
  await app.activateConversation('parent', 'project', saved());
  app.applySnapshot(snapshot(16, 20, 'before-16'));
  await settled();
  assert.match(app.historyError.value, /补页暂时失败/);
  assert.equal(app.error.value, '');
  assert.equal(app.loadingHistory.value, false);
  assert.deepEqual(ids(app.capture().turns), ids(turns(3, 5)));
  assert.equal(app.historyCursor.value, 'before-3');
  app.applySnapshot(snapshot(16, 20, 'before-16'));
  await settled();
  assert.deepEqual(requests, ['before-16', 'before-11', 'before-11', 'before-6']);
  assert.deepEqual(ids(app.turns.value), ids(turns(3, 20)));
  assert.equal(app.scroller.value.scrollTop, 582);
  assert.equal(app.historyError.value, '');
});

test('手动重试更早消息清除历史提示，保留其他操作错误', async t => {
  const attempts = [];
  const app = fixture(t, async url => {
    attempts.push(url);
    if (attempts.length === 1) throw new Error('历史读取失败');
    return page(1, 2, null);
  });
  await app.activateConversation('parent', 'project', saved());
  app.error.value = '另一个操作未完成';
  await app.loadOlder();
  assert.equal(app.historyError.value, '历史读取失败');
  await app.loadOlder();
  assert.equal(app.historyError.value, '');
  assert.equal(app.error.value, '另一个操作未完成');
  assert.deepEqual(ids(app.turns.value), ids(turns(1, 5)));
  assert.deepEqual(attempts, ['/api/threads/parent/history?cursor=before-3', '/api/threads/parent/history?cursor=before-3']);
});

test('补页时又收到无重叠的最新快照，取消旧读取并一直补到原缓存边界', async t => {
  const first = deferred();
  const requests = [];
  const app = fixture(t, async (url, options) => {
    const cursor = new URL(url, 'https://example.test').searchParams.get('cursor');
    requests.push({ cursor, signal: options.signal });
    if (requests.length === 1) return first.promise;
    const upper = Number(cursor.slice('before-'.length)) - 1;
    return page(upper - 4, upper, upper > 5 ? 'before-' + (upper - 4) : null);
  });
  await app.activateConversation('parent', 'project', saved());
  app.applySnapshot(snapshot(16, 20, 'before-16'));
  app.applySnapshot(snapshot(28, 32, 'before-28'));
  await settled();
  first.resolve(page(11, 15, 'before-11'));
  await settled();
  assert.equal(requests[0].signal.aborted, true);
  assert.deepEqual(ids(app.turns.value), ids(turns(3, 32)));
  assert.equal(app.historyCursor.value, 'before-3');
  assert.equal(app.scroller.value.scrollTop, 582);
  assert.equal(app.loadingHistory.value, false);
});

test('未知发送结果阻止同内容和改写内容再次投递，保留草稿', async t => {
  let calls = 0;
  const app = fixture(t, async () => { calls++; throw new Error('响应丢失'); });
  await assert.rejects(app.sendMessage('内容', 'now'), /响应丢失/);
  await assert.rejects(app.sendMessage('内容', 'now'), /尚未确认/);
  await assert.rejects(app.sendMessage('修改内容', 'queue'), /尚未确认/);
  assert.equal(calls, 1);
  assert.equal(app.draft.value, '未发送的草稿');
  assert.equal(app.composerAction.value.disabled, true);
});

test('只读能力阻止发送与续接但保留阅读和草稿', async t => {
  let calls = 0;
  const app = fixture(t, async () => { calls++; });
  app.capabilities.value = {};
  await app.send();
  await assert.rejects(app.sendMessage('内容', 'now'), /草稿会保留/);
  await app.resumeTurn('failed');
  assert.equal(calls, 0);
  assert.equal(app.draft.value, '未发送的草稿');
  assert.equal(app.composerAction.value.disabled, true);
});

test('尚未同步的纯图片发送保留等待状态，后续原生回执才移除', async t => {
  let submitted;
  const app = fixture(t, async (_, options) => { submitted = JSON.parse(options.body); return { accepted: true }; });
  await app.activateConversation('parent', 'project', saved());
  const image = { id: 'b'.repeat(32) + '.png', url: '/api/images/' + 'b'.repeat(32) + '.png', name: '照片.png', size: 12, type: 'image/png' };
  await app.sendMessage('', 'now', [image]);
  assert.equal(app.pendingMessages.value.length, 1);
  assert.equal(app.pendingMessages.value[0].accepted, true);
  app.applySnapshot({ thread: { id: 'parent', title: '父对话' }, connection: 'live',
    turns: [{ id: 'turn-5', status: 'completed', items: [{ type: 'userMessage', id: submitted.requestId }] }] });
  assert.equal(app.pendingMessages.value.length, 0);
});

test('桌面入队回执无需等待队列快照即可移除本地发送气泡', async t => {
  const response = deferred();
  const app = fixture(t, () => response.promise);
  const send = app.sendMessage('稍后执行', 'now');
  assert.equal(app.pendingMessages.value.length, 1);
  response.resolve({ queued: true });
  await send;
  assert.equal(app.pendingMessages.value.length, 0);
  assert.deepEqual(app.queueSnapshots, []);
  assert.equal(app.notice.value, '已加入待执行');
  assert.equal(app.sending.value, false);
});

test('带图发送等待只影响本聊天，清空草稿仍可停止，切换发送不丢失原重试且更新等待所有请求', async t => {
  const first = deferred(), second = deferred(), posts = [];
  const queue = { revision: 'a'.repeat(64), messages: [] };
  const app = fixture(t, async (url, options) => {
    posts.push({ url, ...JSON.parse(options.body) });
    return posts.length === 1 ? first.promise : posts.length === 2 ? second.promise : { queued: true, queue };
  });
  const image = { id: 'c'.repeat(32) + '.png', url: '/api/images/photo.png', name: '截图.png', size: 12, type: 'image/png' };
  app.turns.value = [{ id: 'active-A', status: 'inProgress', items: [] }];
  const firstSend = app.sendMessage('A 的图片', 'queue', [image]);
  assert.equal(app.sending.value, true);
  app.draft.value = '';
  assert.equal(app.composerAction.value.kind, 'stop');
  assert.equal(app.composerAction.value.disabled, false);

  app.selectedId.value = 'second';
  await settled();
  app.draft.value = 'B 的文字';
  assert.equal(app.sending.value, false);
  assert.equal(app.composerAction.value.disabled, false);
  assert.equal(app.pendingMessages.value.length, 0);
  assert.equal(app.canReloadApp(), false);
  const secondSend = app.sendMessage('B 的文字', 'queue');
  assert.equal(app.sending.value, true);
  second.resolve({ queued: true, queue });
  await secondSend;
  assert.equal(app.sending.value, false);
  assert.equal(app.canReloadApp(), false);

  app.selectedId.value = 'child';
  await settled();
  app.turns.value = [{ id: 'active-A', status: 'inProgress', items: [] }];
  app.draft.value = '';
  assert.equal(app.sending.value, true);
  assert.equal(app.composerAction.value.kind, 'stop');
  assert.equal(app.composerAction.value.disabled, false);
  first.reject(new Error('A 的响应丢失'));
  await assert.rejects(firstSend, /响应丢失/);
  await assert.rejects(app.sendMessage('A 的图片', 'queue', [image]), /尚未确认/);
  assert.equal(posts.length, 2);
  assert.equal(app.sending.value, false);
  assert.equal(app.canReloadApp(), true);
});

test('只读问题可重载网页，可回复问题、打开面板和正在发送仍保留页面', t => {
  const app = fixture(t, () => assert.fail('刷新判断不应发送请求'));
  app.interactions.value = [{ id: 'read-only-question', replySupported: false }];
  assert.equal(app.canReloadApp(), true);
  app.interactions.value.push({ id: 'replyable-question', replySupported: true });
  assert.equal(app.canReloadApp(), false);
  app.capabilities.value.interactions = false;
  app.streamStatus.value = 'disconnected';
  assert.equal(app.canReloadApp(), false, '断线不丢弃可回复问题的答案草稿');
  app.interactions.value.pop();
  app.settingsOpen.value = true;
  assert.equal(app.canReloadApp(), false);
  app.settingsOpen.value = false;
  app.pendingSends.add('another-chat');
  assert.equal(app.canReloadApp(), false);
  app.pendingSends.clear();
  assert.equal(app.canReloadApp(), true);
});

test('网页重载使用全部图片草稿的可恢复性，已保存的失效引用不锁更新', t => {
  const app = fixture(t, () => assert.fail('刷新判断不应发送请求'));
  app.attachments.canReload.value = false;
  assert.equal(app.canReloadApp(), false, '另一聊天尚有只保存在内存中的图片');
  app.attachments.canReload.value = true;
  app.imageErrors.value = true;
  assert.equal(app.canReloadApp(), true, '已持久化失效引用可在刷新后恢复');
});

test('刷新前确认图片草稿保存成功，存储失败时保留文字与模型草稿', t => {
  const app = fixture(t, () => assert.fail('保存草稿不应投递消息'));
  app.modelDrafts.child = { model: 'fixture-model', thinking: 'high' };
  let attempts = 0;
  app.attachments.saveDrafts = () => ++attempts > 1;
  assert.equal(app.preserveBeforeReload(), false);
  assert.equal(app.draft.value, '未发送的草稿');
  assert.deepEqual(app.modelDrafts.child, { model: 'fixture-model', thinking: 'high' });
  assert.equal(app.preserveBeforeReload(), true);
  assert.equal(attempts, 2);
});

test('离开底部即提供返回入口，阅读时保留位置，点击后立即恢复输出跟随', async t => {
  const app = fixture(t, async () => ({}));
  app.turns.value = turns(1, 20);
  app.scroller.value.scrollTop = 25;
  app.onScroll();
  assert.equal(app.stickToBottom.value, false);
  assert.equal(app.newOutput.value, true);
  app.applySnapshot({ thread: { id: 'child' }, turns: turns(20, 21), connection: 'live' });
  await settled();
  assert.equal(app.scroller.value.scrollTop, 25);
  const jump = app.scrollBottom(true);
  assert.equal(app.stickToBottom.value, true);
  await jump;
  assert.equal(app.scroller.value.scrollTop, app.scroller.value.scrollHeight);
  assert.equal(app.newOutput.value, false);
  app.applySnapshot({ thread: { id: 'child' }, turns: turns(21, 22), connection: 'live' });
  await settled();
  assert.equal(app.scroller.value.scrollTop, app.scroller.value.scrollHeight);
});

const navigationMethods = new Set([
  'onNavigationScroll', 'onNavigationScrollEnd', 'measureNavigation', 'alignNavigation',
  'onViewportResize', 'resizeComposer', 'finishUsageExit', 'onNavigationPointerDown',
]);
const navigationDeclarations = new Set([
  'view', 'navigationStart', 'chatStart', 'usageStart', 'navigationTarget', 'navigationTimer', 'navigationTouching', 'catalogScope',
]);
const navigationScript = ast.statements.filter(node => {
  if (ts.isFunctionDeclaration(node)) return navigationMethods.has(node.name?.text);
  return ts.isVariableStatement(node)
    && node.declarationList.declarations.some(entry => navigationDeclarations.has(entry.name.getText(ast))
      || entry.initializer?.getText(ast).startsWith('useAppViewport('));
}).map(node => node.getText(ast)).join('\n');
const viewportScript = readFileSync(new URL('../../apps/mobile/src/useAppViewport.ts', import.meta.url), 'utf8')
  .replace(/^import .*\r?\n/, '').replace('export function', 'function');
const compiledNavigation = ts.transpileModule(navigationScript + '\n' + viewportScript, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;

function navigationFixture({ nativeScrollEnd = true, initialView = 'chat', hasConversation = true,
  initialMeasure = true, scrollLeft = initialView === 'chat' ? 390 : 0, paddingLeft = 0 } = {}) {
  const calls = [], frames = new Map();
  let frameId = 0;
  const measurements = { styleReads: 0, offsetReads: 0, composerReads: 0, heights: [], scrolls: [] };
  const layout = { width: 390, paddingLeft, hasConversation };
  const navigationPane = { get offsetLeft() { measurements.offsetReads++; return layout.paddingLeft; } };
  const chatPane = { get offsetLeft() { measurements.offsetReads++; return layout.hasConversation ? layout.width + layout.paddingLeft : 0; } };
  const usageReturnToChat = ref(false), usagePresented = ref(initialView === 'usage');
  const usagePane = { get offsetLeft() { measurements.offsetReads++; return layout.width * (usageReturnToChat.value && layout.hasConversation ? 2 : 1) + layout.paddingLeft; } };
  const shell = ref({
    scrollLeft, ...(nativeScrollEnd ? { onscrollend: null } : {}),
    querySelector: selector => selector === '.navigation-pane' ? navigationPane : selector === '.usage-pane' ? usagePane : chatPane,
    scrollTo(options) { measurements.scrolls.push(options); this.scrollLeft = options.left; },
  });
  const navigationOpen = ref(initialView !== 'chat'), usageOpen = ref(initialView === 'usage');
  const selectedId = ref(hasConversation ? 'thread' : ''), newChat = ref(false);
  const viewport = Object.assign(new EventTarget(), { width: 390, height: 844, offsetTop: 0 });
  const viewportStyles = new Map();
  const document = Object.assign(new EventTarget(), {
    activeElement: null, hidden: false,
    documentElement: {
      clientHeight: 844,
      style: {
        setProperty: (name, value) => viewportStyles.set(name, value),
        removeProperty: name => viewportStyles.delete(name),
      },
    },
  });
  const window = Object.assign(new EventTarget(), {
    visualViewport: viewport, innerWidth: 390, innerHeight: 844, scrollY: 0,
    scrollTo({ top }) { this.scrollY = top; },
    setTimeout, clearTimeout,
    requestAnimationFrame(callback) { const id = ++frameId; frames.set(id, callback); return id; },
    cancelAnimationFrame: id => frames.delete(id),
  });
  const composer = ref({ element: {
    isContentEditable: true,
    style: { set height(value) { measurements.heights.push(value); } },
    get scrollHeight() { measurements.composerReads++; return 80; },
  } });
  const bindings = {
    shell, navigationOpen, usageOpen, usagePresented, usageReturnToChat, nextTick, selectedId, newChat, composer, computed, ref,
    window, document, onMounted: callback => callback(), onScopeDispose() {},
    matchMedia: () => ({ matches: false }), updateAddress() {}, scrollBottom() {},
    getComputedStyle(element) {
      if (element?.isContentEditable) return { maxHeight: '150px' };
      measurements.styleReads++;
      return { paddingLeft: String(layout.paddingLeft) };
    },
    closeUsage() { calls.push('close-usage'); usageOpen.value = false; navigationOpen.value = !usageReturnToChat.value; },
    openProjects() { calls.push('projects'); usageOpen.value = false; navigationOpen.value = true; },
    backToPrevious() { calls.push('back'); navigationOpen.value = true; },
  };
  const app = new Function(...Object.keys(bindings), compiledNavigation
    + '\nreturn { ' + [...navigationMethods].join(', ') + ', view, keyboardOpen };')(...Object.values(bindings));
  if (initialMeasure) app.measureNavigation();
  function clearMeasurements() {
    measurements.styleReads = 0;
    measurements.offsetReads = 0;
    measurements.composerReads = 0;
    measurements.heights.length = 0;
    measurements.scrolls.length = 0;
  }
  clearMeasurements();
  return { ...app, shell, navigationOpen, usageOpen, usagePresented, usageReturnToChat, selectedId, newChat, calls, layout,
    viewport, viewportStyles, document, window, composer, measurements, clearMeasurements,
    flushFrame() { const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback()); },
  };
}

test('原生 scrollend 等待手势结束，边界停顿和快速反向不会提前返回', t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = navigationFixture();
  app.shell.value.scrollLeft = 0;
  app.onNavigationScroll();
  t.mock.timers.tick(500);
  assert.equal(app.view.value, 'chat');
  assert.deepEqual(app.calls, []);

  app.shell.value.scrollLeft = 390;
  app.onNavigationScroll();
  t.mock.timers.tick(500);
  app.onNavigationScrollEnd();
  assert.deepEqual(app.calls, []);

  app.shell.value.scrollLeft = 0;
  app.onNavigationScroll();
  app.onNavigationScrollEnd();
  app.onNavigationScrollEnd();
  assert.equal(app.view.value, 'projects');
  assert.deepEqual(app.calls, ['back']);
});

test('不支持 scrollend 时重置吸附计时，采用反向后的最终页面', t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = navigationFixture({ nativeScrollEnd: false });
  app.shell.value.scrollLeft = 0;
  app.onNavigationScroll();
  t.mock.timers.tick(100);
  app.shell.value.scrollLeft = 390;
  app.onNavigationScroll();
  t.mock.timers.tick(179);
  assert.deepEqual(app.calls, []);
  t.mock.timers.tick(1);
  assert.equal(app.view.value, 'chat');

  app.shell.value.scrollLeft = 0;
  app.onNavigationScroll();
  t.mock.timers.tick(180);
  assert.equal(app.view.value, 'projects');
  assert.deepEqual(app.calls, ['back']);
});

test('原生滑动已经落位时，对齐不重新读布局或启动平滑滚动', () => {
  const app = navigationFixture();
  app.shell.value.scrollLeft = 391;
  app.alignNavigation(true);
  app.shell.value.scrollLeft = 0;
  app.onNavigationScrollEnd();
  assert.equal(app.view.value, 'projects');
  app.alignNavigation(true);
  assert.equal(app.measurements.styleReads, 0);
  assert.equal(app.measurements.offsetReads, 0);
  assert.deepEqual(app.measurements.scrolls, []);
});

test('根列表位于零点，首次显示聊天测量真实位置', () => {
  const app = navigationFixture({ initialView: 'projects', hasConversation: false,
    initialMeasure: false, scrollLeft: 0, paddingLeft: 12 });
  app.alignNavigation();
  assert.deepEqual(app.measurements.scrolls, []);

  app.clearMeasurements();
  app.layout.hasConversation = true;
  app.newChat.value = true;
  app.navigationOpen.value = false;
  app.alignNavigation(true);
  assert.ok(app.measurements.offsetReads > 0);
  assert.deepEqual(app.measurements.scrolls, [{ left: 390, behavior: 'smooth' }]);
});

test('键盘只改变高度时不打断横向拖动，宽度变化才重测并对齐页面', () => {
  const app = navigationFixture();
  app.flushFrame();
  app.clearMeasurements();

  app.document.activeElement = app.composer.value.element;
  app.shell.value.scrollLeft = 190;
  app.viewport.height = 540;
  app.viewport.offsetTop = 20;
  app.viewport.dispatchEvent(new Event('resize'));
  app.flushFrame();
  assert.equal(app.viewportStyles.get('--app-height'), '540px');
  assert.equal(app.viewportStyles.get('--viewport-top'), '20px');
  assert.equal(app.viewportStyles.get('--safe-bottom-visible'), '0px');
  assert.equal(app.shell.value.scrollLeft, 190);
  assert.equal(app.measurements.offsetReads, 0);
  assert.deepEqual(app.measurements.scrolls, []);

  app.clearMeasurements();
  app.viewport.width = 844;
  app.viewport.height = 390;
  app.window.innerHeight = app.document.documentElement.clientHeight = 390;
  app.layout.width = 640;
  app.window.dispatchEvent(new Event('resize'));
  app.flushFrame();
  assert.ok(app.measurements.offsetReads > 0);
  assert.deepEqual(app.measurements.scrolls, [{ left: 640, behavior: 'instant' }]);
});

test('键盘收起和切回前台后清除残留的视口偏移与高度', t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const app = navigationFixture();
  app.flushFrame();
  const composer = app.composer.value.element;
  app.document.activeElement = composer;
  Object.assign(app.viewport, { height: 540, offsetTop: 62 });
  app.viewport.dispatchEvent(new Event('resize'));
  app.flushFrame();
  assert.equal(app.keyboardOpen.value, true);
  assert.equal(app.viewportStyles.get('--viewport-top'), '62px');

  app.document.activeElement = null;
  app.document.dispatchEvent(new Event('focusout'));
  app.flushFrame();
  assert.equal(app.keyboardOpen.value, true);
  app.viewport.height = 844;
  app.window.scrollY = 62;
  app.viewport.dispatchEvent(new Event('resize'));
  app.flushFrame();
  assert.equal(app.keyboardOpen.value, false);
  assert.equal(app.viewportStyles.size, 0);
  assert.equal(app.window.scrollY, 0);

  app.document.activeElement = composer;
  app.viewport.height = 540;
  app.viewport.dispatchEvent(new Event('resize'));
  app.flushFrame();
  app.document.hidden = true;
  app.document.dispatchEvent(new Event('visibilitychange'));
  app.viewport.height = 844;
  app.viewport.dispatchEvent(new Event('scroll'));
  app.flushFrame();
  assert.equal(app.viewportStyles.get('--viewport-top'), '62px');
  app.document.hidden = false;
  app.window.dispatchEvent(new Event('pageshow'));
  app.flushFrame();
  assert.equal(app.keyboardOpen.value, false);
  assert.equal(app.viewportStyles.size, 0);
});

test('列表与用量页不测量隐藏输入框，回到聊天后仍按内容调整高度', () => {
  const app = navigationFixture({ initialView: 'projects' });
  app.resizeComposer();
  app.usageOpen.value = true;
  app.resizeComposer();
  assert.equal(app.measurements.composerReads, 0);
  assert.deepEqual(app.measurements.heights, []);

  app.usageOpen.value = false;
  app.navigationOpen.value = false;
  app.resizeComposer();
  assert.equal(app.measurements.composerReads, 1);
  assert.deepEqual(app.measurements.heights, ['80px']);
});

test('根列表右滑不打开用量，用量右滑遵循原入口', () => {
  const root = navigationFixture({ initialView: 'projects' });
  root.onNavigationScrollEnd();
  assert.deepEqual(root.calls, []);
  for (const fromChat of [false, true]) {
    const app = navigationFixture({ initialView: 'usage' });
    app.usageReturnToChat.value = fromChat;
    app.shell.value.scrollLeft = fromChat ? 390 : 0;
    app.onNavigationScrollEnd();
    assert.deepEqual(app.calls, ['close-usage']);
    assert.equal(app.view.value, fromChat ? 'chat' : 'projects');
    app.onNavigationScrollEnd();
    assert.deepEqual(app.calls, ['close-usage']);
    assert.equal(app.usagePresented.value, false);
  }
});

test('用量尚未滑入就返回时取消旧滚动，移除页面后仍对齐来源', async () => {
  const app = navigationFixture({ initialView: 'projects' });
  app.usagePresented.value = true;
  app.alignNavigation(true);
  assert.equal(app.usagePresented.value, false);
  assert.deepEqual(app.measurements.scrolls, [{ left: 0, behavior: 'instant' }]);
  await nextTick();
  assert.equal(app.shell.value.scrollLeft, 0);
  assert.equal(app.view.value, 'projects');
});

test('打开用量时忽略前一段滚动的结束事件，手势仍可中断进入动画', () => {
  const app = navigationFixture({ initialView: 'usage' });
  app.alignNavigation(true);
  app.shell.value.scrollLeft = 0;
  app.onNavigationScrollEnd();
  assert.equal(app.view.value, 'usage');
  assert.deepEqual(app.calls, []);
  app.onNavigationPointerDown({ pointerType: 'touch' });
  app.onNavigationScrollEnd();
  assert.equal(app.view.value, 'projects');
  assert.deepEqual(app.calls, ['close-usage']);
});
