import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope, nextTick, ref, watchEffect } from 'vue';
import { useImageAttachments } from '../../mobile/src/features/composer/useImageAttachments.ts';

const image = (letter = 'a') => ({ id: letter.repeat(32) + '.png', name: '图片.png', url: '/api/images/' + letter.repeat(32) + '.png', size: 12, type: 'image/png' });
const photo = () => new File([new Uint8Array([1, 2, 3])], '图片.png', { type: 'image/png' });

function fixture(t, respond) {
  const previousFetch = globalThis.fetch;
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const saved = new Map();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: key => saved.get(key) || null, setItem: (key, value) => saved.set(key, value),
  } });
  globalThis.fetch = respond;
  const key = ref('chat-a');
  const scopes = [];
  const open = () => { const scope = effectScope(); scopes.push(scope); return { scope, attachments: scope.run(() => useImageAttachments({ draftKey: () => key.value })) }; };
  t.after(() => {
    for (const scope of scopes) scope.stop();
    globalThis.fetch = previousFetch;
    if (previousStorage) Object.defineProperty(globalThis, 'localStorage', previousStorage);
    else delete globalThis.localStorage;
  });
  return { ...open(), key, saved, open };
}

test('多图上传使用现有认证请求，重开保留已上传图片草稿且按对话分开', async (t) => {
  const requests = [];
  const { attachments, key, scope, open } = fixture(t, async (url, options) => {
    requests.push({ url, options });
    return Response.json(image(requests.length === 1 ? 'a' : 'b'));
  });
  await attachments.add([photo(), photo()]);
  assert.equal(attachments.uploading.value, false);
  assert.equal(attachments.images.value.length, 2);
  assert.ok(attachments.images.value.every(entry => entry.status === 'ready'));
  assert.equal(requests[0].url, '/api/images');
  assert.equal(new Headers(requests[0].options.headers).get('Content-Type'), 'image/png');
  assert.equal(requests[0].options.credentials, 'same-origin');
  assert.ok(requests[0].options.body instanceof File);
  key.value = 'chat-b';
  assert.equal(attachments.images.value.length, 0);
  key.value = 'chat-a';
  const before = attachments.readyImages();
  scope.stop();
  const reopened = open().attachments;
  assert.deepEqual(reopened.readyImages(), before);
  assert.ok(reopened.images.value.every(entry => entry.previewUrl === undefined));
  reopened.clear('chat-a', [before[0].id]);
  assert.deepEqual(reopened.readyImages().map(entry => entry.id), [before[1].id]);
});

test('首次只上传一张图片也会更新已订阅的界面状态并持久化草稿', async (t) => {
  let complete;
  let visible;
  const { attachments, saved, scope, open } = fixture(t, () => new Promise(resolve => { complete = () => resolve(Response.json(image())); }));
  scope.run(() => watchEffect(() => {
    visible = { uploading: attachments.uploading.value, states: attachments.images.value.map(entry => entry.status) };
  }));
  assert.deepEqual(visible, { uploading: false, states: [] });
  const pending = attachments.add([photo()]);
  await nextTick();
  assert.deepEqual(visible, { uploading: true, states: ['uploading'] });
  complete();
  await pending;
  await nextTick();
  assert.deepEqual(visible, { uploading: false, states: ['ready'] });
  assert.deepEqual(JSON.parse(saved.get('codex-mobile-image-drafts')), { 'chat-a': [image()] });
  scope.stop();
  assert.deepEqual(open().attachments.readyImages(), [image()]);
});

test('上传期间打开的本地图片预览在上传成功后仍有效，移除草稿才回收', async (t) => {
  const readBlob = globalThis.fetch;
  let complete;
  const { attachments, saved } = fixture(t, () => new Promise(resolve => { complete = () => resolve(Response.json(image())); }));
  const pending = attachments.add([photo()]);
  const selectedPreviewUrl = attachments.images.value[0].previewUrl;
  assert.match(selectedPreviewUrl, /^blob:/);
  assert.deepEqual(new Uint8Array(await (await readBlob(selectedPreviewUrl)).arrayBuffer()), new Uint8Array([1, 2, 3]));
  complete();
  await pending;
  const draft = attachments.images.value[0];
  assert.equal(draft.status, 'ready');
  assert.equal(draft.url, image().url);
  assert.equal(draft.previewUrl, selectedPreviewUrl);
  assert.deepEqual(new Uint8Array(await (await readBlob(selectedPreviewUrl)).arrayBuffer()), new Uint8Array([1, 2, 3]));
  assert.deepEqual(attachments.readyImages(), [image()]);
  assert.ok(!saved.get('codex-mobile-image-drafts').includes('blob:'));
  assert.ok(!saved.get('codex-mobile-image-drafts').includes('previewUrl'));
  attachments.remove(draft.id);
  await assert.rejects(readBlob(selectedPreviewUrl), TypeError);
});

