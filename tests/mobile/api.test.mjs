import test from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { ApiError, request, requestItem } from '../../apps/mobile/src/api.ts';
import { RelayError } from '../../apps/mobile/src/transport.ts';

test('服务不可达显示启动提示，主动取消不误报为服务故障', async (t) => {
  const original = globalThis.fetch;
  t.after(() => { globalThis.fetch = original; });
  globalThis.fetch = async () => { throw new TypeError('Failed to fetch'); };
  await assert.rejects(request('/api/session'), (error) => error instanceof ApiError && error.status === 0 && /启动服务/.test(error.message));
  const aborted = new DOMException('Cancelled', 'AbortError');
  globalThis.fetch = async () => { throw aborted; };
  await assert.rejects(request('/api/session'), (error) => error === aborted);
});

test('未登录仍返回认证错误，不当作服务停止', async (t) => {
  const original = globalThis.fetch;
  t.after(() => { globalThis.fetch = original; });
  globalThis.fetch = async () => new Response(JSON.stringify({ message: '请先配对。' }), {
    status: 401, headers: { 'Content-Type': 'application/json' },
  });
  await assert.rejects(request('/api/session'), (error) => error instanceof ApiError && error.status === 401);
});

test('代理返回 HTML 或纯文本错误时保留 HTTP 状态，不泄漏 Safari JSON 解析提示', async (t) => {
  const parseError = new SyntaxError('The string did not match the expected pattern');
  t.mock.method(globalThis, 'fetch', async () => ({
    ok: false, status: 403, json: async () => { throw parseError; },
  }));
  await assert.rejects(request('/api/threads'), error => error instanceof ApiError
    && error.status === 403 && /HTTP 403/.test(error.message) && /电脑服务状态与访问地址/.test(error.message));
});

test('JSON 正文截断时返回格式提示，读取中断仍保留取消原因', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => new Response('{"threads":', {
    headers: { 'Content-Type': 'application/json' },
  }));
  await assert.rejects(request('/api/threads'), error => error instanceof ApiError
    && error.status === 200 && /数据格式异常/.test(error.message));
  const cancelled = new DOMException('Cancelled', 'AbortError');
  t.mock.method(globalThis, 'fetch', async () => ({
    ok: true, status: 200, json: async () => { throw cancelled; },
  }));
  await assert.rejects(request('/api/threads'), error => error === cancelled);
});

test('已收到响应头后中转连接断开，保留中转错误和值', async (t) => {
  const relayError = new RelayError('电脑未能完成此请求，请重试。', 503);
  t.mock.method(globalThis, 'fetch', async () => new Response(new ReadableStream({
    start(body) {
      body.enqueue(new TextEncoder().encode('{"threads":'));
      body.error(relayError);
    },
  }), { headers: { 'Content-Type': 'application/json' } }));
  await assert.rejects(request('/api/threads'), error => error instanceof ApiError
    && error.status === relayError.status && error.message === relayError.message);
});

test('同一版本的文件详情合并读取，失败后可重试，新版本读取最新输出', async (t) => {
  const original = globalThis.fetch;
  t.after(() => { globalThis.fetch = original; });
  let resolve;
  let count = 0;
  globalThis.fetch = () => { count++; return new Promise((done) => { resolve = done; }); };
  const first = requestItem('chat', 'files', 'v1');
  const second = requestItem('chat', 'files', 'v1');
  assert.equal(first, second);
  assert.equal(count, 1);
  resolve(Response.json({ message: '暂未保存' }, { status: 404 }));
  await assert.rejects(first, /暂未保存/);
  const retry = requestItem('chat', 'files', 'v1');
  resolve(Response.json({ item: { id: 'files', changes: ['a', 'b'] } }));
  assert.deepEqual((await retry).changes, ['a', 'b']);
  const latest = requestItem('chat', 'files', 'v2');
  resolve(Response.json({ item: { id: 'files', changes: ['a', 'b', 'c'] } }));
  assert.equal((await latest).changes.length, 3);
  assert.equal(count, 3);
});

test('写操作在默认等待上限后释放请求，提示确认结果且不会自动重发', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let activeSignal;
  const fetch = t.mock.method(globalThis, 'fetch', (_, options) => {
    activeSignal = options.signal;
    return new Promise((_, reject) => options.signal.addEventListener('abort', () => reject(options.signal.reason), { once: true }));
  });
  const result = request('/api/threads/chat/interactions/reply', { method: 'POST', body: '{}' });
  const rejected = assert.rejects(result, error => error instanceof ApiError && error.status === 408 && /可能已提交.*刷新确认/.test(error.message));
  t.mock.timers.tick(34_999);
  assert.equal(activeSignal.aborted, false);
  t.mock.timers.tick(1);
  await rejected;
  assert.equal(activeSignal.aborted, true);
  assert.equal(fetch.mock.callCount(), 1);
});

