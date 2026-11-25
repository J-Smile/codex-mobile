export type GoalStatus = 'active' | 'paused' | 'blocked' | 'usageLimited' | 'budgetLimited' | 'complete';

export interface ThreadGoal {
  threadId: string;
  objective: string;
  status: GoalStatus;
  tokenBudget: number | null;
  tokensUsed: number;
  timeUsedSeconds: number;
  createdAt: number;
  updatedAt: number;
}

export interface GoalInput {
  objective: string;
  tokenBudget?: number | null;
}

export interface GoalState {
  goal: ThreadGoal | null;
  completedGoal?: ThreadGoal | null;
}

export const goalStatusLabels: Record<GoalStatus, string> = {
  active: '进行中',
  paused: '已暂停',
  blocked: '等待处理',
  usageLimited: '用量受限',
  budgetLimited: '预算已用完',
  complete: '已完成',
};
