export type UsagePeriod = 'today' | '7d' | '30d' | 'all';
export type UsageMetrics = {
  totalTokens: number;
  inputTokens: number | null;
  outputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  reasoningOutputTokens: number | null;
};
export type UsageCoverage = {
  discoveredSources: number;
  processedSources: number;
  missingSources: number;
  unreadableSources: number;
  unassignedEvents: number;
  firstRecordedAt: string | null;
  lastRecordedAt: string | null;
  unsupportedSources?: number;
  gapEvents?: number;
  indexUnavailable?: boolean;
  unavailableThreads?: number;
};
export type UsageOverview = {
  status: 'indexing' | 'ready' | 'partial';
  refreshing?: boolean;
  dataState?: 'available' | 'empty' | 'unavailable';
  generatedAt: string;
  timeZone: string;
  period: UsagePeriod;
  scope: { kind: 'local' } | { kind: 'thread'; threadId: string; title: string };
  lifetime: UsageMetrics | null;
  today: UsageMetrics | null;
  selected: UsageMetrics | null;
  daily: Array<{ date: string; metrics: UsageMetrics }>;
  models: Array<{ model: string | null; metrics: UsageMetrics }>;
  coverage: UsageCoverage;
};
export type UsageThread = { threadId: string; title: string; metrics: UsageMetrics | null; status?: string; lastRecordedAt?: string | null; parentThreadId?: string | null };
export type UsageThreads = { status: UsageOverview['status']; items: UsageThread[]; nextCursor: string | null; coverage: UsageCoverage };
