import type { DesktopThread } from '../../types';

export interface ContextEntry {
  id: string;
  kind: 'plugin' | 'file' | 'browser' | 'thread';
  label: string;
  description: string;
  mention: string;
}

export interface ContextCatalog {
  mode?: 'references' | 'context';
  plugins: ContextEntry[];
  browserTabs: ContextEntry[];
  files: ContextEntry[];
  browserMessage?: string;
  pluginMessage?: string;
  fileMessage?: string;
}

export function threadContext(thread: DesktopThread): ContextEntry {
  const label = thread.title.replace(/[\r\n]+/g, ' ').replace(/[\\[\]]/g, '\\$&');
  return { id: thread.id, kind: 'thread', label: thread.title, description: thread.cwd || 'Codex 对话',
    mention: `[${label}](codex://threads/${encodeURIComponent(thread.id)})` };
}

export interface DraftContext { mention: string; label: string }

// 上下文保存在草稿首部；输入框只展示正文，发送和恢复草稿时仍使用同一份文本。
export function splitDraftContext(value: string) {
  const contexts: DraftContext[] = [];
  let text = value;
  const reference = /^\[((?:\\.|[^\]\\\r\n])+)\]\(((?:plugin:\/\/|codex:\/\/threads\/)[^\s()]*|<(?:[A-Za-z]:[\\/]|\/)[^>\r\n]*>)\)(?:\r?\n|$)/;
  let match: RegExpExecArray | null;
  while ((match = reference.exec(text))) {
    contexts.push({ mention: match[0].trimEnd(), label: match[1]!.replace(/\\([\\[\]])/g, '$1') });
    text = text.slice(match[0].length);
  }
  if (contexts.length) text = text.replace(/^\r?\n/, '');
  return { contexts, text };
}

export function composeDraftContext(contexts: Pick<DraftContext, 'mention'>[], text: string) {
  return contexts.length ? contexts.map((entry) => entry.mention).join('\n') + '\n\n' + text : text;
}
