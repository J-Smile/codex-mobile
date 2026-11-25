import { onScopeDispose, ref, watch } from 'vue';
import { ApiError, request, threadUrl } from '../../api.ts';
import type { StreamSnapshot, StreamUpdate, ThreadItem, Turn } from '../../types';
import type { QueueSnapshot } from '../composer/useMessageQueue';
import { desktopEvents } from '../../desktopEvents.ts';

export class StreamBaselineError extends Error {}

export function mergeStreamUpdate(turns: Turn[], update: StreamUpdate): Turn[] {
  const values = new Map(turns.map((turn) => [turn.id, turn]));
  const changes = new Map<string, ThreadItem[]>();
  for (const { turnId, item } of update.updates ?? []) {
    const items = changes.get(turnId) || [];
    items.push(item);
    changes.set(turnId, items);
  }
  for (const append of update.textAppends || []) {
    const original = values.get(append.turnId)?.items.find(item => item.id === append.itemId);
    if (original?.type !== 'agentMessage' || typeof original.text !== 'string' || original.text.length !== append.offset) {
      throw new StreamBaselineError('消息已变化，正在重新同步…');
    }
    const items = changes.get(append.turnId) || [];
    items.push({ ...original, text: original.text + append.text });
    changes.set(append.turnId, items);
  }
  if (!update.turns) {
    for (const [turnId, replacements] of changes) {
      const turn = values.get(turnId)!;
      const byId = new Map(replacements.map(item => [item.id, item]));
      values.set(turnId, { ...turn, items: turn.items.map(item => byId.get(item.id) ?? item) });
    }
    return [...values.values()];
  }
  for (const { itemOrder, ...meta } of update.turns) {
    const current = values.get(meta.id);
    const items = new Map((current?.items || []).map((item) => [item.id, item]));
    for (const item of changes.get(meta.id) || []) items.set(item.id, item);
    const ordered = itemOrder.map((id) => items.get(id)).filter((item): item is ThreadItem => !!item);
    // 未变化的轮次保留引用，避免流式输出使已完成的正文与执行分组重新计算。
    if (current && ordered.length === current.items.length && ordered.every((item, index) => item === current.items[index])
      && Object.entries(meta).every(([key, value]) => current[key as keyof Turn] === value)) continue;
    values.set(meta.id, { ...current, ...meta, items: ordered });
  }
  return [...values.values()];
}

export function useThreadStream(options: {
  threadId: () => string;
  authenticated: () => boolean;
  snapshot: (snapshot: StreamSnapshot) => void;
  update: (update: StreamUpdate) => void;
  queue?: (queue: QueueSnapshot, threadId: string) => void;
  expired: () => void;
}) {
  const status = ref<'connecting' | 'connected' | 'disconnected' | 'history'>('connecting');
  const loading = ref(false);
  const error = ref('');
  let stream: ReturnType<typeof desktopEvents> | undefined;
  let sessionCheck: AbortController | undefined;
  let lastActivity = Date.now();
  // 桌面每 15 秒发送心跳，留三个周期确认静默断流。
  const quietTimeout = 45_000;

  function cancelSessionCheck() {
    const pending = sessionCheck;
    sessionCheck = undefined;
    pending?.abort();
  }

  function close() {
    stream?.close();
    stream = undefined;
    cancelSessionCheck();
  }

  async function checkSession(reconnect: boolean) {
    if (!options.authenticated()) return;
    if (sessionCheck) {
      if (!reconnect) return;
      cancelSessionCheck();
    }
    const controller = new AbortController();
    sessionCheck = controller;
    const timeout = setTimeout(() => controller.abort(), 10_000);
    try {
      await request('/api/session', { signal: controller.signal });
      if (sessionCheck === controller && !controller.signal.aborted && reconnect) connect();
    } catch (failure) {
      if (sessionCheck !== controller) return;
      if (controller.signal.aborted) {
        status.value = 'disconnected';
        error.value = '连接超时，正在重试…';
        return;
      }
      if (failure instanceof ApiError && failure.status === 401) {
        close();
        loading.value = false;
        options.expired();
      } else {
        status.value = 'disconnected';
        error.value = (failure as Error).message;
      }
    } finally {
      clearTimeout(timeout);
      if (sessionCheck === controller) sessionCheck = undefined;
    }
  }

  function connect() {
    close();
    if (document.hidden || !options.authenticated() || !options.threadId()) return;
    const threadId = options.threadId();
    const current = desktopEvents(threadUrl(threadId, 'events') + '?textAppends=1&compactTextAppends=1');
    stream = current;
    lastActivity = Date.now();
    status.value = 'connecting';
    error.value = '';
    const relevant = () => stream === current && options.threadId() === threadId;
    const received = (connection: string) => {
      if (connection === 'live') cancelSessionCheck();
      lastActivity = Date.now();
      loading.value = false;
      status.value = connection === 'live' ? 'connected' : 'history';
      error.value = '';
    };
    current.addEventListener('snapshot', (event) => {
      if (!relevant()) return;
      const snapshot = JSON.parse((event as MessageEvent).data) as StreamSnapshot;
      received(snapshot.connection);
      options.snapshot(snapshot);
    });
    current.addEventListener('update', (event) => {
      if (!relevant()) return;
      const update = JSON.parse((event as MessageEvent).data) as StreamUpdate;
      try {
        options.update(update);
        received(update.connection);
      } catch (failure) {
        if (!(failure instanceof StreamBaselineError)) throw failure;
        // Stop accepting patches until a fresh snapshot establishes the baseline.
        close();
        status.value = 'connecting';
        error.value = failure.message;
        void checkSession(true);
      }
    });
    current.addEventListener('desktop-error', (event) => {
      if (!relevant()) return;
      lastActivity = Date.now();
      loading.value = false;
      status.value = 'disconnected';
      error.value = JSON.parse((event as MessageEvent).data).message;
    });
    current.addEventListener('queue', (event) => {
      if (!relevant()) return;
      lastActivity = Date.now();
      options.queue?.(JSON.parse((event as MessageEvent).data) as QueueSnapshot, threadId);
    });
    current.addEventListener('heartbeat', () => { if (relevant()) lastActivity = Date.now(); });
    current.onerror = () => {
      if (!relevant()) return;
      loading.value = false;
      status.value = 'disconnected';
      if (document.hidden) close();
      else void checkSession(false);
    };
  }

  function resume() {
    if (document.hidden) {
      cancelSessionCheck();
      return;
    }
    if (!stream || status.value === 'disconnected' || Date.now() - lastActivity >= quietTimeout) connect();
  }

  // 浏览器未报告断线时，连续三个心跳周期无响应也重新拉取快照。
  const watchdog = setInterval(() => {
    if (document.hidden || !options.authenticated() || !options.threadId() || sessionCheck) return;
    if (Date.now() - lastActivity < quietTimeout) return;
    status.value = 'disconnected';
    error.value = '连接已中断，正在重新同步…';
    void checkSession(true);
  }, 5_000);

  watch([options.threadId, options.authenticated], ([threadId, authenticated]) => {
    loading.value = !!threadId && authenticated;
    connect();
  }, { immediate: true });
  document.addEventListener('visibilitychange', resume);
  window.addEventListener('online', resume);
  window.addEventListener('pageshow', resume);
  window.addEventListener('focus', resume);
  onScopeDispose(() => {
    close();
    clearInterval(watchdog);
    document.removeEventListener('visibilitychange', resume);
    window.removeEventListener('online', resume);
    window.removeEventListener('pageshow', resume);
    window.removeEventListener('focus', resume);
  });
  return { status, loading, error, resume };
}
