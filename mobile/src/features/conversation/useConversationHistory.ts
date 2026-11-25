import { computed, getCurrentInstance, inject, onScopeDispose, provide, ref, shallowReactive, type InjectionKey } from 'vue';
import type { DesktopThread, ThreadItem, Turn } from '../../types';
import { relayDesktop } from '../../transport.ts';
import { conversationParams, setConversationAddress } from './links.ts';

export interface ConversationFrame {
  threadId: string;
  projectId: string;
  thread: DesktopThread | null;
  turns: Turn[];
  historyCursor: string | null;
  changedFileCount?: number;
  scrollTop: number;
  stickToBottom: boolean;
  newOutput: boolean;
}

interface NavigationEntry {
  key: string;
  parents: string[];
}

const stateKey = 'codexMobileConversation';
const disclosureKey: InjectionKey<Map<string, boolean>> = Symbol('conversation-disclosures');
type ItemDetail = { item: ThreadItem; version: string | undefined };
const itemDetailKey: InjectionKey<Map<string, ItemDetail>> = Symbol('conversation-item-details');

export function useConversationDisclosure(key: () => string) {
  const disclosures = inject(disclosureKey, undefined);
  const local = ref<boolean>();
  return computed({
    get: () => disclosures ? disclosures.get(key()) : local.value,
    set: (value: boolean | undefined) => {
      if (!disclosures) local.value = value;
      else if (value === undefined) disclosures.delete(key());
      else disclosures.set(key(), value);
    },
  });
}

export function useConversationItemDetail(key: () => string) {
  const details = inject(itemDetailKey, undefined) || shallowReactive(new Map<string, ItemDetail>());
  return {
    detail: computed(() => details.get(key())?.item),
    version: computed(() => details.get(key())?.version),
    save: (item: ThreadItem, version: string | undefined) => details.set(key(), { item, version }),
  };
}

export function useConversationHistory(options: {
  capture: () => ConversationFrame | undefined;
  activate: (threadId: string, projectId: string, saved?: ConversationFrame) => void | Promise<void>;
}) {
  // 只保留本页已经打开过的视图引用；实时数据仍由当前聊天的服务快照更新。
  const frames = new Map<string, ConversationFrame>();
  const recentFrames = new Map<string, ConversationFrame>();
  const disclosures = shallowReactive(new Map<string, boolean>());
  const details = shallowReactive(new Map<string, ItemDetail>());
  if (getCurrentInstance()) {
    provide(disclosureKey, disclosures);
    provide(itemDetailKey, details);
  }
  const parents = ref<string[]>([]);
  let nextKey = 0;
  let currentKey = newKey();
  let backing = false;
  let disposed = false;

  function newKey() { return 'conversation-' + Date.now() + '-' + ++nextKey; }

  function entry(value: unknown): NavigationEntry | undefined {
    if (!value || typeof value !== 'object') return;
    const state = (value as Record<string, unknown>)[stateKey] as Partial<NavigationEntry> | undefined;
    if (!state || typeof state.key !== 'string' || !Array.isArray(state.parents) || !state.parents.every((key) => typeof key === 'string')) return;
    return { key: state.key, parents: state.parents };
  }

  function state() {
    return { ...window.history.state, [stateKey]: { key: currentKey, parents: [...parents.value] } };
  }

  function remember() {
    const current = options.capture();
    if (!current?.threadId) return;
    frames.set(currentKey, current);
    recentFrames.delete(current.threadId);
    recentFrames.set(current.threadId, current);
    if (recentFrames.size > 5) recentFrames.delete(recentFrames.keys().next().value!);
  }

  function reset(threadId?: string) {
    remember();
    // 从列表打开的聊天会替换当前页历史；只留仍可通过返回或前进到达的帧。
    frames.delete(currentKey);
    currentKey = newKey();
    parents.value = [];
    backing = false;
    window.history.replaceState(state(), '', window.location.href);
    return threadId ? recentFrames.get(threadId) : undefined;
  }

  function clear() {
    frames.clear();
    recentFrames.clear();
    disclosures.clear();
    details.clear();
    currentKey = newKey();
    parents.value = [];
    backing = false;
    window.history.replaceState(state(), '', window.location.href);
  }

  function openChild(threadId: string, projectId?: string) {
    const current = options.capture();
    if (!threadId || current?.threadId === threadId) return;
    remember();
    window.history.replaceState(state(), '', window.location.href);
    if (current?.threadId) parents.value = [...parents.value, currentKey];
    currentKey = newKey();
    const url = new URL(window.location.href);
    const project = projectId ?? current?.projectId ?? '';
    setConversationAddress(url, threadId, project);
    window.history.pushState(state(), '', url.href);
    void options.activate(threadId, project, recentFrames.get(threadId));
  }

  function back() {
    if (!parents.value.length) return false;
    if (backing) return true;
    backing = true;
    window.history.back();
    return true;
  }

  function onPopState() {
    if (disposed) return;
    backing = false;
    remember();
    const target = entry(window.history.state);
    currentKey = target?.key || newKey();
    parents.value = target?.parents || [];
    const url = new URL(window.location.href);
    const params = conversationParams(url);
    const threadId = params.get('thread') || '';
    const saved = frames.get(currentKey);
    const projectId = params.get('project') || saved?.projectId || '';
    void options.activate(threadId, projectId, saved?.threadId === threadId ? saved : undefined);
  }

  function openLink(event: MouseEvent) {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const anchor = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null;
    if (!anchor || anchor.hasAttribute('download') || (anchor.target && anchor.target !== '_self')) return;
    const url = new URL(anchor.href, window.location.href);
    if (url.origin !== window.location.origin || url.pathname !== window.location.pathname) return;
    const desktop = relayDesktop();
    if (desktop && url.searchParams.get('desktop') !== desktop) return;
    const params = conversationParams(url);
    const threadId = params.get('thread');
    if (!threadId) return;
    event.preventDefault();
    openChild(threadId, params.get('project') ?? undefined);
  }

  const initial = entry(window.history.state);
  if (initial) { currentKey = initial.key; parents.value = initial.parents; }
  window.addEventListener('popstate', onPopState);
  onScopeDispose(() => {
    disposed = true;
    frames.clear();
    recentFrames.clear();
    disclosures.clear();
    details.clear();
    window.removeEventListener('popstate', onPopState);
  });

  return {
    canGoBack: computed(() => parents.value.length > 0),
    parentTitle: computed(() => frames.get(parents.value.at(-1) || '')?.thread?.title || '上级对话'),
    openChild, openLink, back, reset, clear,
  };
}
