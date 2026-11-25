import { desktopFetch, relayMode } from './transport.ts';
import { readSse } from './sse.ts';

export function desktopEvents(url: string) {
  if (!relayMode()) return new EventSource(url);
  const target = new EventTarget();
  const controller = new AbortController();
  let retry: ReturnType<typeof setTimeout>;
  let delay = 800;
  const current = {
    addEventListener: target.addEventListener.bind(target),
    onerror: null as (() => void) | null,
    close() { clearTimeout(retry); controller.abort(); },
  };
  async function consume() {
    const request = new AbortController();
    const abort = () => request.abort();
    controller.signal.addEventListener('abort', abort, { once: true });
    const timeout = setTimeout(abort, 35_000);
    try {
      const response = await desktopFetch(url, { signal: request.signal });
      clearTimeout(timeout);
      for await (const { type, data } of readSse(response)) {
        if (controller.signal.aborted) return;
        delay = 800;
        target.dispatchEvent(new MessageEvent(type, { data }));
        if (controller.signal.aborted) return;
      }
    } catch {
      // 保持 EventSource 的断线语义，由现有聊天逻辑确认配对是否仍有效。
    } finally {
      clearTimeout(timeout);
      controller.signal.removeEventListener('abort', abort);
      request.abort();
    }
    if (controller.signal.aborted) return;
    current.onerror?.();
    if (controller.signal.aborted) return;
    retry = setTimeout(consume, delay);
    delay = Math.min(delay * 2, 5_000);
  }
  void consume();
  return current;
}
