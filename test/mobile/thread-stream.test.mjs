import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope, nextTick, ref } from 'vue';
import { mergeStreamUpdate, useThreadStream, StreamBaselineError } from '../../mobile/src/features/conversation/useThreadStream.ts';

const settle = async () => { await new Promise((resolve) => setImmediate(resolve)); await nextTick(); };

function fixture(t, onUpdate) {
  const original = Object.fromEntries(['document', 'window', 'EventSource', 'fetch'].map((key) => [key, globalThis[key]]));
  const document = Object.assign(new EventTarget(), { hidden: false, querySelector: () => null });
  const window = new EventTarget();
  const sources = [];
  class EventSource extends EventTarget {
    constructor(url) { super(); this.url = url; sources.push(this); }
    close() { this.closed = true; }
    emit(name, value) { this.dispatchEvent(new MessageEvent(name, { data: JSON.stringify(value) })); }
    fail() { this.onerror?.(); }
  }
  Object.assign(globalThis, { document, window, EventSource, fetch: async () => Response.json({ local: true }) });
  const threadId = ref('first');
  const authenticated = ref(true);
  const snapshots = [];
  const updates = [];
  let expired = 0;
  const scope = effectScope();
  const stream = scope.run(() => useThreadStream({
    threadId: () => threadId.value, authenticated: () => authenticated.value,
    snapshot: (value) => snapshots.push(value), update: (value) => { onUpdate?.(value); updates.push(value); },
    expired: () => { expired++; authenticated.value = false; },
  }));
  t.after(() => {
    scope.stop();
    for (const [key, value] of Object.entries(original)) {
      if (value === undefined) delete globalThis[key];
      else globalThis[key] = value;
    }
  });
  return { scope, stream, document, window, sources, threadId, snapshots, updates, expired: () => expired };
}

test('增量只更新变化轮次，保留更早历史及正文引用，确认消息后清理临时条目', () => {
  const older = { id: 'older', status: 'completed', items: [{ id: 'answer', type: 'agentMessage', text: '历史' }] };
  const completed = { id: 'completed', status: 'completed', items: [{ id: 'previous', type: 'agentMessage', text: '上一轮' }] };
  const current = { id: 'current', status: 'inProgress', items: [{ id: 'pending', type: 'userMessage' }] };
  const confirmed = { id: 'confirmed', type: 'userMessage', clientId: 'pending' };
  const update = {
    connection: 'live', thread: { id: 'chat', title: '聊天' },
    turns: [{ id: 'completed', status: 'completed', itemOrder: ['previous'] }, { id: 'current', status: 'inProgress', itemOrder: ['confirmed'] }],
    updates: [{ turnId: 'current', item: confirmed }],
  };
  const merged = mergeStreamUpdate([older, completed, current], update);
  assert.equal(merged[0], older);
  assert.equal(merged[1], completed);
  assert.deepEqual(merged[2].items, [confirmed]);
  const unchanged = mergeStreamUpdate(merged, { ...update, updates: [] });
  assert.equal(unchanged[2], merged[2]);
  const finished = mergeStreamUpdate(unchanged, { ...update, turns: [{ ...update.turns[1], status: 'completed' }], updates: [] });
  assert.equal(finished[2].status, 'completed');
  assert.equal(finished[2].items[0], confirmed);
});

test('文字追加与完整消息混合更新，缺失基线拒绝拼接且不修改旧状态', () => {
  const original = { id: 'answer', type: 'agentMessage', text: '中文🙂' };
  const turns = [{ id: 'turn', status: 'inProgress', items: [original] }];
  const update = { connection: 'live', thread: { id: 'chat' }, turns: [{ id: 'turn', status: 'inProgress', itemOrder: ['answer', 'tool'] }],
    updates: [{ turnId: 'turn', item: { id: 'tool', type: 'commandExecution', status: 'inProgress' } }],
    textAppends: [{ turnId: 'turn', itemId: 'answer', offset: original.text.length, text: '\n新文字' }] };
  const result = mergeStreamUpdate(turns, update);
  assert.equal(result[0].items[0].text, '中文🙂\n新文字');
  assert.equal(result[0].items[1].id, 'tool');
  assert.equal(original.text, '中文🙂');
  assert.throws(() => mergeStreamUpdate(result, update), StreamBaselineError);
  assert.throws(() => mergeStreamUpdate([], update), StreamBaselineError);
  const corrected = mergeStreamUpdate(result, { ...update, textAppends: [], updates: [{ turnId: 'turn', item: { ...original, text: '最终更正' } }] });
  assert.equal(corrected[0].items[0].text, '最终更正');
});

