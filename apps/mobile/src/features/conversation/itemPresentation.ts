import type { ThreadItem } from '../../types';
import { isSubagentItem, subagentLabel, subagents } from './subagentPresentation.ts';

export function webCitationText(text: string) {
  if (!text.includes('\uE200')) return text;
  // 手机消息中的网页引用只有编号。流式分片可能停在标记中间。
  return text.replace(/\uE200cite((?:\uE202turn\d+[a-z]+\d+)+)\uE201/g, (_, refs: string) => {
    const count = new Set(refs.slice(1).split('\uE202')).size;
    return `〔${count} 处网页引用〕`;
  }).replace(/\uE200(?:c|ci|cit|cite(?:\uE202\w*)*)?$/, '');
}

export function chatCreationRequestText(item: ThreadItem): string | undefined {
  if (item.type !== 'functionCallOutput' || !['create_thread', 'send_message_to_thread'].includes(item.name)
    || item.namespace !== 'codex_app' || typeof item.output !== 'string') return;
  const delegation = /^\s*<codex_delegation>\s*<source_thread_id>([^<>]+)<\/source_thread_id>\s*<input>([\s\S]*)<\/input>\s*<\/codex_delegation>\s*$/.exec(item.output);
  if (delegation?.[1]?.trim() && delegation[2]?.trim()) return delegation[2];
}

export function itemRunning(item: ThreadItem) {
  return item.status === 'inProgress' || item.status === 'running';
}

export function itemFailed(item: ThreadItem) {
  return item.type === 'error' || item.status === 'failed' || (typeof item.exitCode === 'number' && item.exitCode !== 0);
}

export function browserActivity(item: ThreadItem) {
  return ['cua_repl', 'browser'].includes(item.server) || /browser|cua_repl/.test(item.tool || '');
}

export function runningActivityLabel(items: ThreadItem[]) {
  const running = [...items].reverse().find(itemRunning);
  if (running?.type === 'commandExecution') return '正在运行命令';
  if (running?.type === 'fileChange') return '正在修改文件';
  if (running && browserActivity(running)) return '正在操作浏览器';
  if (running?.type === 'webSearch') return '正在搜索网页';
  if (running) return '正在执行操作';
  return items.at(-1)?.type === 'agentMessage' ? '正在回复' : '正在思考';
}

export type ConversationBlock = { id: string; type: 'message' | 'activity'; items: ThreadItem[] };

export function conversationBlocks(items: ThreadItem[], previousBlocks: ConversationBlock[] = []) {
  const blocks: ConversationBlock[] = [];
  for (const item of items) {
    if (['reasoning', 'steered'].includes(item.type)) continue;
    const type = ['userMessage', 'agentMessage', 'imageView', 'plan'].includes(item.type)
      || chatCreationRequestText(item) !== undefined ? 'message' : 'activity';
    const previous = blocks.at(-1);
    if (type === 'activity' && previous?.type === 'activity') previous.items.push(item);
    else blocks.push({ id: item.id, type, items: [item] });
  }
  // 只追加正文时复用未变化的分组，避免重新渲染其摘要和已展开的工具列表。
  const previousById = new Map(previousBlocks.map(block => [block.id, block]));
  const result = blocks.map(block => {
    const previous = previousById.get(block.id);
    return previous?.type === block.type && previous.items.length === block.items.length
      && block.items.every((item, index) => item === previous.items[index]) ? previous : block;
  });
  return result.length === previousBlocks.length && result.every((block, index) => block === previousBlocks[index])
    ? previousBlocks : result;
}

export function activitySummary(items: ThreadItem[]) {
  const parts: string[] = [];
  if (items.some(browserActivity)) parts.push('使用了浏览器');
  const files = new Set(items.filter((item) => item.type === 'fileChange').flatMap((item) => (item.changes || []).map((change: any) => change.path)));
  if (files.size) parts.push('编辑了 ' + files.size + ' 个文件');
  const commands = items.filter((item) => item.type === 'commandExecution').length;
  if (commands) parts.push('运行了 ' + commands + ' 个命令');
  const agents = new Set(items.flatMap((item) => subagents(item).map((agent) => agent.id)));
  if (agents.size) parts.push('调用了 ' + agents.size + ' 个子智能体');
  return parts.join(' · ') || (items.length === 1 ? itemLabel(items[0]!) : '执行了 ' + items.length + ' 项操作');
}

export function fileChangeLabel(change: { kind?: { type?: string } | string }) {
  const kind = typeof change.kind === 'string' ? change.kind : change.kind?.type;
  return kind === 'add' ? '已创建' : kind === 'delete' ? '已删除' : '已编辑';
}

export function itemLabel(item: ThreadItem): string {
  if (chatCreationRequestText(item) !== undefined) return item.name === 'create_thread' ? '创建聊天请求' : '用户追加消息';
  if (isSubagentItem(item)) return subagentLabel(item);
  if (item.type === 'commandExecution') {
    const action = item.commandActions?.[0];
    if (action?.type === 'read') return '读取 ' + action.name;
    if (action?.type === 'listFiles') return '查看文件列表';
    if (action?.type === 'search') return '搜索 ' + (action.query || '文件内容');
    const command = String(item.command || '');
    let core = /(?:^|\s)-Command\s+([\s\S]+)$/i.exec(command)?.[1]?.trim() || command;
    if ((core[0] === '"' || core[0] === "'") && core.at(-1) === core[0]) core = core.slice(1, -1);
    return core.replace(/\s+/g, ' ').trim() || '执行命令';
  }
  if (typeof item.arguments?.title === 'string' && item.arguments.title.trim()) return item.arguments.title;
  if (item.type === 'fileChange') return (item.changes?.length || 0) + ' 个文件';
  const labels: Record<string, string> = {
    imageView: '查看图片',
    webSearch: '搜索网页', functionCallOutput: '工具返回',
    contextCompaction: '整理上下文', sleep: '等待任务',
    agentMessage: '正在整理回复', plan: '任务计划', error: '执行出错',
  };
  return labels[item.type] || item.tool || item.name || item.type;
}