test('发送清稿只回收已发送图片的预览，剩余图片在卸载时回收', async (t) => {
  const readBlob = globalThis.fetch;
  let calls = 0;
  const { attachments, scope } = fixture(t, async () => Response.json(image(++calls === 1 ? 'a' : 'b')));
  await attachments.add([photo(), photo()]);
  const first = { ...attachments.images.value[0] };
  const second = { ...attachments.images.value[1] };
  attachments.clear('chat-a', [first.id]);
  await assert.rejects(readBlob(first.previewUrl), TypeError);
  assert.equal((await readBlob(second.previewUrl)).status, 200);
  scope.stop();
  await assert.rejects(readBlob(second.previewUrl), TypeError);
});

test('上传失败保留预览并可重试，切换对话不串附件，删除会取消未完成请求', async (t) => {
  let attempts = 0;
  let complete;
  let signal;
  const { attachments, key } = fixture(t, (_url, options) => {
    attempts++;
    if (attempts === 1) return Promise.resolve(Response.json({ message: '上传失败，请重试' }, { status: 503 }));
    signal = options.signal;
    return new Promise(resolve => { complete = () => resolve(Response.json(image())); });
  });
  await attachments.add([photo()]);
  assert.equal(attachments.images.value[0].status, 'error');
  assert.equal(attachments.hasErrors.value, true);
  assert.match(attachments.images.value[0].error, /上传失败/);
  const uploadId = attachments.images.value[0].id;
  const retry = attachments.retry(uploadId);
  assert.equal(attachments.uploading.value, true);
  key.value = 'chat-b';
  assert.equal(attachments.uploading.value, false);
  assert.equal(attachments.hasPendingUploads.value, true);
  complete();
  await retry;
  assert.equal(attachments.images.value.length, 0);
  key.value = 'chat-a';
  assert.equal(attachments.images.value[0].status, 'ready');
  assert.equal(attachments.hasPendingUploads.value, false);
  const pending = attachments.add([photo()]);
  const last = attachments.images.value.at(-1);
  attachments.remove(last.id);
  assert.equal(signal.aborted, true);
  complete();
  await pending;
  assert.equal(attachments.images.value.length, 1);
});

test('退出清理所有聊天的图片草稿并取消当前和后续上传', async (t) => {
  let complete;
  let signal;
  let calls = 0;
  const { attachments, key, saved } = fixture(t, (_url, options) => {
    calls++;
    if (calls === 1) return Promise.resolve(Response.json(image()));
    signal = options.signal;
    return new Promise(resolve => { complete = () => resolve(Response.json(image('b'))); });
  });
  await attachments.add([photo()]);
  key.value = 'chat-b';
  const pending = attachments.add([photo(), photo()]);
  assert.equal(attachments.hasPendingUploads.value, true);
  attachments.clearAll();
  assert.equal(signal.aborted, true);
  assert.equal(attachments.hasPendingUploads.value, false);
  complete();
  await pending;
  assert.equal(calls, 2);
  assert.equal(attachments.images.value.length, 0);
  key.value = 'chat-a';
  assert.equal(attachments.images.value.length, 0);
  assert.equal(saved.get('codex-mobile-image-drafts'), '{}');
});

test('卸载取消当前图片上传后不会启动下一张', async (t) => {
  let calls = 0;
  let complete;
  let signal;
  const { attachments, scope } = fixture(t, (_url, options) => {
    calls++;
    signal = options.signal;
    return new Promise(resolve => { complete = () => resolve(Response.json(image())); });
  });
  const pending = attachments.add([photo(), photo()]);
  scope.stop();
  assert.equal(signal.aborted, true);
  complete();
  await pending;
  assert.equal(calls, 1);
});

test('发送期间添加的新图片不会被清稿误删，不支持的类型不发请求', async (t) => {
  let calls = 0;
  const { attachments } = fixture(t, async () => Response.json(image(++calls === 1 ? 'a' : 'b')));
  await attachments.add([photo()]);
  const submitted = attachments.readyImages().map(entry => entry.id);
  await attachments.add([photo()]);
  attachments.clear('chat-a', submitted);
  assert.deepEqual(attachments.readyImages().map(entry => entry.id), [image('b').id]);
  await attachments.add([new File(['<svg/>'], 'image.svg', { type: 'image/svg+xml' })]);
  assert.equal(calls, 2);
  assert.match(attachments.error.value, /PNG/);
});
