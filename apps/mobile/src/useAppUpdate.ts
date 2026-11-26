import { onMounted, onScopeDispose, ref, watch } from 'vue';

// installed 表示整个 install 已成功；还要核对构建版本，不能把旧 active 当成新资源已就绪。
function waitForBuild(registration: Promise<ServiceWorkerRegistration>, version: string, signal: AbortSignal) {
  return new Promise<ServiceWorker | undefined>((resolve) => {
    const listeners = new Map<ServiceWorker, () => void>();
    const channels = new Map<ServiceWorker, MessageChannel>();
    let current: ServiceWorkerRegistration | undefined;
    let finished = false;
    function finish(worker?: ServiceWorker) {
      if (finished) return;
      finished = true;
      signal.removeEventListener('abort', cancel);
      current?.removeEventListener('updatefound', inspect);
      listeners.forEach((listener, target) => target.removeEventListener('statechange', listener));
      channels.forEach(({ port1, port2 }) => { port1.close(); port2.close(); });
      resolve(worker);
    }
    function cancel() { finish(); }
    function inspect() {
      if (finished || !current) return;
      for (const worker of [current.installing, current.waiting, current.active]) {
        if (!worker) continue;
        if (!listeners.has(worker)) {
          const changed = () => worker.state === 'redundant' ? finish() : inspect();
          listeners.set(worker, changed);
          worker.addEventListener('statechange', changed);
        }
        if (worker.state === 'installing' || worker.state === 'redundant' || channels.has(worker)) continue;
        const channel = new MessageChannel();
        channels.set(worker, channel);
        channel.port1.onmessage = ({ data }) => {
          if (data?.version === version && worker.state !== 'redundant') finish(worker);
        };
        worker.postMessage({ type: 'codex-mobile-build' }, [channel.port2]);
      }
    }
    signal.addEventListener('abort', cancel, { once: true });
    if (signal.aborted) return finish();
    void registration.then((value) => {
      if (finished) return;
      current = value;
      current.addEventListener('updatefound', inspect);
      inspect();
      if (!finished) void current.update().then(inspect, cancel);
    }).catch(cancel);
  });
}

export function useAppUpdate(options: { enabled: boolean; canReload: () => boolean; beforeReload: () => boolean }) {
  const available = ref(false);
  const error = ref('');
  const currentVersion = document.querySelector<HTMLMetaElement>('meta[name="app-version"]')?.content;
  const supportsWorker = 'serviceWorker' in navigator && window.isSecureContext;
  let registration: Promise<ServiceWorkerRegistration> | undefined;
  // HTTP 直连没有 Worker，继续使用在线刷新；有 Worker 时必须拿到对应构建的就绪回执。
  let ready: ServiceWorker | true | undefined;
  let controller: AbortController | undefined;
  let interval: ReturnType<typeof setInterval> | undefined;
  let blurTimer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  let reloading = false;

  function register() {
    registration ??= navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' }).catch((cause) => {
      registration = undefined;
      throw cause;
    });
    return registration;
  }

  function reload() {
    if (!available.value || !ready || (ready !== true && ready.state === 'redundant')
      || controller || disposed || reloading || document.hidden || !options.canReload()) return;
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
    if (!options.enabled || controller || disposed || reloading || document.hidden) return;
    ready = undefined;
    const pending = new AbortController();
    controller = pending;
    const timeout = setTimeout(() => pending.abort(), 30_000);
    try {
      const response = await fetch('/version.json', { cache: 'no-store', signal: pending.signal });
      if (!response.ok) return;
      const latest = await response.json() as { version?: string };
      if (typeof latest.version !== 'string' || !latest.version || !currentVersion || pending.signal.aborted || disposed) return;
      available.value = latest.version !== currentVersion;
      if (available.value) {
        const prepared = supportsWorker ? await waitForBuild(register(), latest.version, pending.signal) : true;
        if (!pending.signal.aborted && !disposed) ready = prepared;
      }
    } catch {
      // 连接服务切换版本期间可能暂时无法读取，下次检查继续。
    } finally {
      clearTimeout(timeout);
      if (controller === pending) controller = undefined;
      reload();
    }
  }

  function resume() {
    if (document.hidden) {
      ready = undefined;
      controller?.abort();
      controller = undefined;
      return;
    }
    void check();
  }
  function afterInput() {
    clearTimeout(blurTimer);
    blurTimer = setTimeout(reload, 0);
  }
  watch(options.canReload, reload, { flush: 'post' });
  onMounted(() => {
    if (!options.enabled) return;
    if (supportsWorker) void register().catch(() => {});
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
