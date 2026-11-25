import test from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { effectScope, nextTick, ref } from 'vue';
import { useMessageQueue } from '../../mobile/src/features/composer/useMessageQueue.ts';

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
