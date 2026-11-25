<script setup lang="ts">
import { nextTick, watch } from 'vue';
import DOMPurify from 'dompurify';
import { webCitationText } from '../itemPresentation';
import { previewFile } from './filePreview';
import { desktopApiPath, relayDesktop } from '../../../transport';
import { mediaUrl } from '../../../relayMedia';
import { conversationLink } from '../links';
import { markdownPreview, markdownPreviewFailed, preloadMarkdownPreview } from './markdownPreview';

defineOptions({ inheritAttrs: false });
const props = defineProps<{ modelValue: string; assistant?: boolean; documentPreview?: boolean }>();
const emit = defineEmits<{ rendered: [] }>();
if (!markdownPreview.value && !markdownPreviewFailed.value) void preloadMarkdownPreview().catch(() => {});
function reopen() { window.dispatchEvent(new Event('codex-reload-requested')); }
watch(markdownPreview, value => { if (value) void nextTick(() => emit('rendered')); }, { immediate: true, flush: 'post' });

function sanitize(html: string) {
  const content = DOMPurify.sanitize(html, { RETURN_DOM: true, ...(props.documentPreview && {
    FORBID_TAGS: ['style', 'svg', 'math', 'video', 'audio', 'source', 'img'], FORBID_ATTR: ['style', 'href', 'src', 'srcset'],
  }) }) as HTMLElement;
  if (props.assistant && html.includes('\uE200')) {
    const walker = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!node.parentElement?.closest('code, pre')) node.textContent = webCitationText(node.textContent || '');
    }
  }
  if (html.includes('<a ')) {
    for (const link of content.querySelectorAll<HTMLAnchorElement>('a[href]')) {
      const href = link.getAttribute('href')!;
      if (previewFile(href)?.kind === 'disabled') {
        link.replaceWith(...link.childNodes);
        continue;
      }
      const desktop = relayDesktop();
      if (desktop && href.startsWith('/?thread=')) {
        link.setAttribute('href', conversationLink(new URL(href, location.href).searchParams.get('thread') || ''));
        continue;
      }
      if (!props.assistant || !href.startsWith('/api/threads/') || !href.endsWith('&preview=file')) {
        if (desktop && desktopApiPath(href)) link.replaceWith(...link.childNodes);
        continue;
      }
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'file-preview-link';
      button.dataset.filePreview = href;
      button.setAttribute('aria-haspopup', 'dialog');
      button.append(...link.childNodes);
      link.replaceWith(button);
    }
  }
  for (const image of content.querySelectorAll('img')) {
    if (relayDesktop()) {
      image.removeAttribute('srcset');
      const src = image.getAttribute('src');
      if (src) image.setAttribute('src', mediaUrl(src));
    }
    image.tabIndex = 0;
    image.setAttribute('role', 'button');
    image.setAttribute('aria-haspopup', 'dialog');
    image.setAttribute('aria-label', image.alt ? '查看大图：' + image.alt : '查看大图');
  }
  return content.innerHTML;
}
</script>

<template>
  <component :is="markdownPreview" v-if="markdownPreview" v-bind="$attrs" :model-value="modelValue" :sanitize="sanitize" />
  <template v-else>
    <div class="markdown-fallback">{{ assistant ? webCitationText(modelValue) : modelValue }}</div>
    <p v-if="markdownPreviewFailed" class="markdown-retry" role="alert"><span>消息排版未能加载</span><button type="button" @click="reopen">重新打开页面</button></p>
  </template>
</template>

<style scoped>
.markdown-fallback { white-space: pre-wrap; overflow-wrap: anywhere; }
.markdown-retry { display: flex; align-items: center; gap: 12px; margin: 4px 0 0; color: var(--muted); font-size: 12px; line-height: 1.5; }
.markdown-retry button { flex: none; min-height: 44px; padding: 8px 0; border: 0; background: none; color: inherit; font: inherit; text-decoration: underline; text-underline-offset: 3px; }
</style>
