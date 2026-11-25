import { reactive, shallowRef, watch } from 'vue';
import { RelayConnection } from './connection.ts';
import type { Desktop } from './devices.ts';

const refreshInterval = 10_000;
const probeTimeout = 5_000;
const maxDesktopProbes = 2;

export function formatRelayLatency(sample?: number | null) {
  if (sample === undefined) return '测量中…';
  if (sample === null) return '暂不可测';
  return sample < 1 ? '<1 ms' : sample + ' ms';
}

async function probe(signal: AbortSignal, measure: (request: AbortSignal) => Promise<number | null>) {
  if (signal.aborted) return null;
  const request = new AbortController();
  const abort = () => request.abort();
  signal.addEventListener('abort', abort, { once: true });
  const timeout = setTimeout(abort, probeTimeout);
  try {
    const sample = await measure(request.signal);
    return request.signal.aborted ? null : sample;
  } catch {
    return null;
  } finally {
    clearTimeout(timeout);
    signal.removeEventListener('abort', abort);
  }
}

async function measureServer(signal: AbortSignal) {
  const start = performance.now();
  const response = await fetch('/relay/health', { cache: 'no-store', credentials: 'omit', redirect: 'error', signal });
  if (response.status === 204) return Math.round(performance.now() - start);
  await response.body?.cancel();
  return null;
}

export function useRelayLatency(options: {
  visibleAccount: () => string | undefined;
  desktops: () => Desktop[];
  activeConnection: (id: string) => RelayConnection | undefined;
}) {
  const serverLatency = shallowRef<number | null>();
  const desktopLatency = reactive(new Map<string, number | null>());
  const onlineIds = () => options.desktops().map(desktop => desktop.id).sort().join(',');

  watch([options.visibleAccount, onlineIds], ([account], [lastAccount], onCleanup) => {
    if (!account) return;
    if (account !== lastAccount) {
      serverLatency.value = undefined;
      desktopLatency.clear();
    }
    const desktops = options.desktops();
    const eligible = new Set(desktops.map(desktop => desktop.id));
    for (const id of desktopLatency.keys()) {
      if (!eligible.has(id)) desktopLatency.delete(id);
    }
    const controller = new AbortController();
    const { signal } = controller;
    let timer: ReturnType<typeof setTimeout> | undefined;

    async function refresh() {
      const health = probe(signal, measureServer).then(value => {
        if (!signal.aborted) serverLatency.value = value;
      });
      for (let offset = 0; offset < desktops.length && !signal.aborted; offset += maxDesktopProbes) {
        await Promise.all(desktops.slice(offset, offset + maxDesktopProbes).map(async desktop => {
          const value = await probe(signal, async request => {
            const active = options.activeConnection(desktop.id);
            const connection = active?.hasTunnel ? active : new RelayConnection(desktop);
            try {
              return await connection.measureLatency(request);
            } finally {
              if (connection !== active) connection.close();
            }
          });
          if (!signal.aborted) desktopLatency.set(desktop.id, value);
        }));
      }
      await health;
      if (!signal.aborted) timer = setTimeout(refresh, refreshInterval);
    }

    onCleanup(() => {
      controller.abort();
      clearTimeout(timer);
    });
    void refresh();
  }, { immediate: true });

  return { serverLatency, desktopLatency };
}
