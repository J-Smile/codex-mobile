import type { ThreadItem } from '../../types';
import { subagents } from '../conversation/subagentPresentation.ts';

export interface SubagentTarget {
  id: string;
  name: string;
  path: string;
  status: string;
}

export interface DelegationRequest {
  prompt: string;
  mode: 'now' | 'queue';
  target?: SubagentTarget;
}

export function subagentTargets(items: ThreadItem[]): SubagentTarget[] {
  const targets = new Map<string, SubagentTarget>();
  for (const item of items) {
    for (const agent of subagents(item)) {
      const previous = targets.get(agent.id);
      targets.set(agent.id, {
        id: agent.id,
        name: agent.path ? agent.name : previous?.name || agent.name,
        path: agent.path || previous?.path || '',
        status: agent.status === '查看子对话' ? previous?.status || agent.status : agent.status,
      });
    }
  }
  return [...targets.values()];
}

export function delegationPrompt(task: string, target?: SubagentTarget) {
  if (!task.trim()) throw new Error('请填写需要指派的任务。');
  const instruction = target
    ? '请将下面的任务指派给已有子智能体「' + (target.path || target.name) + '」（子对话 ID：' + target.id + '），由它执行，并在当前对话汇总结果。'
    : '请创建一个子智能体来执行下面的任务，并在当前对话协调、汇总它的结果。';
  return instruction + '\n\n任务内容：\n' + task;
}
