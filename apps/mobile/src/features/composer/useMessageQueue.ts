import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import { ApiError, request, threadUrl } from '../../api.ts';

export type MessageMode = 'now' | 'queue';
export type QueuedMessage = {
  id: string;
  prompt: string;
  status: string;
  createdAt?: number;
  pausedReason?: string;
  attachmentCount?: number;
};
export type QueueSnapshot = { revision: string; messages: QueuedMessage[] };
export type QueueActions = { remove: (messageId: string) => Promise<boolean> };
type QueueChange = { action: 'delete'; messageId: string };

export function useMessageQueue(options: { threadId: () => string; authenticated: () => boolean }) {
  const queue = shallowRef<QueueSnapshot | null>(null);
  const loading = ref(false);
  const busy = ref(false);
  const error = ref('');
  let generation = 0;
  let reader: AbortController | undefined;
  let writer: AbortController | undefined;

  function acceptSnapshot(snapshot: QueueSnapshot, threadId = options.threadId()) {
    if (threadId !== options.threadId() || !options.authenticated()) return;
    reader?.abort();
    reader = undefined;
    queue.value = snapshot;
    loading.value = false;
    error.value = '';
  }

  async function reload() {
    reader?.abort();
    const threadId = options.threadId();
    if (!threadId || !options.authenticated()) return;
    const controller = new AbortController();
    const current = generation;
    reader = controller;
    loading.value = true;
    error.value = '';
    try {
      const snapshot = await request<QueueSnapshot>(threadUrl(threadId, 'queue'), { signal: controller.signal, timeoutMs: 15_000 });
      if (!controller.signal.aborted && current === generation) acceptSnapshot(snapshot, threadId);
    } catch (failure) {
      if (current === generation && reader === controller) {
        if (failure instanceof ApiError && failure.status === 408) error.value = '读取队列超时，请刷新重试。';
        else if (!controller.signal.aborted) error.value = (failure as Error).message;
      }
    } finally {
      if (reader === controller) { reader = undefined; loading.value = false; }
    }
  }

  async function change(action: QueueChange, revision?: string): Promise<boolean> {
    const snapshot = queue.value;
    const threadId = options.threadId();
    if (!snapshot || !threadId || !options.authenticated() || busy.value) return false;
    if (revision && revision !== snapshot.revision) {
      error.value = '队列已变化，请按最新顺序重新调整。';
      return false;
    }
    const current = generation;
    const controller = new AbortController();
    writer = controller;
    busy.value = true;
    error.value = '';
    try {
      const updated = await request<QueueSnapshot>(threadUrl(threadId, 'queue'), {
        method: 'POST', body: JSON.stringify({ ...action, revision: snapshot.revision }), signal: controller.signal, timeoutMs: 15_000,
      });
      if (current !== generation || controller.signal.aborted) return false;
      // 桌面可能同时消费队列，较迟响应不能覆盖已收到的新状态。
      if (queue.value?.revision !== snapshot.revision && queue.value?.revision !== updated.revision) await reload();
      else acceptSnapshot(updated, threadId);
      return current === generation;
    } catch (failure) {
      if (current !== generation) return false;
      if (failure instanceof ApiError && failure.status === 408) {
        error.value = '等待操作结果超时，请刷新队列确认是否已生效。';
        return false;
      }
      if (failure instanceof ApiError && failure.status === 409) await reload();
      if (current === generation) error.value = (failure as Error).message;
      return false;
    } finally {
      if (writer === controller) writer = undefined;
      if (current === generation) busy.value = false;
    }
  }

  function remove(messageId: string) {
    return change({ action: 'delete', messageId });
  }

  watch([options.threadId, options.authenticated], () => {
    generation++;
    reader?.abort();
    writer?.abort();
    reader = undefined;
    writer = undefined;
    queue.value = null;
    loading.value = false;
    busy.value = false;
    error.value = '';
    void reload();
  }, { immediate: true });
  onScopeDispose(() => { generation++; reader?.abort(); writer?.abort(); });
  return { queue, loading, busy, error, acceptSnapshot, reload, remove };
}
