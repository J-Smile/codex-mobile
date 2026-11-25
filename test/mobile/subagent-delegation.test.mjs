import test from 'node:test';
import assert from 'node:assert/strict';
import { delegationPrompt, subagentTargets } from '../../mobile/src/features/composer/subagentDelegation.ts';

test('委派目标从原生事件去重，保留名称并采用最新生命周期', () => {
  const started = { id: 'start', type: 'subAgentActivity', kind: 'started', agentThreadId: 'child', agentPath: '/root/research' };
  const waiting = { id: 'wait', type: 'collabAgentToolCall', tool: 'wait', status: 'completed', receiverThreadIds: ['child'] };
  const finished = { id: 'finish', type: 'subAgentActivity', kind: 'completed', agentThreadId: 'child' };
  assert.deepEqual(subagentTargets([started, waiting]), [
    { id: 'child', name: 'research', path: '/root/research', status: '已启动' },
  ]);
  assert.deepEqual(subagentTargets([started, waiting, finished]), [
    { id: 'child', name: 'research', path: '/root/research', status: '已完成' },
  ]);
  assert.deepEqual(subagentTargets([{ id: 'other', type: 'agentMessage', text: '回复' }]), []);
});

test('新建与指派使用明确的原生委派请求，完整保留用户任务', () => {
  const task = '检查移动端问题。\n只读取现有实现，不改数据库。\n' + '长内容'.repeat(2000);
  const create = delegationPrompt(task);
  assert.match(create, /请创建一个子智能体/);
  assert.ok(create.endsWith(task));
  const target = { id: 'child-id', name: 'research', path: '/root/research', status: '已完成' };
  const assign = delegationPrompt(task, target);
  assert.match(assign, /已有子智能体「\/root\/research」/);
  assert.match(assign, /子对话 ID：child-id/);
  assert.doesNotMatch(assign, /请创建一个子智能体/);
  assert.ok(assign.endsWith(task));
  assert.throws(() => delegationPrompt(' \n\t'), /请填写/);
});
