import { desktopApiPath, desktopFetch, relayDesktop } from './transport.ts';

const handles = new Map<string, string>();
const paths = new Map<string, string>();

export function mediaUrl(url: string) {
  if (!relayDesktop()) return url;
  const path = desktopApiPath(url);
  if (!path) return url;
  let handle = handles.get(path);
  if (!handle) {
    handle = '/relay-media/' + crypto.randomUUID();
    handles.set(path, handle);
    paths.set(handle, path);
  }
  return handle;
}

export function listenForRelayMedia(worker: ServiceWorkerContainer) {
  function forwardMedia(event: MessageEvent) {
    if (event.source !== worker.controller || event.data?.type !== 'codex-relay-media') return;
    const port = event.ports[0];
    if (!port) return;
    const url = paths.get(event.data.handle);
    if (!url) { port.postMessage({ status: 404 }); port.close(); return; }
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 35_000);
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    function close() { clearTimeout(timeout); port.close(); }
    port.onmessage = async ({ data }) => {
      if (data === 'cancel') { controller.abort(); close(); return; }
      if (data !== 'pull' || !reader) return;
      try {
        const { value, done } = await reader.read();
        if (value) port.postMessage({ chunk: value }, [value.buffer as ArrayBuffer]);
        if (done) { port.postMessage({ done }); close(); }
      } catch { port.postMessage({ error: true }); close(); }
    };
    const { range, head } = event.data;
    void desktopFetch(url, {
      signal: controller.signal, method: head ? 'HEAD' : 'GET', headers: typeof range === 'string' ? { range } : undefined,
    }).then(response => {
      clearTimeout(timeout);
      reader = response.body?.getReader();
      port.postMessage({ status: response.status, headers: [...response.headers], body: !!reader });
      if (!reader) close();
    }, () => { port.postMessage({ status: 503 }); close(); });
  }
  worker.addEventListener('message', forwardMedia);
  return () => {
    worker.removeEventListener('message', forwardMedia);
    handles.clear();
    paths.clear();
  };
}
