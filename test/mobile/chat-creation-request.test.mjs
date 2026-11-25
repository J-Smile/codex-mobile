import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { computed, createSSRApp, defineComponent, h, ref } from 'vue';
import { renderToString } from '@vue/server-renderer';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as filePreview from '../../mobile/src/features/conversation/preview/filePreview.ts';
import * as presentation from '../../mobile/src/features/conversation/itemPresentation.ts';
import * as subagentPresentation from '../../mobile/src/features/conversation/subagentPresentation.ts';

const seed = JSON.parse(readFileSync(new URL('../fixtures/official-create-thread-seed.json', import.meta.url), 'utf8'));
const prompt = 'Codex Mobile Rust HTTP isolated settings and queue test. Reply only READY. Do not call any tools.';
const source = readFileSync(new URL('../../mobile/src/features/conversation/ItemView.vue', import.meta.url), 'utf8');
const { descriptor } = parse(source, { filename: 'ItemView.vue' });
const script = compileScript(descriptor, { id: 'creation-request', inlineTemplate: true });
const compiled = ts.transpileModule(script.content, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const require = createRequire(import.meta.url);

async function render(item) {
  const previews = [], requests = [];
  const empty = defineComponent({ render: () => null });
  const imports = {
    './preview/MarkdownPreview.vue': { default: defineComponent({
      props: ['id', 'modelValue'],
      setup(props) {
        previews.push({ id: props.id, text: props.modelValue });
        return () => h('div', { class: 'markdown-preview' }, props.modelValue);
      },
    }) },
    dompurify: { default: { sanitize: value => value } },
    './clipboard': { copyText: async () => {} },
    '../../api': { threadUrl: () => '', requestItem: (...args) => { requests.push(args); throw new Error('Unexpected detail request'); } },
    '../../relayMedia': { mediaUrl: value => value },
    './preview/filePreview': filePreview,
    './itemPresentation': presentation,
    './subagentPresentation': subagentPresentation,
    './SubagentActivity.vue': { default: empty },
    '../../components/CollapseMotion.vue': { default: empty },
    './useConversationHistory': {
      useConversationDisclosure: () => ref(false),
      useConversationItemDetail: () => ({ detail: ref(), version: ref(), save() {} }),
    },
    './useStreamText': { useStreamText: value => computed(value) },
  };
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(
    name => imports[name] || require(name), module, module.exports,
  );
  const app = createSSRApp(module.exports.default, { item, threadId: 'existing-chat', theme: 'light' });
  return { html: await renderToString(app), previews, requests };
}

test('official create_thread seed exposes its exact original request without changing the item', () => {
  const original = Object.freeze({ ...seed });
  assert.equal(presentation.chatCreationRequestText(original), prompt);
  assert.equal(presentation.itemLabel(original), '创建聊天请求');
  assert.deepEqual(original, seed);
  const text = '  原始请求🙂\n检查 `<root>` 与 &lt;原文&gt;。  ';
  const wrapped = { ...seed, output: seed.output.replace(prompt, text) };
  assert.equal(presentation.chatCreationRequestText(wrapped), text);
});

test('the official seed remains one message block with the original identity and order', () => {
  const command = { id: 'command', type: 'commandExecution' };
  const answer = { id: 'answer', type: 'agentMessage', text: 'READY' };
  const queuedUser = { id: 'queue-user', type: 'userMessage' };
  const blocks = presentation.conversationBlocks([command, seed, { id: 'reason', type: 'reasoning' }, answer, queuedUser]);
  assert.deepEqual(blocks.map(block => [block.id, block.type]), [
    ['command', 'activity'], [seed.id, 'message'], ['answer', 'message'], ['queue-user', 'message'],
  ]);
  assert.equal(blocks[1].items[0], seed);
  assert.equal(blocks[1].items[0].type, 'functionCallOutput');
  assert.equal(blocks.flatMap(block => block.items).filter(item => item.id === seed.id).length, 1);
  assert.equal(blocks.flatMap(block => block.items).filter(item => item.type === 'userMessage').length, 1);
});

test('unrelated tools and incomplete or quoted delegation examples remain tool outputs', () => {
  const variants = [
    { name: 'read_file' }, { namespace: 'other_server' }, { namespace: undefined }, { type: 'mcpToolCall' },
    { output: 'ordinary create_thread result' }, { output: { text: seed.output } },
    { output: 'Example: ' + seed.output },
    { output: '<codex_delegation><input>request</input></codex_delegation>' },
    { output: '<codex_delegation><source_thread_id> </source_thread_id><input>request</input></codex_delegation>' },
    { output: '<codex_delegation><source_thread_id>control</source_thread_id><input> </input></codex_delegation>' },
  ];
  for (const change of variants) {
    const item = { ...seed, ...change };
    assert.equal(presentation.chatCreationRequestText(item), undefined);
    assert.equal(presentation.conversationBlocks([item])[0].type, 'activity');
  }
});

test('the actual ItemView renderer displays the request once without a collapsed tool record', async () => {
  const { html, previews, requests } = await render(Object.freeze({ ...seed }));
  assert.match(html, /aria-label="创建聊天请求"/);
  assert.match(html, /creation-request-label/);
  assert.ok(html.includes(prompt));
  assert.ok(!html.includes('工具返回'));
  assert.ok(!html.includes('<details'));
  assert.deepEqual(previews, [{ id: 'creation-' + seed.id, text: prompt }]);
  assert.deepEqual(requests, []);
});

test('the actual renderer keeps other tools labelled as tool output', async () => {
  const { html, previews } = await render({ ...seed, namespace: 'other_server' });
  assert.ok(html.includes('工具返回'));
  assert.ok(html.includes('<details'));
  assert.ok(!html.includes('创建聊天请求'));
  assert.deepEqual(previews, []);
});

test('official follow-up input uses the message renderer without duplicate tool details', async () => {
  const item = { ...seed, name: 'send_message_to_thread' };
  assert.equal(presentation.chatCreationRequestText(item), prompt);
  const { html, previews, requests } = await render(item);
  assert.match(html, /aria-label="用户追加消息"/);
  assert.ok(!html.includes('创建聊天请求'));
  assert.deepEqual(previews, [{ id: 'creation-' + seed.id, text: prompt }]);
  assert.deepEqual(requests, []);
});
