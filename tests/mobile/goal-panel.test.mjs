import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import * as goals from '../../apps/mobile/src/features/goals/goals.ts';

const source = readFileSync(new URL('../../apps/mobile/src/features/goals/GoalPanel.vue', import.meta.url), 'utf8');
const script = compileScript(parse(source).descriptor, { id: 'goal-panel-test' }).content;
const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
const goal = (status = 'active', changes = {}) => ({ threadId: 'chat', objective: '完成手机目标', status, tokenBudget: 5000,
  tokensUsed: 1234, timeUsedSeconds: 65, createdAt: 100, updatedAt: 200, ...changes });
const settled = async () => { await vue.nextTick(); await setImmediate(); await vue.nextTick(); };
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function fixture(t, responder, overrides = {}) {
  const props = vue.reactive({ open: true, threadId: 'chat', ...overrides });
  const calls = [], events = [], cleanup = [];
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => {
    if (name === 'vue') return { ...vue, useId: () => 'goal-test', onBeforeUnmount: callback => cleanup.push(callback) };
    if (name === './goals') return goals;
    if (name === '../../api') return {
      threadUrl: (id, action) => '/api/threads/' + encodeURIComponent(id) + '/' + action,
      request: async (url, options = {}) => { calls.push({ url, ...options }); return responder(url, options, calls.length); },
    };
    return {};
  }, module, module.exports);
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit: (...event) => events.push(event) }));
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  return { props, app, calls, events };
}

test('切换聊天后旧目标暂停回执和失败不污染新目标', async t => {
  for (const failure of [false, true]) {
    const pending = deferred();
    const f = fixture(t, async (url, options) => options.method ? pending.promise
      : { goal: goal('active', { threadId: url.includes('/other/') ? 'other' : 'chat' }) });
    await settled();
    const saving = f.app.setStatus('paused');
    f.props.threadId = 'other';
    await settled();
    if (failure) pending.reject(new Error('旧聊天失败'));
    else pending.resolve({ goal: goal('paused') });
    await saving;
    assert.equal(f.app.goal.value.threadId, 'other');
    assert.equal(f.app.goal.value.status, 'active');
    assert.equal(f.app.actionError.value, '');
    assert.deepEqual(f.events, []);
  }
});

test('目标读取失败禁止写入，重试成功后才允许创建', async t => {
  const f = fixture(t, async (_url, options, count) => {
    if (count === 1) throw new Error('目标服务未连接');
    return { goal: options.method ? goal() : null, completedGoal: null };
  });
  await settled();
  f.app.objective.value = '保留草稿';
  f.app.formOpen.value = true;
  assert.equal(f.app.ready.value, false);
  assert.match(f.app.loadError.value, /未连接/);
  await f.app.save();
  assert.equal(f.calls.length, 1);
  await f.app.load(true);
  assert.equal(f.app.objective.value, '保留草稿');
  await f.app.save();
  assert.deepEqual(JSON.parse(f.calls.at(-1).body), { objective: '保留草稿', status: 'active' });
  assert.equal(f.calls.at(-1).method, 'PATCH');
  assert.equal(f.events.at(-1)[0], 'close');
});

test('新对话只检查目标服务并调用创建回调，线程编号变化不重置草稿或重复提交', async t => {
  const pending = deferred(), submitted = [];
  const f = fixture(t, async () => ({ ready: true }), { threadId: '', createGoal: async (input, onCreated) => { submitted.push(input); onCreated('created-chat'); await pending.promise; } });
  await settled();
  assert.deepEqual(f.calls.map(call => call.url), ['/api/goal-service']);
  f.app.objective.value = '新目标\n保留项目默认模型';
  const saving = f.app.save();
  f.props.threadId = 'created-chat';
  f.props.goal = goal('active', { threadId: 'created-chat' });
  await settled();
  assert.equal(f.app.objective.value, '新目标\n保留项目默认模型');
  assert.equal(f.app.busy.value, 'saving');
  assert.equal(f.calls.length, 1);
  await f.app.save();
  assert.equal(submitted.length, 1);
  pending.resolve();
  await saving;
  assert.deepEqual(submitted, [{ objective: '新目标\n保留项目默认模型' }]);
  assert.deepEqual(f.events, [['close']]);
});

