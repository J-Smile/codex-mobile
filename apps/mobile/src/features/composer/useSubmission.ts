import { onScopeDispose, reactive, ref } from 'vue';
import { ApiError, request } from '../../api.ts';
import { desktopStorageKey } from '../../transport.ts';

export interface OperationReceipt {
  threadId?: string;
  firstTurnAccepted?: boolean;
  status?: string;
}
export interface PendingOperation {
  id: string;
  kind: string;
  target: string;
  phase: 'dispatching' | 'unknown' | 'accepted' | 'not-dispatched' | 'missing';
  localOnly?: boolean;
  closed?: boolean;
  receipt?: OperationReceipt;
}
type Identity = { requestId: string };
type OperationResponse = { operations: PendingOperation[]; results?: PendingOperation[] };
export interface SubmissionReadiness {
  target: string;
  capable: boolean;
  connected: boolean;
  hasContent: boolean;
  uploading: boolean;
  imageErrors: boolean;
  imageCount: number;
  imageSupported: boolean;
  voiceBusy: boolean;
  firstTurnPending?: boolean;
  missingSource?: boolean;
}

export function useSubmission() {
  // 绑定本次页面的电脑；异步回执不能写进后来选择的电脑。
  const storageKey = desktopStorageKey('codex-mobile-pending-v2');
  const identities = reactive(new Map<string, Identity>());
  try {
    const saved = JSON.parse(localStorage.getItem(storageKey) || '[]');
    if (Array.isArray(saved)) for (const entry of saved) {
      if (Array.isArray(entry) && typeof entry[0] === 'string' && /^[a-zA-Z0-9_-]{16,100}$/.test(entry[1]?.requestId || '')) {
        identities.set(entry[0], { requestId: entry[1].requestId });
      }
    }
  } catch { /* 无法读取时仍从服务端核对未决操作。 */ }
  const pendingSends = reactive(new Set<string>());
  const unresolvedOperations = ref<PendingOperation[]>([]);
  const recoveredCreations = ref<PendingOperation[]>([]);
  const operationsReady = ref(false);
  const operationsError = ref('');
  let lifetime = new AbortController();
  let revision = 0;
  let querySequence = 0;
  let disposed = false;

  function persist() { localStorage.setItem(storageKey, JSON.stringify([...identities])); }
  function forget(target: string, id: string) {
    if (identities.get(target)?.requestId !== id) return;
    identities.delete(target);
    try { persist(); }
    catch (failure) { identities.set(target, { requestId: id }); throw failure; }
  }

  async function loadOperations(): Promise<void> {
    const version = revision;
    const query = ++querySequence;
    const signal = lifetime.signal;
    const saved = [...identities];
    try {
      const responses: OperationResponse[] = [];
      if (!saved.length) responses.push(await request<OperationResponse>('/api/operations', { signal }));
      for (let offset = 0; offset < saved.length; offset += 100) {
        const ids = saved.slice(offset, offset + 100).map(([, identity]) => identity.requestId).join(',');
        responses.push(await request<OperationResponse>('/api/operations?ids=' + encodeURIComponent(ids), { signal }));
      }
      if (disposed || signal.aborted || query !== querySequence) return;
      // 并发提交改变身份时重新核对；只有新查询或会话结束可以替代本次核对。
      if (version !== revision) return loadOperations();
      const pending = new Map(responses.flatMap(value => value.operations).map(value => [value.id, value]));
      const results = new Map(responses.flatMap(value => value.results || []).map(value => [value.id, value]));
      const recovered: PendingOperation[] = [];
      const oldServer = saved.length > 0 && responses.some(value => !Array.isArray(value.results));
      for (const [target, identity] of saved) {
        const id = identity.requestId;
        if (pendingSends.has(target)) { pending.delete(id); continue; }
        const result = results.get(id);
        if (result && result.phase !== 'missing' && result.target !== target) throw new Error('操作归属与本地记录不一致。');
        if (result?.closed || result?.phase === 'not-dispatched' || result?.phase === 'accepted') {
          pending.delete(id);
          if (result.phase === 'accepted' && result.kind === 'create' && result.receipt?.threadId) {
            // 保留 ID 直至用户查看原聊天；刷新仍能恢复，不清旧草稿。
            recovered.push(result);
          } else forget(target, id);
        } else {
          pending.set(id, result && result.phase !== 'missing' ? result : pending.get(id)
            || { id, target, kind: target.startsWith('new:') ? 'create' : 'message', phase: 'unknown', localOnly: true });
        }
      }
      unresolvedOperations.value = [...pending.values()];
      recoveredCreations.value = recovered;
      operationsReady.value = !oldServer;
      operationsError.value = oldServer ? '电脑版本不支持查询已完成的操作，请更新电脑端后重新核对；草稿已保留。' : '';
    } catch (failure) {
      if (disposed || signal.aborted || query !== querySequence) return;
      if (version !== revision) return loadOperations();
      operationsReady.value = false;
      operationsError.value = '操作状态暂时无法核对；阅读和草稿仍可使用。' + (failure instanceof Error && !(failure instanceof ApiError) ? failure.message : '');
    }
  }

  function blocked(target: string) {
    if (!operationsReady.value) return '操作状态尚未核对，请先重新核对。';
    if (pendingSends.has(target)) return '正在发送，请等待回执。';
    if (unresolvedOperations.value.some(operation => operation.target === target)) return '此前操作结果尚未确认，请先核对原聊天。';
    if (recoveredCreations.value.some(operation => operation.target === target)) return '聊天已创建，请先查看原聊天。';
    if (identities.has(target)) return '此前操作结果待确认，请先重新核对。';
    return '';
  }
  function readiness(state: SubmissionReadiness) {
    if (!state.target) return '请先选择聊天。';
    if (state.firstTurnPending) return '正在核对新聊天首轮，请稍候。';
    if (!state.capable) return '当前连接暂不支持此操作，草稿会保留。';
    if (!state.connected) return '连接恢复后可发送，草稿已保留。';
    if (state.missingSource) return '请先选择来源聊天和官方项目。';
    const operation = blocked(state.target);
    if (operation) return operation;
    if (state.uploading) return '图片正在上传，完成后可发送。';
    if (state.imageErrors) return '请先重试或移除失败的图片。';
    if (state.imageCount && !state.imageSupported) return '当前连接暂不支持图片发送，图片草稿已保留。';
    if (state.voiceBusy) return '请先结束语音输入。';
    if (!state.hasContent) return '请输入消息或添加图片。';
    return '';
  }

  async function submit<T>({ target, kind, url, input, started }: {
    target: string; kind: 'message' | 'create'; url: string; input: Record<string, unknown>; started?: (id: string) => void;
  }): Promise<{ result: T; requestId: string }> {
    const reason = blocked(target);
    if (disposed || reason) throw new Error(reason || '页面已关闭。');
    const requestId = Array.from(crypto.getRandomValues(new Uint8Array(16))).map(value => value.toString(16).padStart(2, '0')).join('');
    // JSON 在等待网络前固定文字、附件、模型；不持久化正文。
    const body = JSON.stringify({ ...input, requestId });
    identities.set(target, { requestId });
    try { persist(); }
    catch { identities.delete(target); throw new Error('浏览器无法保存操作编号，尚未发送；请检查存储空间。'); }
    ++revision;
    pendingSends.add(target);
    const signal = lifetime.signal;
    try {
      started?.(requestId);
      const result = await request<T>(url, { method: 'POST', body, timeoutMs: 65_000, signal });
      if (signal.aborted) throw signal.reason;
      ++revision;
      if (kind === 'create') {
        recoveredCreations.value.push({ id: requestId, target, kind, phase: 'accepted', receipt: result as OperationReceipt });
      } else forget(target, requestId);
      unresolvedOperations.value = unresolvedOperations.value.filter(operation => operation.id !== requestId);
      return { result, requestId };
    } catch (failure) {
      if (!disposed && !signal.aborted) {
        ++revision;
        if (failure instanceof ApiError && failure.outcome === 'not-dispatched') forget(target, requestId);
        pendingSends.delete(target);
        await loadOperations();
      }
      throw failure;
    } finally { if (!signal.aborted) pendingSends.delete(target); }
  }

  async function closeOperation(operation: PendingOperation) {
    if (operation.phase !== 'unknown') return;
    if (!window.confirm('请先在桌面核对操作结果。结束等待不会撤销此前操作；之后再次发送属于新的操作，可能重复。')) return;
    const signal = lifetime.signal;
    try {
      if (!operation.localOnly) await request('/api/operations/' + encodeURIComponent(operation.id), {
        method: 'POST', body: JSON.stringify({ confirm: 'close-unknown' }), signal,
      });
      if (disposed || signal.aborted) return;
      ++revision;
      forget(operation.target, operation.id);
      await loadOperations();
    } catch (failure) { if (!disposed && !signal.aborted) operationsError.value = (failure as Error).message; }
  }

  function acknowledgeCreation(operation: PendingOperation) {
    try { forget(operation.target, operation.id); }
    catch { operationsError.value = '创建结果已确认，但浏览器无法保存状态，请重新核对。'; return; }
    recoveredCreations.value = recoveredCreations.value.filter(value => value.id !== operation.id);
    ++revision;
  }
  function invalidate() {
    ++revision;
    lifetime.abort();
    lifetime = new AbortController();
    pendingSends.clear();
    unresolvedOperations.value = [];
    recoveredCreations.value = [];
    operationsReady.value = false;
    operationsError.value = '';
  }
  onScopeDispose(() => { disposed = true; invalidate(); });
  return { pendingSends, unresolvedOperations, recoveredCreations, operationsReady, operationsError, loadOperations, closeOperation, readiness, submit, acknowledgeCreation, invalidate };
}
