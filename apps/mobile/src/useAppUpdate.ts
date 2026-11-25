import { onMounted, onScopeDispose, ref, watch } from 'vue';

export function useAppUpdate(options: { enabled: boolean; canReload: () => boolean; beforeReload: () => boolean }) {
  const available = ref(false);
  const error = ref('');
  const currentVersion = document.querySelector<HTMLMetaElement>('meta[name="app-version"]')?.content;
  let registration: ServiceWorkerRegistration | undefined;
  let controller: AbortController | undefined;
  let interval: ReturnType<typeof setInterval> | undefined;
  let blurTimer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  let reloading = false;

  function reload() {
    if (!available.value || disposed || reloading || document.hidden || !options.canReload()) return;
    if (document.querySelector('[role="dialog"]')) return;
    const focused = document.activeElement as HTMLElement | null;
    if (focused?.matches('input, textarea, select') || focused?.isContentEditable) return;
    if (!options.beforeReload()) {
      error.value = '草稿暂时无法保存，请先复制草稿再刷新。';
      return;
    }
    reloading = true;
    window.location.reload();
  }

  async function check() {
    if (!options.enabled || controller || disposed || document.hidden) return;
    const pending = new AbortController();
    controller = pending;
    const timeout = setTimeout(() => pending.abort(), 10_000);
    try {
      const response = await fetch('/version.json', { cache: 'no-store', signal: pending.signal });
      if (!response.ok) return;
      const latest = await response.json() as { version?: string };
      if (!latest.version || !currentVersion || pending.signal.aborted || disposed) return;
      available.value = latest.version !== currentVersion;
      if (available.value) {
        void registration?.update().catch(() => {});
        reload();
      }
    } catch {
      // 连接服务切换版本期间可能暂时无法读取，下次检查继续。
    } finally {
      clearTimeout(timeout);
      if (controller === pending) controller = undefined;
    }
  }

  function resume() {
    if (document.hidden) {
      controller?.abort();
      controller = undefined;
      return;
    }
    reload();
    if (!reloading) void check();
  }
  function afterInput() {
    clearTimeout(blurTimer);
    blurTimer = setTimeout(reload, 0);
  }
  watch(options.canReload, reload, { flush: 'post' });
  onMounted(() => {
    if (!options.enabled) return;
    if ('serviceWorker' in navigator && window.isSecureContext) {
      void navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' })
        .then((value) => { registration = value; }).catch(() => {});
    }
    void check();
    interval = setInterval(resume, 60_000);
    document.addEventListener('visibilitychange', resume);
    document.addEventListener('focusout', afterInput);
    window.addEventListener('focus', resume);
    window.addEventListener('online', resume);
    window.addEventListener('pageshow', resume);
  });
  onScopeDispose(() => {
    disposed = true;
    controller?.abort();
    clearInterval(interval);
    clearTimeout(blurTimer);
    document.removeEventListener('visibilitychange', resume);
    document.removeEventListener('focusout', afterInput);
    window.removeEventListener('focus', resume);
    window.removeEventListener('online', resume);
    window.removeEventListener('pageshow', resume);
  });
  return { available, error, check, reload };
}
