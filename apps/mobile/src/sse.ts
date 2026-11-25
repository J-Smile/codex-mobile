export class SseReadError extends Error {}

export async function* readSse(response: Response) {
  if (!response.ok || !response.body || !response.headers.get('content-type')?.startsWith('text/event-stream')) {
    throw new SseReadError(`SSE 响应无效（HTTP ${response.status}）。`);
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = '';
  let type = 'message';
  let fields: string[] = [];
  try {
    for (;;) {
      const { value, done } = await reader.read().catch(failure => {
        throw new SseReadError('SSE 连接已中断。', { cause: failure });
      });
      if (done) return;
      pending += decoder.decode(value, { stream: true });
      let lineEnd: number;
      while ((lineEnd = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, lineEnd).replace(/\r$/, '');
        pending = pending.slice(lineEnd + 1);
        if (line.startsWith('event:')) type = line.slice(6).trimStart();
        else if (line.startsWith('data:')) fields.push(line.slice(5).replace(/^ /, ''));
        else if (!line) {
          if (fields.length) yield { type, data: fields.join('\n') };
          type = 'message';
          fields = [];
        }
      }
    }
  } finally {
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