test('新建目标中已创建聊天但后续失败时保留表单，重新读取后在该聊天重试', async t => {
  const pending = deferred();
  let creates = 0;
  const f = fixture(t, async url => url === '/api/goal-service' ? { ready: true } : { goal: null, completedGoal: null },
    { threadId: '', createGoal: async (_input, onCreated) => { creates++; onCreated('created-chat'); await pending.promise; } });
  await settled();
  f.app.objective.value = '不应创建第二条聊天';
  f.app.tokenBudget.value = 123456789;
  const saving = f.app.save();
  f.props.threadId = 'created-chat';
  await settled();
  pending.reject(new Error('绑定目标失败'));
  await saving;
  assert.equal(f.app.objective.value, '不应创建第二条聊天');
  assert.equal(f.app.tokenBudget.value, 123456789);
  assert.equal(f.app.ready.value, true);
  assert.match(f.app.actionError.value, /绑定目标失败/);
  await f.app.save();
  assert.equal(creates, 1);
  assert.equal(f.calls.at(-1).url, '/api/threads/created-chat/goal');
  assert.deepEqual(JSON.parse(f.calls.at(-1).body), { objective: '不应创建第二条聊天', tokenBudget: 123456789, status: 'active' });
});

test('新建目标时切换到其他聊天或关闭后，旧回执不能关闭新面板', async t => {
  for (const leave of ['switch', 'close']) {
    const pending = deferred();
    const f = fixture(t, async url => url === '/api/goal-service' ? { ready: true } : { goal: goal() },
      { threadId: '', createGoal: async (_input, onCreated) => { onCreated('created-chat'); await pending.promise; } });
    await settled();
    f.app.objective.value = '原目标';
    const saving = f.app.save();
    if (leave === 'switch') f.props.threadId = 'other-chat';
    else f.props.open = false;
    await settled();
    pending.resolve();
    await saving;
    assert.deepEqual(f.events, []);
    if (leave === 'switch') assert.equal(f.app.goal.value.objective, '完成手机目标');
  }
});

test('目标服务未就绪时新建目标不会调用创建回调', async t => {
  let creates = 0;
  const f = fixture(t, async () => ({ ready: false }), { threadId: '', createGoal: async () => { creates++; } });
  await settled();
  f.app.formOpen.value = true;
  f.app.objective.value = '目标';
  await f.app.save();
  assert.equal(f.app.ready.value, false);
  assert.equal(creates, 0);
  assert.match(f.app.loadError.value, /尚未就绪/);
});

test('无当前目标时从普通草稿预填，输入后父级草稿变化不覆盖编辑', async t => {
  const f = fixture(t, async () => ({ goal: null, completedGoal: null }), { initialObjective: '普通消息中的任务' });
  await settled();
  assert.equal(f.app.objective.value, '普通消息中的任务');
  f.app.objective.value = '用户正在修改的目标';
  f.props.initialObjective = '父级草稿变化';
  await settled();
  assert.equal(f.app.objective.value, '用户正在修改的目标');
});

test('桌面状态实时更新操作能力，保持正在编辑的内容，完成记录不可暂停或继续', async t => {
  const f = fixture(t, async () => ({ goal: goal(), completedGoal: null }));
  await settled();
  await f.app.edit();
  f.app.objective.value = '正在编辑的新内容';
  for (const status of ['paused', 'blocked', 'usageLimited']) {
    f.props.goal = goal(status);
    await settled();
    assert.equal(f.app.goal.value.status, status);
    assert.equal(f.app.canResume.value, true);
    assert.equal(f.app.objective.value, '正在编辑的新内容');
  }
  f.props.goal = goal('budgetLimited', { tokensUsed: 5000 });
  await settled();
  assert.equal(f.app.canResume.value, false);
  await f.app.setStatus('active');
  f.props.goal = goal('complete');
  await settled();
  assert.equal(f.app.formOpen.value, false);
  await f.app.setStatus('paused');
  await f.app.setStatus('active');
  await f.app.endGoal();
  assert.equal(f.calls.length, 1);
});

