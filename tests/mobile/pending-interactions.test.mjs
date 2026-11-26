import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import * as interactions from '../../apps/mobile/src/features/conversation/interactions.ts';
import { InteractionReplyState, questionAnswers } from '../../apps/mobile/src/features/conversation/interactions.ts';

const question = (overrides = {}) => ({
  id: 'request-1', turnId: 'turn-1', kind: 'questions', title: '问题', message: '', replySupported: true, skipSupported: true,
  questions: [
    { id: 'choice', header: '', question: '使用哪种方式？', secret: false, options: [{ label: '直接发送', description: '' }, { label: '排队', description: '' }] },
    { id: 'details', header: '', question: '补充要求？', secret: false, options: [] },
  ], ...overrides,
});

test('stream refresh preserves choices, free text, current question and minimized state', () => {
  const state = new InteractionReplyState(), entry = question();
  state.sync('thread-a', [entry]);
  const draft = state.drafts[entry.id];
  draft.choices.choice = 1;
  draft.text.details = '保留长行';
  draft.step = 1;
  draft.minimized = true;
  state.sync('thread-a', [question()]);
  assert.equal(state.drafts[entry.id], draft);
  assert.deepEqual(questionAnswers(entry, draft), { choice: ['排队'], details: ['保留长行'] });
  assert.equal(draft.step, 1);
  assert.equal(draft.minimized, true);
});

test('successful replies disappear immediately and stale snapshots cannot resurrect them', () => {
  const state = new InteractionReplyState(), entry = question();
  state.sync('thread-a', [entry]);
  state.finish(state.begin(entry));
  assert.deepEqual(state.pending, []);
  state.sync('thread-a', []);
  state.sync('thread-a', [question()]);
  assert.deepEqual(state.pending, []);
  state.sync('thread-b', [question()]);
  assert.equal(state.pending.length, 1);
  state.sync('thread-a', [question()]);
  assert.deepEqual(state.pending, []);
  state.sync('thread-a', [question({ turnId: 'turn-2' })]);
  assert.equal(state.pending.length, 1);
});

test('in-flight submission cannot be duplicated and a failure preserves the answer for retry', () => {
  const state = new InteractionReplyState(), entry = question();
  state.sync('thread-a', [entry]);
  const draft = state.drafts[entry.id];
  draft.choices.choice = -1;
  draft.text.choice = '另一个方式';
  const first = state.begin(entry);
  assert.equal(state.begin(entry), undefined);
  state.fail(first, '暂时无法连接');
  assert.equal(draft.error, '暂时无法连接');
  assert.deepEqual(questionAnswers(entry, draft).choice, ['另一个方式']);
  assert.equal(state.pending.length, 1);
  const retry = state.begin(entry);
  assert.ok(retry);
  assert.equal(draft.error, '');
  state.finish(retry);
  assert.deepEqual(state.pending, []);
});

test('thread change resets drafts and an old reply cannot hide the new thread request with the same ID', () => {
  const state = new InteractionReplyState(), entry = question();
  state.sync('thread-a', [entry]);
  state.drafts[entry.id].text.details = '旧聊天答案';
  const submission = state.begin(entry);
  state.sync('thread-b', [question()]);
  assert.deepEqual(state.drafts[entry.id].text, {});
  state.finish(submission);
  assert.equal(state.pending.length, 1);
  assert.equal(state.drafts[entry.id].busy, false);
  assert.equal(state.drafts[entry.id].error, '');
});

test('a removed request closes immediately and cannot be submitted from its departing panel', () => {
  const state = new InteractionReplyState(), entry = question();
  state.sync('thread-a', [entry]);
  state.sync('thread-a', []);
  assert.deepEqual(state.pending, []);
  assert.equal(state.begin(entry), undefined);
});

test('custom answer overrides a radio choice only when explicitly selected, and blanks stay incomplete', () => {
  const state = new InteractionReplyState(), entry = question();
  state.sync('thread-a', [entry]);
  const draft = state.drafts[entry.id];
  draft.choices.choice = 0;
  draft.text.choice = '保留的草稿';
  assert.deepEqual(questionAnswers(entry, draft).choice, ['直接发送']);
  draft.choices.choice = -1;
  assert.deepEqual(questionAnswers(entry, draft).choice, ['保留的草稿']);
  draft.text.choice = '   ';
  assert.deepEqual(questionAnswers(entry, draft), { choice: [], details: [] });
});

test('异步问题在桌面部分回复后保留当前未答题与草稿，题号随剩余问题调整', () => {
  const state = new InteractionReplyState();
  const entry = question({ source: 'async-message' });
  state.sync('thread-a', [entry]);
  const draft = state.drafts[entry.id];
  draft.step = 1;
  draft.text.details = '尚未提交的答案';
  state.sync('thread-a', [{ ...entry, questions: entry.questions.slice(1) }]);
  assert.equal(state.drafts[entry.id], draft);
  assert.equal(draft.step, 0);
  assert.deepEqual(questionAnswers(state.pending[0], draft), { details: ['尚未提交的答案'] });
});

