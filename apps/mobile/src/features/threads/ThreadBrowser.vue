<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { ArrowLeft, ArrowRight, ChatDotRound, Close, Loading, Refresh, Search } from '@element-plus/icons-vue';
import { request } from '../../api';
import type { DesktopThread, Project } from '../../types';
import { threadActive } from './useThreadCatalog';

const props = defineProps<{ project: Project | null; initialQuery?: string }>();
const emit = defineEmits<{ back: []; select: [thread: DesktopThread] }>();
const query = ref(props.initialQuery || '');
const entries = ref<DesktopThread[]>([]);
const nextCursor = ref<string | null>(null);
const loading = ref(false);
const error = ref('');
const retryFromStart = ref(true);
const list = ref<HTMLElement>();
const searchInput = ref<HTMLInputElement>();
const title = computed(() => props.project?.label || '全部聊天');
const rowHeight = 52;
const scrollTop = ref(0);
const viewportHeight = ref(0);
const start = computed(() => Math.max(0, Math.floor(scrollTop.value / rowHeight) - 5));
const end = computed(() => Math.min(entries.value.length, Math.ceil((scrollTop.value + viewportHeight.value) / rowHeight) + 5));
const visibleEntries = computed(() => entries.value.slice(start.value, end.value));
let controller: AbortController | undefined;
let debounce: ReturnType<typeof setTimeout> | undefined;

async function load(reset = false) {
  if (!reset && (loading.value || !nextCursor.value)) return;
  controller?.abort();
  const pending = new AbortController();
  controller = pending;
  loading.value = true;
  error.value = '';
  const params = new URLSearchParams();
  if (props.project) params.set('projectId', props.project.projectId);
  if (query.value.trim()) params.set('query', query.value.trim());
  if (!reset && nextCursor.value) params.set('cursor', nextCursor.value);
  try {
    const result = await request<{ threads: DesktopThread[]; nextCursor: string | null }>('/api/thread-catalog?' + params, { signal: pending.signal });
    if (pending.signal.aborted) return;
    if (reset) {
      scrollTop.value = 0;
      if (list.value) list.value.scrollTop = 0;
    }
    const known = new Set(reset ? [] : entries.value.map(entry => entry.id));
    entries.value = [...(reset ? [] : entries.value), ...result.threads.filter(entry => !known.has(entry.id))];
    nextCursor.value = result.nextCursor;
    await nextTick();
    if (reset && list.value) list.value.scrollTop = 0;
  } catch (failure) {
    if (!pending.signal.aborted) {
      error.value = (failure as Error).message;
      retryFromStart.value = reset;
    }
  }
  finally {
    if (controller === pending) {
      loading.value = false;
      await nextTick();
      onScroll();
    }
  }
}
function onScroll() {
  const element = list.value;
  if (!element) return;
  scrollTop.value = element.scrollTop;
  viewportHeight.value = element.clientHeight;
  if (!error.value && element.scrollHeight - element.scrollTop - element.clientHeight < 300) void load();
}
function clearSearch() {
  query.value = '';
  searchInput.value?.focus({ preventScroll: true });
}
watch(query, () => {
  controller?.abort();
  controller = undefined;
  clearTimeout(debounce);
  entries.value = [];
  nextCursor.value = null;
  error.value = '';
  scrollTop.value = 0;
  if (list.value) list.value.scrollTop = 0;
  loading.value = true;
  debounce = setTimeout(() => void load(true), 250);
});
watch(list, (element, _, cleanup) => {
  if (!element) return;
  const observer = new ResizeObserver(onScroll);
  observer.observe(element);
  onScroll();
  cleanup(() => observer.disconnect());
}, { flush: 'post' });
onBeforeUnmount(() => { controller?.abort(); clearTimeout(debounce); });
void load(true);
</script>