test('追加基线不匹配时关闭旧连接并通过新快照恢复', async (t) => {
  const f = fixture(t, (value) => mergeStreamUpdate([], value));
  const first = f.sources[0];
  first.emit('update', { connection: 'live', textAppends: [{ turnId: 'turn', itemId: 'answer', offset: 512, text: '追加' }] });
  assert.ok(first.closed);
  assert.equal(f.stream.status.value, 'connecting');
  assert.equal(f.updates.length, 0);
  await settle();
  assert.equal(f.sources.length, 2);
  first.emit('snapshot', { connection: 'live', turns: [{ id: 'stale' }] });
  assert.equal(f.snapshots.length, 0);
  const snapshot = { connection: 'live', turns: [{ id: 'turn', items: [{ id: 'answer', type: 'agentMessage', text: '完整正文' }] }] };
  f.sources[1].emit('snapshot', snapshot);
  assert.deepEqual(f.snapshots, [snapshot]);
  assert.equal(f.stream.status.value, 'connected');
  assert.equal(f.stream.error.value, '');
});

test('首包直接初始化，切换聊天后忽略旧连接，销毁时释放连接和监听', async (t) => {
  const f = fixture(t);
  const first = f.sources[0];
  assert.equal(first.url, '/api/threads/first/events?textAppends=1&compactTextAppends=1');
  assert.equal(f.stream.loading.value, true);
  first.emit('snapshot', { connection: 'history', turns: [], nextCursor: 'older' });
  assert.equal(f.stream.loading.value, false);
  assert.equal(f.snapshots[0].nextCursor, 'older');
  f.threadId.value = 'second';
  await nextTick();
  assert.ok(first.closed);
  first.emit('snapshot', { connection: 'live', turns: [{ id: 'stale' }] });
  assert.equal(f.snapshots.length, 1);
  f.sources[1].emit('update', { connection: 'live', turns: [] });
  assert.equal(f.stream.status.value, 'connected');
  assert.equal(f.updates.length, 1);
  f.scope.stop();
  assert.ok(f.sources[1].closed);
  f.window.dispatchEvent(new Event('pageshow'));
  await settle();
  assert.equal(f.sources.length, 2);
});

test('手机短暂切换保留连接，真实会话过期停止重连并返回登录', async (t) => {
  const f = fixture(t);
  f.sources[0].emit('snapshot', { connection: 'live', turns: [] });
  f.document.hidden = true;
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.equal(f.sources.length, 1);
  f.document.hidden = false;
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.equal(f.sources[0].closed, undefined);
  assert.equal(f.sources.length, 1);
  globalThis.fetch = async () => Response.json({ message: '连接过期' }, { status: 401 });
  f.sources[0].fail();
  await settle();
  assert.equal(f.expired(), 1);
  assert.ok(f.sources[0].closed);
  f.window.dispatchEvent(new Event('online'));
  await settle();
  assert.equal(f.sources.length, 1);
});

test('恢复前台会取消仍在等待的旧登录检查，旧请求不能关闭新连接', async (t) => {
  const f = fixture(t);
  let pendingSignal;
  globalThis.fetch = (_, options) => new Promise((resolve, reject) => {
    pendingSignal = options.signal;
    options.signal.addEventListener('abort', () => reject(new DOMException('取消', 'AbortError')));
  });
  f.sources[0].fail();
  globalThis.fetch = async () => Response.json({ local: true });
  f.window.dispatchEvent(new Event('pageshow'));
  await settle();
  assert.ok(pendingSignal.aborted);
  assert.equal(f.sources.length, 2);
  assert.ok(!f.sources[1].closed);
  assert.equal(f.stream.error.value, '');
});

test('实时快照恢复后取消旧会话检查，迟到的 401 不会退出当前连接', async (t) => {
  const f = fixture(t);
  let reply, signal;
  globalThis.fetch = (_, options) => new Promise(resolve => { reply = resolve; signal = options.signal; });
  const current = f.sources[0];
  current.fail();
  current.emit('snapshot', { connection: 'history', turns: [] });
  assert.equal(signal.aborted, false);
  assert.equal(f.stream.status.value, 'history');
  current.emit('snapshot', { connection: 'live', turns: [{ id: 'latest' }] });
  assert.equal(signal.aborted, true);
  reply(Response.json({ message: '旧会话已过期' }, { status: 401 }));
  await settle();
  assert.equal(f.expired(), 0);
  assert.equal(current.closed, undefined);
  assert.equal(f.stream.status.value, 'connected');
  assert.equal(f.stream.error.value, '');
});