const source = readFileSync(new URL('../../apps/mobile/src/features/conversation/PendingInteractions.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'pending-interactions-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
const settled = async () => { await vue.nextTick(); await setImmediate(); await vue.nextTick(); };
function fixture(t, entries, responder = async () => ({})) {
  const props = vue.reactive({ threadId: 'isolated-chat', interactions: entries });
  const calls = [], events = [], module = { exports: {} };
  new Function('require', 'module', 'exports', 'HTMLTextAreaElement', compiled)(name => {
    if (name === 'vue') return { ...vue, useId: () => 'questions-test' };
    if (name === './interactions') return interactions;
    if (name === '../../api') return {
      threadUrl: (id, action) => '/api/threads/' + id + '/' + action,
      request: async (url, options) => { calls.push({ url, ...options }); return responder(calls.length); },
    };
    return {};
  }, module, module.exports, class HTMLTextAreaElement {});
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit: (...event) => events.push(event) }));
  t.after(() => scope.stop());
  return { props, app, calls, events };
}

test('异步跳过只清当前题并进入下一题，最后提交实际答案和明确跳过的题目', async t => {
  const f = fixture(t, [question({ source: 'async-message' })]);
  f.app.selectOption(0);
  f.app.skipQuestion();
  await settled();
  assert.equal(f.app.draft.value.step, 1);
  assert.deepEqual(f.app.draft.value.skipped, { choice: true });
  assert.equal(f.app.draft.value.choices.choice, undefined);
  assert.equal(f.calls.length, 0);
  f.app.draft.value.text.details = '保留发送与停止';
  f.app.customInput();
  await f.app.submit();
  await settled();
  assert.deepEqual(JSON.parse(f.calls[0].body), { answers: { details: ['保留发送与停止'] }, skipped: ['choice'] });
  assert.equal(f.app.active.value, undefined);
});

test('最后一题跳过时发送前面答案，失败保留选择和跳过状态供重试', async t => {
  const entries = [question({ source: 'async-message' }), question({ id: 'next-request' })];
  const f = fixture(t, entries, async count => { if (count === 1) throw new Error('连接暂时失败'); return {}; });
  f.app.selectOption(1);
  await f.app.changeQuestion(1);
  f.app.draft.value.text.details = '跳过时应清除这题草稿';
  f.app.skipQuestion();
  await settled();
  assert.equal(f.app.draft.value.error, '连接暂时失败');
  assert.equal(f.app.draft.value.choices.choice, 1);
  assert.equal(f.app.draft.value.text.details, undefined);
  assert.deepEqual(f.app.draft.value.skipped, { details: true });
  f.app.skipQuestion();
  await settled();
  assert.equal(f.calls.length, 2);
  assert.deepEqual(JSON.parse(f.calls[1].body), { answers: { choice: ['排队'] }, skipped: ['details'] });
  assert.equal(f.app.active.value.id, 'next-request');
  assert.ok(f.events.every(([, state]) => state.pending && state.expanded));
});

test('异步全部跳过不发请求，旧快照不复活卡片并连续展示下一请求，原生仍可跳过全部', async t => {
  const entries = [question({ source: 'async-message' }), question({ id: 'native-request' })];
  const f = fixture(t, entries);
  f.app.skipQuestion();
  await settled();
  f.app.skipQuestion();
  await settled();
  assert.equal(f.calls.length, 0);
  assert.equal(f.app.active.value.id, 'native-request');
  f.props.interactions = structuredClone(entries);
  await settled();
  assert.equal(f.app.active.value.id, 'native-request');
  f.app.skipQuestion();
  await settled();
  assert.equal(f.calls.length, 1);
  assert.deepEqual(JSON.parse(f.calls[0].body), { skip: true });
  assert.equal(f.app.active.value, undefined);
});

test('回到已跳过题重新作答取消跳过，正常发送完整答案', async t => {
  const f = fixture(t, [question({ source: 'async-message' })]);
  f.app.skipQuestion();
  await settled();
  f.app.draft.value.text.details = '保留既有行为';
  f.app.customInput();
  await f.app.changeQuestion(0);
  f.app.selectOption(0);
  await f.app.changeQuestion(1);
  await f.app.submit();
  assert.deepEqual(JSON.parse(f.calls[0].body), { answers: { choice: ['直接发送'], details: ['保留既有行为'] } });
});

test('不可回复或断线卡片不占用普通输入，连接恢复仍保留答案', async t => {
  const f = fixture(t, [question()]);
  f.app.draft.value.text.details = '尚未提交';
  f.props.disabled = true;
  f.props.unavailableReason = '当前连接不支持在手机回复';
  await settled();
  assert.equal(f.events.at(-1)[1].expanded, false);
  await f.app.submit();
  assert.equal(f.calls.length, 0);
  assert.equal(f.app.draft.value.text.details, '尚未提交');
  f.props.disabled = false;
  await settled();
  assert.equal(f.events.at(-1)[1].expanded, true);
  f.props.interactions = [question({ replySupported: false })];
  await settled();
  assert.equal(f.events.at(-1)[1].expanded, false);
  assert.equal(f.app.draft.value.text.details, '尚未提交');
});
