import test from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { effectScope, nextTick, ref } from 'vue';
import { useMessageQueue } from '../../apps/mobile/src/features/composer/useMessageQueue.ts';

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
const response = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const snapshot = (revision, ids) => ({ revision, messages: ids.map((id) => ({ id, prompt: id, status: 'queued' })) });
function fixture(t, fetch) {
  t.mock.method(globalThis, 'fetch', fetch);
  const threadId = ref('first');
  const authenticated = ref(true);
  const scope = effectScope();
  const queue = scope.run(() => useMessageQueue({ threadId: () => threadId.value, authenticated: () => authenticated.value }));
  t.after(() => scope.stop());
  return { queue, threadId, authenticated };
}

test('Mobile 队列编辑保留修订号，排序和执行复用同一提交生命周期', async t => {
  const calls = [];
  const { queue } = fixture(t, async (url, options) => {
    if (options.method !== 'POST') return response(snapshot('version-1', ['a', 'b']));
    const input = JSON.parse(options.body); calls.push(input);
    return response(snapshot('version-' + (calls.length + 1), input.action === 'send' ? ['b'] : ['a', 'b']));
  });
  await setImmediate();
  assert.equal(await queue.edit('a', '修改后的文字', 'version-1'), true);
  assert.equal(await queue.edit('a', '旧编辑草稿', 'version-1'), false);
  assert.equal(calls.length, 1);
  assert.equal(await queue.reorder(['b', 'a'], 'version-2'), true);
  assert.equal(await queue.send('a'), true);
  assert.deepEqual(calls, [
    { action: 'edit', messageId: 'a', prompt: '修改后的文字', revision: 'version-1' },
    { action: 'reorder', messageIds: ['b', 'a'], revision: 'version-2' },
    { action: 'send', messageId: 'a', revision: 'version-3' },
  ]);
  assert.equal(queue.busy.value, false);
});

test('实时队列更新到达后，旧读取结果不会覆盖最新顺序', async (t) => {
  const pending = deferred();
  const { queue } = fixture(t, () => pending.promise);
  const latest = snapshot('latest', ['second', 'first']);
  queue.acceptSnapshot(latest);
  pending.resolve(response(snapshot('old', ['first', 'second'])));
  await setImmediate();
  assert.deepEqual(queue.queue.value, latest);
  assert.equal(queue.loading.value, false);
});

test('立即发送等待回执期间重复点击只提交一次，失败后保留队列', async t => {
  const pending = deferred();
  const calls = [];
  const original = snapshot('current', ['selected', 'next']);
  const { queue } = fixture(t, (_, options) => {
    if (options.method !== 'POST') return Promise.resolve(response(original));
    calls.push(JSON.parse(options.body));
    return pending.promise;
  });
  await setImmediate();
  const sending = queue.send('selected');
  assert.equal(queue.busy.value, true);
  assert.equal(await queue.send('selected'), false);
  assert.equal(calls.length, 1);
  pending.resolve(response({ message: '消息已发送，移除队列失败；再次操作只会重试移除。' }, 503));
  assert.equal(await sending, false);
  assert.deepEqual(queue.queue.value, original);
  assert.equal(queue.busy.value, false);
  assert.match(queue.error.value, /消息已发送/);
});

test('重连收到原生持久化队列后清除旧接口错误，不留下错误提示', async (t) => {
  const { queue } = fixture(t, () => Promise.resolve(response({ message: '找不到这个接口' }, 404)));
  await setImmediate();
  assert.match(queue.error.value, /找不到/);
  const restored = snapshot('restored', ['saved-message']);
  queue.acceptSnapshot(restored);
  assert.equal(queue.error.value, '');
  assert.deepEqual(queue.queue.value, restored);
});

test('删除过程中切换聊天，旧聊天返回结果不会改写新聊天队列', async (t) => {
  const pending = deferred();
  const { queue, threadId } = fixture(t, (url, options) => {
    if (options.method === 'POST') return pending.promise;
    return Promise.resolve(response(url.includes('/first/') ? snapshot('first', ['a']) : snapshot('second', ['b'])));
  });
  await setImmediate();
  const deleting = queue.remove('a');
  assert.equal(queue.busy.value, true);
  threadId.value = 'second';
  await nextTick();
  await setImmediate();
  pending.resolve(response(snapshot('first-empty', [])));
  await deleting;
  assert.deepEqual(queue.queue.value, snapshot('second', ['b']));
  assert.equal(queue.busy.value, false);
});











test('队列读取挂起会释放加载状态，保留已有消息并允许手动刷新', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const original = snapshot('initial', ['a', 'b']);
  let reads = 0;
  const { queue } = fixture(t, (_, options) => {
    if (++reads !== 2) return Promise.resolve(response(original));
    return new Promise((_, reject) => options.signal.addEventListener('abort', () => reject(options.signal.reason), { once: true }));
  });
  await setImmediate();
  const reading = queue.reload();
  t.mock.timers.tick(14_999);
  assert.equal(queue.loading.value, true);
  t.mock.timers.tick(1);
  await reading;
  assert.equal(queue.loading.value, false);
  assert.deepEqual(queue.queue.value, original);
  assert.match(queue.error.value, /读取队列超时.*刷新/);
  assert.equal(reads, 2);
  await queue.reload();
  assert.equal(queue.error.value, '');
  assert.deepEqual(queue.queue.value, original);
});
