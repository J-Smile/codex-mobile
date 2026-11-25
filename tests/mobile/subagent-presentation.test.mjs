import test from 'node:test';
import assert from 'node:assert/strict';
import { isSubagentItem, subagentLabel, subagents } from '../../apps/mobile/src/features/conversation/subagentPresentation.ts';

test('子智能体生命周期保留名称、模型和子线程身份，不把启动当成完成', () => {
  const item = { id: 'started', type: 'subAgentActivity', kind: 'started', agentThreadId: 'child', agentPath: '/root/research', model: 'model-a', reasoningEffort: 'high' };
  const agent = subagents(item)[0];
  assert.equal(agent.id, 'child');
  assert.equal(agent.name, 'research');
  assert.equal(agent.status, '已启动');
  assert.equal(agent.completed, false);
  assert.equal(agent.model, 'model-a');
  assert.equal(agent.effort, 'high');
  assert.equal(subagentLabel(item), '已启动子智能体 · research');
  assert.equal(subagents({ ...item, kind: 'completed' })[0].completed, true);
  assert.equal(subagents({ ...item, kind: 'interrupted' })[0].status, '已中断');
  assert.equal(subagents({ ...item, kind: 'interacted' })[0].status, '已联系');
});

test('协作调用的完成状态与子智能体本身的执行状态分别呈现，结果文本完整保留', () => {
  const item = { id: 'call', type: 'collabAgentToolCall', tool: 'wait', status: 'completed', receiverThreadIds: ['child-a', 'child-a', 'child-b'], agentsStates: {
    'child-a': { status: 'running', message: null },
    'child-b': { status: 'completed', message: '完整结果\n' + '长行'.repeat(500) },
    'child-c': { status: 'errored', message: '执行失败详情' },
  } };
  const agents = subagents(item);
  assert.deepEqual(agents.map((agent) => agent.id), ['child-a', 'child-b', 'child-c']);
  assert.equal(agents[0].status, '执行中');
  assert.equal(agents[0].completed, false);
  assert.equal(agents[1].message, item.agentsStates['child-b'].message);
  assert.equal(agents[2].failed, true);
  assert.equal(subagentLabel(item), '等待子智能体结束');
});

test('无目标的等待不虚构子对话，其他工具仍沿用原渲染', () => {
  assert.deepEqual(subagents({ id: 'wait', type: 'collabAgentToolCall', tool: 'wait', receiverThreadIds: [], agentsStates: {} }), []);
  assert.deepEqual(subagents({ id: 'missing', type: 'subAgentActivity', kind: 'started' }), []);
  assert.equal(isSubagentItem({ id: 'command', type: 'commandExecution' }), false);
});
