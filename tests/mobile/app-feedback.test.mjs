import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { nextTick, ref } from 'vue';
import { ApiError, threadUrl } from '../../apps/mobile/src/api.ts';
import { submissionFixture, emptyOperations } from '../fixtures/submission.mjs';

const source = readFileSync(new URL('../../apps/mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const methods = new Set(['login', 'disconnectBrowser', 'clearConversation', 'stopTurn', 'openDesktop']);
const compiled = ts.transpileModule(ast.statements.filter(node => ts.isFunctionDeclaration(node) && methods.has(node.name?.text))
  .map(node => node.getText(ast)).join('\n'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t, request) {
  const state = {
    loggingIn: ref(false), loginKey: ref('fixture-key'), loginError: ref(''), loginFailureStatus: ref(null),
    disconnecting: ref(false), disconnectError: ref(''), disconnectFailure: ref({ scrollIntoView() {} }), authenticated: ref(true), settingsOpen: ref(true),
    selectedId: ref('fixture-chat'), selectedThread: ref({ id: 'fixture-chat' }), turns: ref([{ id: 'turn', status: 'inProgress' }]),
    stopping: ref(false), streamStatus: ref('connected'), error: ref(''), capabilities: ref({ desktopNavigation: false }),
    usageOpen: ref(false), historyCursor: ref('older'), changedFileCount: ref(3), newChat: ref(false),
    connectionError: ref(''), composerMenu: ref(false), chatMenu: ref(false), contextOpen: ref(false), notice: ref(''),
    interactions: ref([{ id: 'question' }]), interactionState: ref({ expanded: true, pending: true }),
    delegationOpen: ref(false), threads: ref([{ id: 'fixture-chat' }]),
    pendingMessages: ref([]), drafts: { 'fixture-chat': '保留的草稿' }, modelDrafts: { 'fixture-chat': {model:'selected-model'} },
  };
  const cleared = [];
  let entries = 0;
  const submissions = submissionFixture(t, call => emptyOperations(call.url));
  submissions.operationsReady.value = true;
  const bindings = { ...state, submissions, request, ApiError, threadUrl, nextTick, enterApp: async () => { entries++; },
    voice: { clear: () => cleared.push('voice') }, conversationHistory: { clear: () => cleared.push('history') },
    attachments: { clearAll: () => cleared.push('attachments') } };
  const actions = new Function(...Object.keys(bindings), compiled + '\nreturn { login, disconnectBrowser, stopTurn, openDesktop };')(...Object.values(bindings));
  return { ...state, ...actions, submissions, cleared, get entries() { return entries; } };
}

test('连接请求等待时显示忙碌，重复提交不重发，失败保留密钥并可再次连接', async t => {
  let reject;
  const calls = [];
  const f = fixture(t, (url, init) => {
    calls.push({ url, body: JSON.parse(init.body) });
    return calls.length === 1 ? new Promise((_, fail) => { reject = fail; }) : Promise.resolve({ ok: true });
  });
  const first = f.login();
  assert.equal(f.loggingIn.value, true);
  await f.login();
  assert.equal(calls.length, 1);
  reject(new ApiError('连接密钥不正确', 401));
  await first;
  assert.equal(f.loggingIn.value, false);
  assert.equal(f.loginKey.value, 'fixture-key');
  assert.equal(f.loginError.value, '连接密钥不正确');
  assert.equal(f.loginFailureStatus.value, 401);
  assert.equal(f.entries, 0);
  await f.login();
  assert.equal(f.entries, 1);
  assert.equal(f.loginError.value, '');
  assert.equal(f.loggingIn.value, false);
  assert.equal(calls.length, 2);
});

test('断开失败在设置内保留错误与草稿，重试成功后才清理会话', async t => {
  let reject;
  let calls = 0;
  const f = fixture(t, () => ++calls === 1 ? new Promise((_, fail) => { reject = fail; }) : Promise.resolve({ ok: true }));
  const first = f.disconnectBrowser();
  assert.equal(f.disconnecting.value, true);
  await f.disconnectBrowser();
  assert.equal(calls, 1);
  reject(new ApiError('暂时无法断开，请重试', 503));
  await first;
  assert.equal(f.disconnecting.value, false);
  assert.equal(f.disconnectError.value, '暂时无法断开，请重试');
  assert.equal(f.settingsOpen.value, true);
  assert.equal(f.authenticated.value, true);
  assert.equal(f.drafts['fixture-chat'], '保留的草稿');
  assert.deepEqual(f.interactionState.value, { expanded: true, pending: true });
  assert.deepEqual(f.cleared, ['voice']);
  assert.equal(f.submissions.operationsReady.value, true);
  await f.disconnectBrowser();
  assert.equal(f.disconnectError.value, '');
  assert.equal(f.settingsOpen.value, false);
  assert.equal(f.authenticated.value, false);
  assert.equal(f.changedFileCount.value, undefined);
  assert.deepEqual(f.interactionState.value, { expanded: false, pending: false });
  assert.deepEqual(f.drafts, {});
  assert.deepEqual(f.cleared, ['voice', 'voice', 'history', 'attachments']);
  assert.equal(f.submissions.operationsReady.value, false);
});

test('只读模式提示在桌面停止，不提交中断或清空草稿', async t => {
  const calls = [];
  const f = fixture(t, async (url, init) => {
    calls.push({ url, method: init.method, body: JSON.parse(init.body) });
    return { ok: true, interruptedTurnId: 'turn', goalPauseError: 'Failed to pause thread goal' };
  });
  await f.stopTurn();
  assert.deepEqual(calls, []);
  assert.equal(f.error.value, '手机停止操作尚未开放，请在官方 Codex 停止任务。');
  assert.equal(f.stopping.value, false);
  assert.equal(f.drafts['fixture-chat'], '保留的草稿');
});

test('Mobile 停止绑定原聊天与活动轮次，重复点击和迟到失败不影响新聊天', async t => {
  const calls = []; let reject;
  const f = fixture(t, (url, init) => { calls.push({ url, body: JSON.parse(init.body) }); return new Promise((_, fail) => { reject = fail; }); });
  f.capabilities.value = { interrupt: true, managed: true };
  const stopping = f.stopTurn();
  await f.stopTurn();
  assert.deepEqual(calls, [{ url: '/api/threads/fixture-chat/interrupt', body: { turnId: 'turn' } }]);
  assert.equal(f.stopping.value, true);
  f.selectedId.value = 'another-chat'; f.error.value = '其他聊天的提示';
  reject(new Error('旧轮次停止失败')); await stopping;
  assert.equal(f.error.value, '其他聊天的提示'); assert.equal(f.stopping.value, false);
  assert.equal(f.drafts['fixture-chat'], '保留的草稿');
});

test('桌面导航独立于全局目录能力，失败与切聊天不误报打开成功', async t => {
  let resolve, reject;
  const calls = [];
  const f = fixture(t, (url, init) => {
    calls.push({ url, method: init.method });
    return new Promise((done, fail) => { resolve = done; reject = fail; });
  });
  await f.openDesktop();
  assert.deepEqual(calls, []);
  f.capabilities.value = { catalog: false, desktopNavigation: true };
  f.notice.value = '';
  const opening = f.openDesktop();
  assert.deepEqual(calls, [{ url: '/api/threads/fixture-chat/open', method: 'POST' }]);
  assert.equal(f.notice.value, '');
  reject(new ApiError('官方桌面尚未确认打开', 503));
  await opening;
  assert.equal(f.notice.value, '');
  assert.equal(f.error.value, '官方桌面尚未确认打开');
  const next = f.openDesktop();
  f.selectedId.value = 'other-chat';
  resolve({ ok: true });
  await next;
  assert.equal(f.notice.value, '');
  f.selectedId.value = 'fixture-chat';
  const confirmed = f.openDesktop();
  resolve({ ok: true });
  await confirmed;
  assert.equal(f.notice.value, '已在电脑上打开此对话');
  assert.equal(f.error.value, '');
  const unconfirmed = f.openDesktop();
  assert.equal(f.notice.value, '');
  resolve({ ok: false });
  await unconfirmed;
  assert.equal(f.notice.value, '');
  assert.match(f.error.value, /尚未确认打开/);
  assert.equal(f.drafts['fixture-chat'], '保留的草稿');
});
