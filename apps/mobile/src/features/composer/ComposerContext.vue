<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue';
import { ChatLineRound, Close, Connection, Document, Monitor, Search } from '@element-plus/icons-vue';
import { request } from '../../api';
import { threadContext } from './composerContext';
import type { ContextCatalog, ContextEntry } from './composerContext';
import type { DesktopThread } from '../../types';
import NativeSheet from '../../components/NativeSheet.vue';

const props = defineProps<{ open: boolean; threadId?: string; projectId?: string; sourceThreadId?: string; threads: DesktopThread[] }>();
const emit = defineEmits<{ close: []; select: [mention: string]; 'after-close': [] }>();
const titleId = useId();
const query = ref('');
const loading = ref(false);
const error = ref('');
const retry = ref(0);
const catalog = ref<ContextCatalog>({ plugins: [], browserTabs: [], files: [] });
const chatResults = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  if (!needle) return [];
  return props.threads.filter((thread) => thread.id !== props.threadId && `${thread.title} ${thread.cwd || ''}`.toLocaleLowerCase().includes(needle))
    .slice(0, 20).map(threadContext);
});
const sections = computed(() => [
  { id: 'plugins', title: '插件', items: catalog.value.plugins, empty: catalog.value.pluginMessage || '没有匹配的已启用插件' },
  { id: 'browser', title: '标签页', items: catalog.value.browserTabs, empty: catalog.value.browserMessage || '没有匹配的浏览器标签页' },
  { id: 'files', title: '文件和聊天', items: [...catalog.value.files, ...chatResults.value],
    empty: catalog.value.fileMessage || (query.value.trim() ? '没有匹配的文件或聊天' : '输入名称以查找文件或聊天') },
].filter((section) => section.id === 'files' || (!loading.value && !error.value && catalog.value.mode !== 'references'))
  .sort((a, b) => Number(b.items.length > 0) - Number(a.items.length > 0)));
const icons = { plugin: Connection, file: Document, browser: Monitor, thread: ChatLineRound };

watch([() => props.open, () => props.threadId, () => props.projectId, query, retry, () => props.sourceThreadId], ([open, threadId, projectId, search, , sourceThreadId], _, cleanup) => {
  catalog.value = { mode: catalog.value.mode, plugins: [], browserTabs: [], files: [] };
  loading.value = false;
  error.value = '';
  if (!open) return;
  loading.value = true;
  const controller = new AbortController();
  const timer = setTimeout(async () => {
    try {
      const params = new URLSearchParams({ query: search });
      if (threadId) params.set('threadId', threadId);
      else if (projectId) {
        params.set('projectId', projectId);
        if (sourceThreadId) params.set('sourceThreadId', sourceThreadId);
      }
      const result = await request<ContextCatalog>('/api/context?' + params.toString(), { signal: controller.signal });
      if (!controller.signal.aborted) catalog.value = result;
    } catch (failure) { if (!controller.signal.aborted) error.value = (failure as Error).message; }
    finally { if (!controller.signal.aborted) loading.value = false; }
  }, search.trim() ? 150 : 0);
  cleanup(() => { clearTimeout(timer); controller.abort(); });
}, { immediate: true });

function select(entry: ContextEntry) { emit('select', entry.mention); }
</script>

<template>
  <NativeSheet :open="open" :labelledby="titleId" panel-class="settings-modal" style="--sheet-width: 460px" @close="emit('close')" @after-close="query = ''; emit('after-close')">
      <section class="context-sheet">
        <header class="modal-heading"><h2 :id="titleId">添加上下文</h2><button type="button" class="icon-button" aria-label="关闭上下文选择" @click="emit('close')"><Close /></button></header>
        <label class="context-search"><Search /><input v-model="query" :aria-label="catalog.mode === 'references' ? '搜索文件或聊天' : '搜索插件、文件或聊天'" :placeholder="catalog.mode === 'references' ? '搜索文件或聊天' : '搜索插件、文件或聊天'" autocomplete="off" autofocus /><span v-if="loading" class="context-loading" role="status" aria-label="正在查找" /></label>
        <p v-if="catalog.mode === 'references'" class="context-empty">文件和聊天会作为文字引用加入消息，由当前聊天读取。</p>
        <div class="context-results" :aria-busy="loading">
          <div v-if="error" class="context-error" role="alert"><span>{{ error }}</span><button type="button" class="text-button" @click="retry++">重新读取上下文</button></div>
          <section v-for="section in sections" :key="section.id" class="context-section" :aria-labelledby="titleId + '-' + section.id">
            <h3 :id="titleId + '-' + section.id">{{ section.title }}</h3>
            <button v-for="entry in section.items" :key="entry.id" type="button" class="context-result" :title="entry.description" @click="select(entry)">
              <component :is="icons[entry.kind]" class="context-entry-icon" />
              <span class="context-result-copy"><strong>{{ entry.label }}</strong><small>{{ entry.description }}</small></span>
            </button>
            <p v-if="!section.items.length && !loading" class="context-empty">{{ section.empty }}</p>
          </section>
        </div>
      </section>
  </NativeSheet>
</template>

<style scoped>
.context-sheet { display: flex; flex-direction: column; max-height: min(560px, calc(var(--app-height, 100dvh) - var(--safe-top-visible) - 82px)); min-height: 0; }
.context-sheet .modal-heading { flex: none; }
.context-sheet .context-search { display: flex; flex: none; align-items: center; gap: 8px; min-height: 48px; margin: 4px 0 12px; padding: 0 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--page); }
.context-search > svg { width: 17px; height: 17px; color: var(--signal); flex: none; }
.context-search input { min-width: 0; min-height: 44px; width: 100%; padding: 10px 0; border: 0; outline: none; background: none; color: var(--text); font-size: 16px; }
.context-search input::placeholder { color: var(--muted); }
.context-search:focus-within { outline: 2px solid var(--signal); outline-offset: 1px; }
.context-search input:focus-visible { outline: none; }
.context-loading { width: 13px; height: 13px; flex: none; border: 1.5px solid var(--muted); border-right-color: transparent; border-radius: 50%; animation: context-spin .8s linear infinite; }
.context-results { min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
.context-section + .context-section { margin-top: 12px; border-top: 1px solid var(--border); padding-top: 12px; }
.context-section h3 { margin: 6px 10px; font-size: 13px; font-weight: 600; color: var(--muted); }
.context-result { display: flex; align-items: center; gap: 12px; width: 100%; min-height: 52px; padding: 8px 10px; border: 0; border-radius: 10px; background: none; color: var(--text); text-align: left; }
.context-result:hover, .context-result:focus-visible { background: var(--hover); }
.context-entry-icon { flex: none; width: 18px; height: 18px; color: var(--signal); }
.context-result-copy { display: block; min-width: 0; }
.context-result strong { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; font-weight: 500; line-height: 1.6; }
.context-result small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); font-size: 12px; line-height: 1.6; }
.context-empty { margin: 10px 10px 6px; color: var(--muted); font-size: 13px; line-height: 1.6; }
.context-error { display: flex; align-items: center; flex-wrap: wrap; gap: 4px 12px; padding: 10px; color: var(--error); font-size: 13px; line-height: 1.6; }
.context-error > span { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.context-error button { min-height: 44px; flex: none; font-size: 13px; }
button:focus-visible { outline: 2px solid var(--text); outline-offset: -2px; }
@keyframes context-spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .context-loading { animation: none; } }
</style>
