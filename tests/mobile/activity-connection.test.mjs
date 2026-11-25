import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import * as presentation from '../../apps/mobile/src/features/conversation/itemPresentation.ts';
import * as subagentPresentation from '../../apps/mobile/src/features/conversation/subagentPresentation.ts';
import { useStreamText } from '../../apps/mobile/src/features/conversation/useStreamText.ts';

const compiled = Object.fromEntries(['ActivityGroup', 'ItemView', 'TurnView'].map(name => {
  const source = readFileSync(new URL('../../apps/mobile/src/features/conversation/' + name + '.vue', import.meta.url), 'utf8');
  const script = compileScript(parse(source).descriptor, { id: name + '-connection-test' }).content;
  return [name, ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText];
}));

function fixture(t, component, input) {
  const props = (component === 'TurnView' ? vue.shallowReactive : vue.reactive)({ threadId: 'isolated-chat', theme: 'light', live: true, ...input });
  const module = { exports: {} }, cleanup = [];
  new Function('require', 'module', 'exports', compiled[component])(name => {
    if (name === 'vue') return { ...vue, useId: () => 'activity-test', onUnmounted: callback => cleanup.push(callback) };
    if (name === './itemPresentation') return presentation;
    if (name === './subagentPresentation') return subagentPresentation;
    if (name === './useStreamText') return { useStreamText };
    if (name === './useConversationHistory') return {
      useConversationDisclosure: () => vue.ref(),
      useConversationItemDetail: () => ({ detail: vue.ref(), version: vue.ref(), save() {} }),
    };
    return {};
  }, module, module.exports);
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit() {} }));
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  return { app, props };
}

test('断线的执行摘要等待同步，保持缓存状态和展开选择，恢复后重新显示执行状态', t => {
  const item = { id: 'cmd', type: 'commandExecution', command: 'npm test', status: 'inProgress' };
  const { app, props } = fixture(t, 'ActivityGroup', { items: [item], running: true });
  assert.equal(app.label.value, '正在运行命令');
  assert.equal(app.expanded.value, true);
  props.live = false;
  assert.equal(app.label.value, '等待连接同步…');
  assert.equal(app.expanded.value, true);
  assert.equal(props.items[0].status, 'inProgress');
  app.expandedByUser.value = false;
  props.live = true;
  assert.equal(app.label.value, '正在运行命令');
  assert.equal(app.expanded.value, false);
  props.items[0].status = 'completed';
  props.running = false;
  props.live = false;
  assert.equal(app.label.value, '运行了 1 个命令');
  props.items[0].status = 'failed';
  assert.equal(app.label.value, '执行遇到问题');
});

test('工具等待同步不清空输出或详情展开，恢复连接后使用实际完成状态', async t => {
  const { app, props } = fixture(t, 'ItemView', { item: {
    id: 'cmd', type: 'commandExecution', command: 'npm test', status: 'inProgress', aggregatedOutput: '第一行\n仍保留的输出',
  } });
  app.open.value = true;
  await vue.nextTick();
  props.live = false;
  assert.equal(app.awaitingSync.value, true);
  assert.equal(app.activityPrefix.value, '等待同步');
  assert.equal(app.toolStatus.value, '等待同步');
  assert.equal(app.commandOutput.value, '第一行\n仍保留的输出');
  assert.equal(app.detailsOpen.value, true);
  assert.equal(props.item.status, 'inProgress');
  props.live = true;
  assert.equal(app.activityPrefix.value, '正在运行');
  props.item.status = 'completed';
  assert.equal(app.activityPrefix.value, '已运行');
  assert.equal(app.toolStatus.value, '完成');
  assert.equal(app.detailsOpen.value, true);
});

test('连接展示变化不清空已渲染正文，恢复后的文本继续更新', async t => {
  const { app, props } = fixture(t, 'ItemView', { item: { id: 'reply', type: 'agentMessage', text: '已有正文' }, streaming: true });
  props.live = false;
  await vue.nextTick();
  assert.equal(app.displayText.value, '已有正文');
  props.live = true;
  props.item.text += '，连接恢复后继续。';
  await vue.nextTick();
  assert.equal(app.displayText.value, '已有正文，连接恢复后继续。');
});

test('当前轮次追加文字不刷新已有工具分组，工具结果、顺序及失败状态仍及时更新', async t => {
  const command = { id: 'cmd', type: 'commandExecution', status: 'completed' };
  const answer = { id: 'reply', type: 'agentMessage', text: '已有正文' };
  const { app, props } = fixture(t, 'TurnView', { turn: { id: 'turn', status: 'inProgress', items: [command, answer] } });
  const original = app.blocks.value;
  let changes = 0;
  const stop = vue.watch(() => app.blocks.value[0].items, () => changes++);
  t.after(stop);
  for (let index = 0; index < 20; index++) {
    props.turn = { ...props.turn, items: [command, { ...answer, text: '已有正文' + index }] };
    await vue.nextTick();
  }
  assert.equal(changes, 0);
  assert.equal(app.blocks.value[0], original[0]);
  assert.equal(app.blocks.value[1].items[0].text, '已有正文19');
  const latest = app.blocks.value;
  props.turn = { ...props.turn, items: [...props.turn.items, { id: 'hidden', type: 'reasoning', summary: ['思考更新'] }] };
  await vue.nextTick();
  assert.equal(app.blocks.value, latest);
  const failed = { ...command, status: 'failed', exitCode: 1 };
  props.turn = { ...props.turn, status: 'failed', error: { message: '命令失败' }, items: [failed, answer] };
  await vue.nextTick();
  assert.equal(changes, 1);
  assert.equal(app.blocks.value[0].items[0], failed);
  assert.equal(app.failed.value, true);
  assert.equal(app.failureMessage.value, '命令失败');
  const changed = app.blocks.value[0];
  const user = { id: 'user', type: 'userMessage', content: [] };
  props.turn = { ...props.turn, items: [user, failed, answer] };
  assert.deepEqual(app.blocks.value.map(block => block.id), ['user', 'cmd', 'reply']);
  assert.equal(app.blocks.value[1], changed);
  props.turn = { ...props.turn, items: [user, answer] };
  assert.deepEqual(app.blocks.value.map(block => block.id), ['user', 'reply']);
  assert.deepEqual(original[0].items, [command]);
});
