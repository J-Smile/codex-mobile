<script setup lang="ts">
import { onBeforeUnmount, ref, useId, watch } from 'vue';
import { ArrowRight, Close, Connection, Loading, Monitor } from '@element-plus/icons-vue';
import { request, threadUrl } from '../../api';
import NativeSheet from '../../components/NativeSheet.vue';

interface HookSource { label: string; path: string; config: Record<string, unknown> }
interface HookReviewData {
  mode: 'read-only';
  pendingReview: null;
  sources: HookSource[];
  issues: { path: string; message: string }[];
}
const props = defineProps<{ threadId: string | null }>();
const emit = defineEmits<{ open: []; 'after-close': [] }>();
const titleId = useId();
const open = ref(false);
const loading = ref(false);
const openingDesktop = ref(false);
const openedDesktop = ref(false);
const error = ref('');
const desktopError = ref('');
const data = ref<HookReviewData | null>(null);
let controller: AbortController | undefined;

async function load() {
  if (!props.threadId || !open.value) return;
  controller?.abort();
  const pending = new AbortController();
  controller = pending;
  loading.value = true;
  error.value = '';
  data.value = null;
  try {
    const result = await request<HookReviewData>(threadUrl(props.threadId, 'hooks'), { signal: pending.signal });
    if (!pending.signal.aborted) data.value = result;
  } catch (failure) { if (!pending.signal.aborted) error.value = (failure as Error).message; }
  finally { if (!pending.signal.aborted) loading.value = false; }
}
watch(() => props.threadId, () => { open.value = false; });
watch(open, async (visible, _, cleanup) => {
  cleanup(() => controller?.abort());
  if (!visible) return;
  openedDesktop.value = false;
  desktopError.value = '';
  await load();
});
onBeforeUnmount(() => controller?.abort());

function show() {
  if (!props.threadId) return;
  open.value = true;
  emit('open');
}
function events(source: HookSource) {
  const hooks = source.config.hooks;
  return hooks && typeof hooks === 'object' ? Object.keys(hooks).join(' · ') : '';
}
async function openDesktop() {
  if (!props.threadId || openingDesktop.value) return;
  const threadId = props.threadId;
  openingDesktop.value = true;
  openedDesktop.value = false;
  desktopError.value = '';
  try {
    const result = await request<{ ok: boolean }>(threadUrl(threadId, 'open'), { method: 'POST' });
    if (!result.ok) throw new Error('无法打开电脑上的对话，请稍后重试。');
    if (props.threadId === threadId) openedDesktop.value = true;
  } catch (failure) { if (props.threadId === threadId) desktopError.value = (failure as Error).message; }
  finally { openingDesktop.value = false; }
}
</script>

<template>
  <button type="button" role="menuitem" :disabled="!threadId" @click="show"><Connection /><span>审核钩子</span></button>
  <NativeSheet :open="open" :labelledby="titleId" panel-class="settings-modal" style="--sheet-width: 560px" @close="open = false" @after-close="emit('after-close')">
      <section class="hook-sheet">
        <header class="modal-heading"><h2 :id="titleId">审核钩子</h2><button type="button" class="icon-button" aria-label="关闭审核钩子" @click="open = false"><Close /></button></header>
        <p class="hook-description">这里展示本机配置，信任状态和审批请在电脑上确认。</p>
        <p v-if="loading" class="hook-description" role="status">正在读取 Hooks 配置…</p>
        <template v-else-if="data">
          <p v-if="!data.sources.length && !data.issues.length" class="hook-description hook-empty">没有找到用户、当前项目或已启用插件的 Hooks 配置。</p>
          <div v-if="data.sources.length" class="hook-sources">
            <details v-for="source in data.sources" :key="source.path" class="hook-source">
              <summary><span><strong>{{ source.label }}</strong><small v-if="events(source)">{{ events(source) }}</small></span><ArrowRight /></summary>
              <p class="hook-path">{{ source.path }}</p>
              <pre>{{ JSON.stringify(source.config, null, 2) }}</pre>
            </details>
          </div>
          <p v-for="issue in data.issues" :key="issue.path" class="hook-error"><span>{{ issue.message }}</span><small>{{ issue.path }}</small></p>
        </template>
        <div v-if="error" class="hook-error" role="alert"><p>{{ error }}</p><button type="button" class="hook-retry" :disabled="loading" @click="load">重新读取</button></div>
        <p v-if="desktopError" class="hook-error" role="alert">{{ desktopError }}</p>
        <p v-if="openedDesktop" class="hook-description" role="status">已打开电脑上的对话，请在输入框的 Review hooks 中审阅。</p>
        <footer><button type="button" class="primary-button hook-desktop" :disabled="openingDesktop" @click="openDesktop"><Loading v-if="openingDesktop" class="is-spin" /><Monitor v-else />{{ openingDesktop ? '正在打开…' : desktopError ? '重试打开' : '在电脑审阅' }}</button></footer>
      </section>
  </NativeSheet>
</template>

<style scoped>
.hook-sheet { min-width: 0; }
.hook-description { margin: 4px 0 16px; color: var(--muted); font-size: 13px; line-height: 1.6; overflow-wrap: anywhere; }
.hook-empty { padding: 14px; border-radius: 12px; background: var(--panel); }
.hook-sources { border: 1px solid var(--border); border-radius: 12px; background: var(--page); }
.hook-source { min-width: 0; padding-inline: 12px; }
.hook-source + .hook-source { border-top: 1px solid var(--border); }
.hook-source summary { display: flex; align-items: center; gap: 12px; min-height: 52px; padding: 12px 0; list-style: none; cursor: pointer; }
.hook-source summary::-webkit-details-marker { display: none; }
.hook-source summary > span { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.hook-source strong { display: block; font-size: 14px; line-height: 1.5; font-weight: 500; }
.hook-source small, .hook-path { display: block; margin: 3px 0 0; color: var(--muted); font-size: 12px; line-height: 1.6; overflow-wrap: anywhere; }
.hook-source summary > svg { width: 14px; height: 14px; flex: none; color: var(--muted); }
.hook-source[open] summary > svg { transform: rotate(90deg); color: var(--signal); }
.hook-path { margin: 0 0 8px; }
.hook-source pre { max-height: 42dvh; margin: 0 0 12px; padding: 12px; border-radius: 10px; background: var(--terminal); color: var(--terminal-text); font-size: 12px; line-height: 1.65; white-space: pre-wrap; overflow-wrap: anywhere; word-break: break-word; overflow-y: auto; }
.hook-error { margin: 12px 0; color: var(--error); font-size: 13px; line-height: 1.6; overflow-wrap: anywhere; }
.hook-error p { margin: 0; }
.hook-error small { display: block; font-size: 12px; }
.hook-retry { min-height: 44px; padding: 8px 0; border: 0; background: transparent; color: var(--text); text-decoration: underline; text-underline-offset: 3px; }
footer { display: flex; justify-content: flex-end; padding-top: 16px; }
.hook-desktop { margin-top: 0; }
.hook-desktop svg { width: 16px; height: 16px; }
button:disabled { opacity: .5; cursor: default; }
button:focus-visible, summary:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
</style>
