<script setup lang="ts">
import { defineAsyncComponent, nextTick, onBeforeUnmount, ref, shallowRef, useId } from 'vue';
import { Close, Document, Loading } from '@element-plus/icons-vue';
import { request } from '../../../api';
import { mediaUrl } from '../../../relayMedia';
import type { PreviewKind } from './filePreview';

const props = defineProps<{ theme: 'light' | 'dark' }>();
const MarkdownPreview = defineAsyncComponent(() => import('./MarkdownPreview.vue'));
const headingId = useId();
const dialog = ref<HTMLDialogElement>();
type DisplayKind = Exclude<PreviewKind, 'disabled'>;
const selected = ref<{ url: string; title: string; name: string; kind: DisplayKind }>();
type Preview = { kind: 'html' | 'markdown' | 'text'; text: string } | { kind: 'audio' | 'video' | 'image'; url: string };
const content = shallowRef<Preview>();
const error = ref('');
let controller: AbortController | undefined;
let opener: HTMLButtonElement | undefined;

function loadHtml(event: Event) {
  const frame = event.target as HTMLIFrameElement;
  if (content.value?.kind === 'html') {
    frame.contentWindow?.postMessage({ type: 'codex-html-preview', html: content.value.text }, '*');
  }
}

async function load() {
  const file = selected.value;
  if (!file || controller) return;
  const pending = new AbortController();
  controller = pending;
  error.value = '';
  try {
    if (file.kind === 'audio' || file.kind === 'video' || file.kind === 'image') {
      content.value = { kind: file.kind, url: mediaUrl(file.url) };
      return;
    }
    const text = await request(file.url, { signal: pending.signal }, response => response.text());
    if (!pending.signal.aborted) content.value = { kind: file.kind, text };
  } catch (failure) {
    if (!pending.signal.aborted) error.value = failure instanceof SyntaxError ? '无法读取预览，请稍后重试。' : (failure as Error).message;
  } finally {
    if (controller === pending) controller = undefined;
  }
}

async function open(url: string, file: { name: string; kind: DisplayKind }, target: HTMLButtonElement) {
  if (selected.value) return;
  opener = target;
  target.focus({ preventScroll: true });
  selected.value = { ...file, url, title: target.textContent?.trim() || file.name };
  await nextTick();
  if (!dialog.value) return;
  dialog.value.showModal();
  void load();
}

function close() {
  const target = opener;
  opener = undefined;
  controller?.abort();
  controller = undefined;
  selected.value = undefined;
  content.value = undefined;
  error.value = '';
  if (target) requestAnimationFrame(() => {
    // WebKit 有时要等到下一帧才从 iframe 释放焦点。
    if (document.activeElement === document.body) target.focus({ preventScroll: true });
  });
}

onBeforeUnmount(() => { opener = undefined; close(); dialog.value?.close(); });
defineExpose({ open });
</script>

<template>
  <Teleport to="body">
    <dialog v-if="selected" ref="dialog" class="file-preview" :aria-labelledby="headingId" @close="close">
      <header class="image-preview-toolbar file-preview-toolbar">
        <span class="file-preview-icon"><Document aria-hidden="true" /></span>
        <div class="file-preview-heading">
          <h2 :id="headingId" :title="selected.title">{{ selected.title }}</h2>
          <p v-if="selected.name !== selected.title" :title="selected.name">{{ selected.name }}</p>
        </div>
        <button type="button" class="icon-button image-preview-close" aria-label="关闭预览" title="关闭预览" autofocus @click="dialog?.close()"><Close /></button>
      </header>
      <iframe v-if="!error && content?.kind === 'html'" class="file-preview-body" title="HTML 预览" sandbox="allow-scripts" referrerpolicy="no-referrer" src="/preview-sandbox.html" @load="loadHtml" />
      <div v-else-if="!error && content?.kind === 'markdown'" class="file-preview-body file-preview-text"><MarkdownPreview :model-value="content.text" :theme="theme" document-preview preview-theme="github" no-img-zoom-in /></div>
      <pre v-else-if="!error && content?.kind === 'text'" class="file-preview-body file-preview-text">{{ content.text }}</pre>
      <div v-else-if="!error && content && 'url' in content" class="file-preview-body file-preview-media">
        <video v-if="content.kind === 'video'" :src="content.url" controls playsinline preload="metadata" @error="error = '无法播放此视频。'" />
        <audio v-else-if="content.kind === 'audio'" :src="content.url" controls preload="metadata" @error="error = '无法播放此音频。'" />
        <img v-else :src="content.url" :alt="selected.name" @error="error = '无法显示此图片。'" />
      </div>
      <div v-else class="file-preview-state">
        <p v-if="error" class="error-text" role="alert">{{ error }}</p>
        <button v-if="error" type="button" class="secondary-button" @click="load">重试</button>
        <p v-else class="muted" role="status"><Loading class="is-spin" />正在加载预览…</p>
      </div>
    </dialog>
  </Teleport>
</template>

<style scoped>
.file-preview-toolbar { gap: 10px; padding: max(8px, var(--safe-top-visible)) clamp(12px, 4vw, 24px) 8px; background: var(--sidebar); }
.file-preview-icon { display: grid; flex: none; place-items: center; width: 34px; height: 34px; border-radius: 10px; background: var(--signal-soft); color: var(--signal); }
.file-preview-heading { flex: 1; min-width: 0; }
.file-preview-heading h2 { margin: 0; font-size: 17px; font-weight: 650; line-height: 1.35; }
.file-preview-heading p { margin: 3px 0 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; line-height: 1.35; color: var(--muted); }
.image-preview-close { background: var(--panel); }
.image-preview-close:hover { background: var(--hover); }
.file-preview-text { width: min(100%, 820px); margin-inline: auto; padding: clamp(14px, 5vw, 26px); }
.file-preview-media { background: var(--sidebar); }
.file-preview-state { gap: 14px; }
.file-preview-state p { max-width: 32ch; overflow-wrap: anywhere; }
@media (prefers-reduced-motion: no-preference) {
  .file-preview[open] .file-preview-toolbar { animation: file-preview-ready 200ms ease-out; }
  @keyframes file-preview-ready { from { opacity: .35; } }
}
</style>