test('预算耗尽时不可继续，提高或取消预算后仍由用户显式恢复', async t => {
  const f = fixture(t, async (_url, options) => ({ goal: goal(options.method ? 'active' : 'budgetLimited',
    { tokensUsed: 12, tokenBudget: 1 }), completedGoal: null }));
  await settled();
  assert.equal(f.app.canResume.value, false);
  f.props.goal = goal('budgetLimited', { tokensUsed: 12, tokenBudget: 12 });
  await settled();
  await f.app.setStatus('active');
  assert.equal(f.calls.length, 1);
  f.props.goal = goal('budgetLimited', { tokensUsed: 12, tokenBudget: 1000 });
  await settled();
  assert.equal(f.app.canResume.value, true);
  assert.equal(f.calls.length, 1);
  await f.app.setStatus('active');
  assert.deepEqual(JSON.parse(f.calls.at(-1).body), { status: 'active' });
  f.props.goal = goal('budgetLimited', { tokensUsed: 12, tokenBudget: null });
  await settled();
  assert.equal(f.app.canResume.value, true);
  assert.equal(f.calls.length, 2);
  await f.app.setStatus('active');
  assert.equal(f.calls.length, 3);
});

test('读取中的桌面新状态不会被较早的接口结果覆盖', async t => {
  const pending = deferred();
  const f = fixture(t, () => pending.promise, { goal: goal() });
  f.props.goal = goal('blocked');
  await settled();
  pending.resolve({ goal: goal(), completedGoal: null });
  await settled();
  assert.equal(f.app.goal.value.status, 'blocked');
  assert.equal(f.app.canResume.value, true);
});

test('编辑预算空白明确清空，保存失败保留表单且成功后同步状态', async t => {
  let writes = 0;
  const f = fixture(t, async (_url, options) => {
    if (!options.method) return { goal: goal(), completedGoal: null };
    if (++writes === 1) throw new Error('保存失败');
    return { goal: goal('active', JSON.parse(options.body)), completedGoal: null };
  });
  await settled();
  await f.app.edit();
  f.app.objective.value = '更新后的目标';
  f.app.tokenBudget.value = '';
  await f.app.save();
  assert.equal(f.app.objective.value, '更新后的目标');
  assert.equal(f.app.tokenBudget.value, '');
  assert.equal(f.app.formOpen.value, true);
  assert.equal(f.events.length, 0);
  await f.app.save();
  assert.deepEqual(JSON.parse(f.calls.at(-1).body), { objective: '更新后的目标', tokenBudget: null });
  assert.equal(f.events[0][0], 'changed');
  assert.equal(f.events[0][1].goal.tokenBudget, null);
  assert.equal(f.events[1][0], 'close');
});

test('暂停和继续发送真实状态更新，结束使用 DELETE 而非标记完成', async t => {
  const f = fixture(t, async (_url, options) => ({ goal: options.method === 'DELETE' ? null
    : goal(options.body ? JSON.parse(options.body).status : 'active'), completedGoal: null }));
  await settled();
  await f.app.setStatus('paused');
  await f.app.setStatus('active');
  await f.app.endGoal();
  assert.deepEqual(f.calls.slice(1).map(call => [call.method, call.body && JSON.parse(call.body)]), [
    ['PATCH', { status: 'paused' }], ['PATCH', { status: 'active' }], ['DELETE', undefined],
  ]);
  assert.equal(f.events.at(-2)[1].goal, null);
});

test('已完成目标只读，新目标清空表单且不沿用旧预算', async t => {
  const f = fixture(t, async (_url, options) => ({ goal: options.method ? goal() : null, completedGoal: goal('complete') }));
  await settled();
  assert.equal(f.app.shownGoal.value.status, 'complete');
  assert.equal(f.app.formOpen.value, false);
  await f.app.edit(true);
  assert.equal(f.app.objective.value, '');
  assert.equal(f.app.tokenBudget.value, '');
  f.app.objective.value = '下一目标';
  f.app.tokenBudget.value = 0;
  assert.equal(f.app.canSave.value, false);
  f.app.tokenBudget.value = 1234567890123;
  assert.equal(f.app.canSave.value, true);
  f.app.tokenBudget.value = '';
  await f.app.save();
  assert.deepEqual(JSON.parse(f.calls.at(-1).body), { objective: '下一目标', status: 'active' });
});
