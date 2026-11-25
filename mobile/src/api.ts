import type { ThreadItem } from './types';
import { desktopFetch, RelayError } from './transport.ts';

export class ApiError extends Error {
  status: number;
  outcome?: string;
  constructor(message: string, status: number, outcome?: string) { super(message); this.status = status; this.outcome = outcome; }
}

export type RequestOptions = RequestInit & { timeoutMs?: number };

export async function request<T>(url: string, options: RequestOptions = {}, read: (response: Response) => Promise<T> = response => response.json()): Promise<T> {
  // 桌面 RPC 最多等待 30 秒；请求额外保留响应传输时间。
  const { timeoutMs = 35_000, signal, ...init } = options;
  if (signal?.aborted) throw signal.reason;
  const headers = new Headers(init.headers);
  headers.set('x-codex-mobile-contract', '2');
  if (init.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
  const controller = new AbortController();
  const cancel = () => controller.abort(signal?.reason);
  signal?.addEventListener('abort', cancel, { once: true });
  let timedOut = false;
  const timeout = setTimeout(() => {
    if (controller.signal.aborted) return;
    timedOut = true;
    controller.abort();
  }, timeoutMs);
  try {
    let response: Response;
    try {
      response = await desktopFetch(url, { ...init, credentials: 'same-origin', headers, signal: controller.signal });
    } catch (failure) {
      if (controller.signal.aborted || (failure as Error).name === 'AbortError') throw failure;
      if (failure instanceof RelayError) throw new ApiError(failure.message, failure.status);
      throw new ApiError('无法连接电脑上的 Codex Mobile 服务。请先在电脑启动服务，并确认设备处于同一网络。', 0);
    }
    let value;
    try {
      value = response.ok ? await read(response) : await response.json();
    } catch (failure) {
      if (controller.signal.aborted || (failure as Error).name === 'AbortError') throw failure;
      if (failure instanceof RelayError) throw new ApiError(failure.message, failure.status);
      throw new ApiError(response.ok
        ? '电脑服务返回的数据格式异常，请刷新后重试。'
        : `连接请求返回了异常页面（HTTP ${response.status}），请检查电脑服务状态与访问地址后重试。`, response.status);
    }
    if (!response.ok) throw new ApiError(value.message || '请求失败。', response.status, value.outcome);
    return value as T;
  } catch (error) {
    if (timedOut) {
      const reading = !init.method || ['GET', 'HEAD'].includes(init.method.toUpperCase());
      throw new ApiError(reading ? '请求超时，请刷新后重试。' : '等待操作结果超时，操作可能已提交，请刷新确认后再试。', 408);
    }
    if (signal?.aborted) throw signal.reason;
    throw error;
  } finally {
    clearTimeout(timeout);
    signal?.removeEventListener('abort', cancel);
  }
}

export function threadUrl(threadId: string, action: string) {
  return '/api/threads/' + encodeURIComponent(threadId) + '/' + action;
}

const itemRequests = new Map<string, Promise<ThreadItem>>();

export function requestItem(threadId: string, itemId: string, version: string): Promise<ThreadItem> {
  const key = JSON.stringify([threadId, itemId, version]);
  let pending = itemRequests.get(key);
  if (!pending) {
    pending = request<{ item: ThreadItem }>(threadUrl(threadId, 'item') + '?itemId=' + encodeURIComponent(itemId))
      .then((result) => result.item).finally(() => itemRequests.delete(key));
    itemRequests.set(key, pending);
  }
  return pending;
}
