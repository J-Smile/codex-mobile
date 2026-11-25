import type { ThreadItem } from '../../types';

export function isSubagentItem(item: ThreadItem) {
  return item.type === 'subAgentActivity' || item.type === 'collabAgentToolCall';
}

const activityStates: Record<string, string> = {
  started: '已启动', completed: '已完成', interrupted: '已中断', interacted: '已联系',
};
const agentStates: Record<string, string> = {
  pendingInit: '准备中', running: '执行中', completed: '已完成',
  errored: '执行失败', shutdown: '已关闭', notFound: '未找到',
};

export function subagents(item: ThreadItem) {
  if (!isSubagentItem(item)) return [];
  if (item.type === 'subAgentActivity') {
    if (typeof item.agentThreadId !== 'string' || !item.agentThreadId) return [];
    const path = typeof item.agentPath === 'string' ? item.agentPath : '';
    return [{
      id: item.agentThreadId,
      name: path.split('/').filter(Boolean).at(-1) || '子智能体',
      path,
      status: activityStates[item.kind] || '状态已更新',
      failed: false,
      completed: item.kind === 'completed',
      model: typeof item.model === 'string' ? item.model : '',
      effort: typeof item.reasoningEffort === 'string' ? item.reasoningEffort : '',
      message: '',
    }];
  }
  const states = item.agentsStates || {};
  const ids = new Set<string>([...(item.receiverThreadIds || []), ...Object.keys(states)]
    .filter((id) => typeof id === 'string' && id));
  return [...ids].map((id) => {
    const state = states[id];
    return {
      id, name: '子智能体 ' + id.slice(0, 8), path: '',
      status: agentStates[state?.status] || '查看子对话',
      failed: state?.status === 'errored' || state?.status === 'notFound',
      completed: state?.status === 'completed',
      model: typeof item.model === 'string' ? item.model : '',
      effort: typeof item.reasoningEffort === 'string' ? item.reasoningEffort : '',
      message: typeof state?.message === 'string' ? state.message : '',
    };
  });
}

export function subagentLabel(item: ThreadItem) {
  if (item.type === 'subAgentActivity') {
    const labels: Record<string, string> = {
      started: '已启动子智能体', completed: '子智能体已完成',
      interrupted: '子智能体已中断', interacted: '已联系子智能体',
    };
    const label = labels[item.kind] || '子智能体状态更新';
    const agent = subagents(item)[0];
    return agent ? label + ' · ' + agent.name : label;
  }
  if (item.tool === 'wait') return item.status === 'inProgress' ? '正在等待子智能体' : '等待子智能体结束';
  return '子智能体协作';
}
