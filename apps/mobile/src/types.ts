export interface MessageModel {
  model?: string;
  thinking?: string;
}

export interface DesktopThread {
  id: string;
  title: string;
  cwd?: string;
  status?: string | { type: string };
  model?: string | null;
  mode?: string | null;
  reasoningEffort?: string | null;
  permissionMode?: string;
  permissionProfile?: string | null;
  pinnedIndex?: number;
  projectId?: string | null;
  updatedAt?: number;
  goal?: import('./features/goals/goals').ThreadGoal | null;
  completedGoal?: import('./features/goals/goals').ThreadGoal | null;
}

export interface Project {
  projectId: string;
  label: string;
  path: string;
}

// 保留桌面端的条目结构，新的工具输出也能通过详情展示。
export interface ThreadItem {
  id: string;
  type: string;
  [field: string]: any;
}

export interface Turn {
  id: string;
  status: string;
  startedAt?: number;
  completedAt?: number | null;
  error?: unknown;
  items: ThreadItem[];
}

export interface HistoryPage {
  thread: DesktopThread;
  turns: Turn[];
  nextCursor: string | null;
}

export type StreamSnapshot = Omit<HistoryPage, 'nextCursor'> & {
  firstTurnPending?: boolean;
  syncMode?: 'desktop-poll';
  capabilities?: Record<string, boolean>;
  nextCursor?: string | null;
  changedFileCount?: number;
  connection: 'live' | 'history';
  interactions?: PendingInteraction[];
  queue?: QueueSnapshot;
};

type StreamTextAppend = { turnId: string; itemId: string; offset: number; text: string };

export type StreamUpdate = {
  connection: StreamSnapshot['connection'];
} & ({
  thread: DesktopThread;
  turns: (Omit<Turn, 'items'> & { itemOrder: string[] })[];
  updates: { turnId: string; item: ThreadItem }[];
  changedFileCount?: number;
  textAppends?: StreamTextAppend[];
  interactions?: PendingInteraction[];
} | {
  turns?: never;
  updates?: never;
  textAppends: StreamTextAppend[];
});
import type { PendingInteraction } from './features/conversation/interactions';
import type { QueueSnapshot } from './features/composer/useMessageQueue';
