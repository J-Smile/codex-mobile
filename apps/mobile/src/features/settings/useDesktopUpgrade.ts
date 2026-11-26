import { onScopeDispose, ref } from 'vue';
import { ApiError, request } from '../../api.ts';

export type DesktopUpdateSettings = {
  version: string;
  appServer?: { connected: boolean; version: string | null };
  mode: 'source' | 'paused' | 'automatic';
  update?: {
    phase: 'idle' | 'queued' | 'checking' | 'current' | 'ready' | 'applying' | 'error';
    message: string;
    stage?: 'checking' | 'downloading' | 'verifying' | 'extracting';
    downloadedBytes?: number;
    totalBytes?: number | null;
    elapsedMs?: number;
    stageElapsedMs?: number;
    stageTimingsMs?: Record<string, number>;
  };
};

export function useDesktopUpgrade(finished: (result: { message: string; failed?: boolean; updated?: boolean }) => void) {
  const upgrading = ref(false);
  const reconnecting = ref(false);
  const progress = ref<DesktopUpdateSettings['update']>();
  let controller: AbortController | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function stop() {
    controller?.abort();
    controller = undefined;
    clearTimeout(timer);
    upgrading.value = false;
    reconnecting.value = false;
    progress.value = undefined;
  }

  async function start(version: string) {
    if (controller) return;
    const pending = new AbortController();
    controller = pending;
    upgrading.value = true;
    progress.value = { phase: 'queued', message: '正在请求电脑准备升级…' };
    let submissionUnconfirmed = false;
    try {
      await request('/api/update', { method: 'POST', signal: pending.signal, timeoutMs: 10_000 });
    } catch (failure) {
      if (pending.signal.aborted) return;
      if (failure instanceof ApiError && ![0, 408, 502, 503, 504].includes(failure.status)) {
        stop();
        throw failure;
      }
      // 连接中断时电脑可能已经收到请求，先查询状态再提示重试。
      submissionUnconfirmed = true;
    }
    if (pending.signal.aborted) return;
    let disconnectedAt: number | undefined;
    function finish(message: string, outcome: 'current' | 'updated' | 'failed') {
      stop();
      finished({ message, failed: outcome === 'failed', updated: outcome === 'updated' });
    }

    async function poll() {
      if (!document.hidden) {
        try {
          const settings = await request<DesktopUpdateSettings>('/api/update-settings', { signal: pending.signal, timeoutMs: 4000 });
          if (pending.signal.aborted) return;
          disconnectedAt = undefined;
          if (settings.version !== version) return finish(`已更新到 v${settings.version}。`, 'updated');
          progress.value = settings.update;
          const state = settings.update;
          reconnecting.value = state?.phase === 'applying';
          switch (state?.phase) {
            case 'current': return finish('当前已是最新正式版本。', 'current');
            case 'error': return finish(state.message, 'failed');
            case 'queued': case 'checking': case 'ready': case 'applying': break;
            default: return finish(submissionUnconfirmed ? '未能确认电脑已收到升级请求，请重试。' : '升级已中断，请重试。', 'failed');
          }
        } catch (failure) {
          if (pending.signal.aborted) return;
          if (failure instanceof ApiError && failure.status === 401) return finish('连接已过期，请重新配对。', 'failed');
          disconnectedAt ??= Date.now();
          reconnecting.value = true;
          if (Date.now() - disconnectedAt >= 75_000) return finish('暂未重新连上电脑，请确认 Codex Mobile 服务已启动。', 'failed');
        }
      }
      timer = setTimeout(poll, 2000);
    }
    void poll();
  }

  onScopeDispose(stop);
  return { upgrading, reconnecting, progress, start, stop };
}