<template>
  <section class="thread-browser" aria-label="聊天历史">
    <header class="browser-header">
      <button type="button" class="icon-button" aria-label="返回项目列表" @click="emit('back')"><ArrowLeft /></button>
      <h1>{{ title }}</h1>
      <button type="button" class="icon-button" aria-label="刷新聊天历史" :disabled="loading" @click="load(true)"><Refresh :class="{ 'is-spin': loading }" /></button>
    </header>
    <div class="browser-search">
      <Search aria-hidden="true" />
      <input ref="searchInput" v-model="query" type="search" :aria-label="project ? '搜索项目聊天' : '搜索全部聊天'" placeholder="搜索聊天" maxlength="200" />
      <button v-if="query" type="button" class="icon-button" aria-label="清空聊天搜索" @mousedown.prevent @click="clearSearch"><Close /></button>
    </div>
    <div ref="list" class="browser-list" :aria-busy="loading" @scroll.passive="onScroll">
      <nav aria-label="历史聊天列表" :style="{ paddingTop: start * rowHeight + 'px', paddingBottom: (entries.length - end) * rowHeight + 'px' }">
        <button v-for="thread in visibleEntries" :key="thread.id" type="button" class="thread-button browser-thread" @click="emit('select', thread)">
          <ChatDotRound class="browser-thread-icon" aria-hidden="true" />
          <span class="thread-title">{{ thread.title || '未命名聊天' }}</span>
          <span v-if="threadActive(thread)" class="thread-executing">执行中</span>
          <ArrowRight class="browser-thread-arrow" aria-hidden="true" />
        </button>
      </nav>
      <div v-if="error" class="browser-state browser-error">
        <p role="alert">{{ error }}</p>
        <button type="button" class="secondary-button" :disabled="loading" @click="load(retryFromStart)">{{ retryFromStart ? '重新读取' : '继续加载' }}</button>
      </div>
      <p v-else-if="loading" class="browser-status" role="status"><Loading class="is-spin" />{{ entries.length ? '正在加载…' : '正在读取聊天…' }}</p>
      <div v-else-if="!entries.length" class="browser-state">
        <component :is="query ? Search : ChatDotRound" aria-hidden="true" />
        <p role="status">{{ query ? '没有找到匹配的聊天' : '暂无聊天' }}</p>
        <button v-if="query" class="secondary-button" type="button" @click="clearSearch">返回全部结果</button>
      </div>
      <p v-else-if="!nextCursor" class="browser-status">已显示全部聊天</p>
    </div>
  </section>
</template>

<style scoped>
.thread-browser { --catalog-inset: clamp(16px, 5vw, 24px); display: flex; flex: 1; flex-direction: column; min-height: 0; height: 100%; background: var(--sidebar); }
.browser-header { display: flex; align-items: center; gap: 12px; padding: calc(var(--safe-top-visible) + var(--toolbar-padding)) var(--catalog-inset) var(--toolbar-padding); }
.browser-header .icon-button { background: var(--panel); color: var(--text); outline-offset: -3px; }
.browser-header .icon-button:hover:not(:disabled) { background: var(--hover); }
h1 { flex: 1; min-width: 0; margin: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 21px; font-weight: 650; }
.browser-search { display: flex; align-items: center; gap: 8px; min-height: 46px; margin: 6px var(--catalog-inset) 0; padding-left: 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--page); color: var(--muted); }
.browser-search > svg { width: 16px; height: 16px; }
.browser-search input { flex: 1; min-width: 0; width: 100%; min-height: 44px; padding: 8px 12px 8px 0; border: 0; outline: 0; background: transparent; color: var(--text); font-size: 16px; }
input::-webkit-search-cancel-button { display: none; }
.browser-search:focus-within { border-color: var(--signal); outline: 2px solid var(--signal); outline-offset: 1px; }
.browser-search input:focus-visible { outline: 0; }
.browser-list { flex: 1; min-height: 0; overflow-y: auto; margin-top: 14px; padding: 0 var(--catalog-inset) max(12px, var(--safe-bottom-visible)); overscroll-behavior-y: contain; overflow-anchor: none; }
.browser-list nav { display: flex; flex-direction: column; }
.browser-list .browser-thread { position: relative; flex: none; height: 52px; min-height: 52px; padding-inline: 10px; }
.browser-thread + .browser-thread::before { content: ''; position: absolute; inset: 0 10px auto; border-top: 1px solid var(--border); }
.browser-thread:focus-visible { outline-offset: -3px; }
.browser-thread.selected { background: var(--signal-soft); color: var(--signal); }
.browser-thread :is(.browser-thread-icon, .browser-thread-arrow) { color: var(--muted); }
.browser-thread.selected :is(.browser-thread-icon, .browser-thread-arrow) { color: var(--signal); }
.browser-thread-arrow { width: 14px; height: 14px; }
.browser-state { display: grid; justify-items: start; gap: 12px; padding: clamp(12px, 5vw, 24px) 12px; color: var(--muted); font-size: 14px; overflow-wrap: anywhere; }
.browser-state p { margin: 0; }
.browser-state > svg { width: 38px; height: 38px; padding: 10px; border-radius: 12px; background: var(--signal-soft); color: var(--signal); }
.browser-error { color: var(--error); }
.browser-state .secondary-button { min-height: 44px; font-size: 14px; }
.browser-status { display: flex; align-items: center; justify-content: center; gap: 6px; min-height: 44px; margin: 0; color: var(--muted); font-size: 12px; }
.browser-status svg { width: 14px; height: 14px; }
</style>