test('响应头已收到但 JSON 正文未读完时，仍按配置超时', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let activeSignal;
  t.mock.method(globalThis, 'fetch', async (_, options) => {
    activeSignal = options.signal;
    return new Response(new ReadableStream({
      start(body) {
        body.enqueue(new TextEncoder().encode('{"ok":'));
        options.signal.addEventListener('abort', () => body.error(options.signal.reason), { once: true });
      },
    }), { headers: { 'Content-Type': 'application/json' } });
  });
  const result = request('/api/session', { timeoutMs: 120 });
  const rejected = assert.rejects(result, error => error instanceof ApiError && error.status === 408 && /请求超时.*刷新/.test(error.message));
  await setImmediate();
  t.mock.timers.tick(119);
  assert.equal(activeSignal.aborted, false);
  t.mock.timers.tick(1);
  await rejected;
});

test('主动取消保留调用方的取消原因，不变成网络错误或超时', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const caller = new AbortController();
  const reason = new Error('聊天已切换');
  const remove = t.mock.method(caller.signal, 'removeEventListener');
  t.mock.method(globalThis, 'fetch', (_, options) => new Promise((_, reject) => {
    options.signal.addEventListener('abort', () => reject(options.signal.reason), { once: true });
  }));
  const result = request('/api/threads/chat/queue', { signal: caller.signal, timeoutMs: 100 });
  const rejected = assert.rejects(result, error => error === reason);
  caller.abort(reason);
  t.mock.timers.tick(100);
  await rejected;
  assert.equal(remove.mock.callCount(), 1);
  assert.equal(remove.mock.calls[0].arguments[0], 'abort');
});

test('已取消的请求不会调用 fetch', async (t) => {
  const caller = new AbortController();
  caller.abort();
  const fetch = t.mock.method(globalThis, 'fetch', () => { throw new Error('不应发送'); });
  await assert.rejects(request('/api/session', { signal: caller.signal }), error => error === caller.signal.reason);
  assert.equal(fetch.mock.callCount(), 0);
});

test('完成和 HTTP 失败都清理计时器及取消监听，不影响后续状态', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const status of [200, 409]) {
    const caller = new AbortController();
    let activeSignal;
    const remove = t.mock.method(caller.signal, 'removeEventListener');
    t.mock.method(globalThis, 'fetch', async (_, options) => {
      activeSignal = options.signal;
      return Response.json({ message: '队列已变化。', ok: true }, { status });
    });
    const result = request('/api/threads/chat/queue', { signal: caller.signal, timeoutMs: 100 });
    if (status === 200) assert.equal((await result).ok, true);
    else await assert.rejects(result, error => error instanceof ApiError && error.status === 409);
    t.mock.timers.tick(100);
    caller.abort();
    assert.equal(activeSignal.aborted, false);
    assert.equal(remove.mock.callCount(), 1);
  }
});

test('JSON 默认请求头和显式图片请求头兼容 Headers 与数组写法', async (t) => {
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (_, options) => { calls.push(options); return Response.json({ ok: true }); });
  await request('/api/threads/chat/messages', { method: 'POST', body: '{}' });
  const image = new Blob(['image bytes'], { type: 'image/png' });
  await request('/api/images', {
    method: 'POST', body: image, timeoutMs: 60_000,
    headers: new Headers({ 'content-type': 'image/png', 'x-file-name': 'example.png' }),
  });
  await request('/api/threads/chat/messages', { method: 'POST', body: '{}', headers: [['Content-Type', 'application/json; charset=utf-8']] });
  assert.equal(calls[0].headers.get('Content-Type'), 'application/json');
  assert.equal(calls[1].headers.get('Content-Type'), 'image/png');
  assert.equal(calls[1].headers.get('X-File-Name'), 'example.png');
  assert.equal(calls[1].body, image);
  assert.equal(calls[2].headers.get('Content-Type'), 'application/json; charset=utf-8');
  assert.ok(calls.every(options => options.credentials === 'same-origin' && !Object.hasOwn(options, 'timeoutMs')));
});
