import test from 'node:test';
import assert from 'node:assert/strict';
import { composeDraftContext, splitDraftContext, threadContext } from '../../mobile/src/features/composer/composerContext.ts';

test('chat references preserve the chat ID and escape titles instead of creating additional markdown', () => {
  assert.equal(threadContext({ id: 'chat-id', title: 'UI [review]' }).mention, '[UI \\[review\\]](codex://threads/chat-id)');
});

test('草稿显示与编辑正文时保留上下文，移除引用不修改正文或普通 Markdown 链接', () => {
  const contexts = [{ mention: '[Browser](plugin://browser@openai-bundled?tabId=page)' }, { mention: '[UI \\[review\\].vue](<D:/work/UI [review].vue>)' }];
  const text = '检查这个页面。\n\n[文档](https://example.com)\n保留换行。';
  const draft = composeDraftContext(contexts, text);
  const parts = splitDraftContext(draft);
  assert.deepEqual(parts.contexts.map((entry) => entry.label), ['Browser', 'UI [review].vue']);
  assert.equal(parts.text, text);
  assert.equal(composeDraftContext(parts.contexts, parts.text), draft);
  assert.equal(splitDraftContext(composeDraftContext(parts.contexts.slice(1), '修改后的正文')).text, '修改后的正文');
  assert.equal(composeDraftContext([], text), text);
  for (const ordinary of ['[文档](<https://example.com>)', '[Browser](plugin://browser@test) 后面的正文', '正文 [Browser](plugin://browser@test)']) {
    assert.deepEqual(splitDraftContext(ordinary), { contexts: [], text: ordinary });
  }
});
