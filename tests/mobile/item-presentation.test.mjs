import test from 'node:test';
import assert from 'node:assert/strict';
import { activitySummary, conversationBlocks, fileChangeLabel, itemLabel, webCitationText } from '../../apps/mobile/src/features/conversation/itemPresentation.ts';

test('网页引用在流式分片收齐后显示数量，普通文字与链接保持原样', () => {
  const refs = '\uE200cite\uE202turn11view2\uE202turn3search4\uE202turn8view3\uE201';
  const text = '官网 [示例](https://example.com)，编号 turn11view2。';
  for (let end = 0; end < refs.length; end++) assert.equal(webCitationText(text + refs.slice(0, end)), text);
  assert.equal(webCitationText(text + refs + ' 继续。'), text + '〔3 处网页引用〕 继续。');
  assert.equal(webCitationText(refs + '\uE200cite\uE202turn4view1\uE202turn4view1\uE201'), '〔3 处网页引用〕〔1 处网页引用〕');
});

test('执行分组保留消息顺序，新的正文分隔操作，实时追加不改变组标识', () => {
  const items = [
    { id: 'user', type: 'userMessage' },
    { id: 'reason', type: 'reasoning' },
    { id: 'command', type: 'commandExecution' },
    { id: 'browser', type: 'mcpToolCall', server: 'cua_repl' },
    { id: 'progress', type: 'agentMessage', text: '正在检查' },
    { id: 'file', type: 'fileChange', changes: [{ path: 'a.vue', kind: { type: 'add' } }] },
    { id: 'steer', type: 'steered' },
  ];
  const blocks = conversationBlocks(items);
  assert.deepEqual(blocks.map((block) => [block.id, block.type, block.items.map((item) => item.id)]), [
    ['user', 'message', ['user']], ['command', 'activity', ['command', 'browser']],
    ['progress', 'message', ['progress']], ['file', 'activity', ['file']],
  ]);
  const next = conversationBlocks([...items, { id: 'command2', type: 'commandExecution' }]);
  assert.equal(next.at(-1).id, 'file');
  assert.equal(next.at(-1).items.length, 2);
  assert.equal(blocks.at(-1).items.length, 1);
  assert.equal(activitySummary(blocks[1].items), '使用了浏览器 · 运行了 1 个命令');
  assert.equal(fileChangeLabel(items[5].changes[0]), '已创建');
});

test('执行组按子线程计数，启动和完成事件不重复计算', () => {
  const started = { id: 'start', type: 'subAgentActivity', kind: 'started', agentThreadId: 'child-a', agentPath: '/root/research' };
  const completed = { ...started, id: 'end', kind: 'completed' };
  const other = { ...started, id: 'other', agentThreadId: 'child-b', agentPath: '/root/implementation' };
  assert.equal(activitySummary([started, completed, other]), '调用了 2 个子智能体');
  assert.equal(itemLabel(started), '已启动子智能体 · research');
  assert.equal(itemLabel(completed), '子智能体已完成 · research');
  assert.equal(itemLabel({ id: 'read', type: 'commandExecution', commandActions: [{ type: 'read', name: 'README.md' }] }), '读取 README.md');
});