test('实时更新在超时检查期间恢复后，迟到的检查结果不会重连或覆盖输出状态', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval', 'Date'], now: 1_000 });
  const f = fixture(t);
  const current = f.sources[0];
  current.emit('snapshot', { connection: 'live', turns: [] });
  for (const response of [Response.json({ local: true }), Response.json({ message: '旧连接失败' }, { status: 503 })]) {
    let reply, signal;
    globalThis.fetch = (_, options) => new Promise(resolve => { reply = resolve; signal = options.signal; });
    t.mock.timers.tick(45_000);
    assert.equal(f.stream.status.value, 'disconnected');
    current.emit('update', { connection: 'live', turns: [] });
    assert.equal(signal.aborted, true);
    reply(response);
    await settle();
    assert.equal(f.sources.length, 1);
    assert.equal(f.stream.status.value, 'connected');
    assert.equal(f.stream.error.value, '');
  }
});

test('收到心跳保持连接，静默断流超过三个周期后重新拉取快照', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval', 'Date'], now: 1_000 });
  const f = fixture(t);
  const first = f.sources[0];
  first.emit('snapshot', { connection: 'live', turns: [] });
  t.mock.timers.tick(40_000);
  await settle();
  assert.equal(f.sources.length, 1);
  first.emit('heartbeat', {});
  t.mock.timers.tick(40_000);
  await settle();
  assert.equal(f.sources.length, 1);
  t.mock.timers.tick(5_000);
  await settle();
  assert.ok(first.closed);
  assert.equal(f.sources.length, 2);
  f.sources[1].emit('snapshot', { connection: 'live', turns: [{ id: 'latest' }] });
  assert.equal(f.snapshots.at(-1).turns[0].id, 'latest');
});

test('实际断线时后台暂停重试，回前台恢复，旧心跳不覆盖新连接', async (t) => {
  const f = fixture(t);
  let checks = 0;
  globalThis.fetch = async () => { checks++; return Response.json({ local: true }); };
  f.document.hidden = true;
  f.document.dispatchEvent(new Event('visibilitychange'));
  assert.equal(f.sources[0].closed, undefined);
  f.sources[0].fail();
  assert.ok(f.sources[0].closed);
  f.document.hidden = false;
  f.window.dispatchEvent(new Event('focus'));
  await settle();
  assert.equal(checks, 0);
  assert.equal(f.sources.length, 2);
  f.sources[0].emit('snapshot', { connection: 'live', turns: [{ id: 'old' }] });
  assert.equal(f.snapshots.length, 0);
});

test('选图返回和重复焦点事件保留正常连接，不重复请求登录或重建流', async (t) => {
  const f = fixture(t);
  const first = f.sources[0];
  first.emit('snapshot', { connection: 'live', turns: [] });
  let checks = 0;
  globalThis.fetch = async () => { checks++; return Response.json({ local: true }); };
  f.window.dispatchEvent(new Event('focus'));
  f.window.dispatchEvent(new Event('pageshow'));
  f.window.dispatchEvent(new Event('online'));
  await settle();
  assert.equal(checks, 0);
  assert.equal(f.sources.length, 1);
  assert.equal(first.closed, undefined);
  assert.equal(f.stream.status.value, 'connected');
});

test('后台静默超过三个心跳周期时直接恢复实时流，无需等待登录探测', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval', 'Date'], now: 1_000 });
  const f = fixture(t);
  f.sources[0].emit('snapshot', { connection: 'live', turns: [] });
  f.document.hidden = true;
  f.document.dispatchEvent(new Event('visibilitychange'));
  t.mock.timers.tick(45_000);
  assert.equal(f.sources[0].closed, undefined);
  let checks = 0;
  globalThis.fetch = (_, { signal }) => new Promise((_, reject) => {
    checks++;
    signal.addEventListener('abort', () => reject(new DOMException('取消', 'AbortError')));
  });
  f.document.hidden = false;
  f.document.dispatchEvent(new Event('visibilitychange'));
  f.window.dispatchEvent(new Event('focus'));
  f.window.dispatchEvent(new Event('pageshow'));
  await settle();
  assert.equal(checks, 0);
  assert.equal(f.sources.length, 2);
  f.sources[1].emit('snapshot', { connection: 'live', turns: [] });
  assert.equal(f.stream.status.value, 'connected');
});
