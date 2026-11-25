<script setup lang="ts">
import { computed, onUnmounted, ref, useId, watch } from 'vue';
import MdPreview from './preview/MarkdownPreview.vue';
import type { ThreadItem } from '../../types';
import { copyText } from './clipboard';
import { requestItem, threadUrl } from '../../api';
import { mediaUrl } from '../../relayMedia';
import { previewFile } from './preview/filePreview';
import { browserActivity, chatCreationRequestText, fileChangeLabel, itemFailed, itemLabel, itemRunning } from './itemPresentation';
import { isSubagentItem } from './subagentPresentation';
import SubagentActivity from './SubagentActivity.vue';
import CollapseMotion from '../../components/CollapseMotion.vue';
import { useConversationDisclosure, useConversationItemDetail } from './useConversationHistory';
import { useStreamText } from './useStreamText';
import { ArrowDown, Connection, CopyDocument, EditPen, Loading, Monitor, Tools, Warning } from '@element-plus/icons-vue';

const props = withDefaults(defineProps<{ item: ThreadItem; changePath?: string; threadId: string; theme: 'dark' | 'light'; streaming?: boolean; live?: boolean; arrive?: boolean }>(), { live: true });
const emit = defineEmits<{ rendered: [] }>();
const displayText = useStreamText(() => props.item.text || '', () => !!props.streaming, () => emit('rendered'));
const replyArrival = ref(!!props.arrive && !matchMedia('(prefers-reduced-motion: reduce)').matches);
const copied = ref(false);
const copyError = ref('');
const open = useConversationDisclosure(() => JSON.stringify([props.threadId, 'item', props.item.id, props.changePath]));
const contentId = useId();
const summary = ref<HTMLElement>();
const detailsOpen = ref(!!open.value);
watch(open, (value) => { if (value) detailsOpen.value = true; });
function syncDetails(event: Event) {
  const visible = (event.target as HTMLDetailsElement).open;
  if (visible === detailsOpen.value) return;
  detailsOpen.value = visible;
  open.value = visible;
}
const savedDetail = useConversationItemDetail(() => JSON.stringify([props.threadId, props.item.id]));
const detail = savedDetail.detail;
const detailError = ref('');
const loading = ref(false);
const retry = ref(0);
const item = computed(() => detail.value ? { ...props.item, ...detail.value } : props.item);
const creationRequest = computed(() => chatCreationRequestText(item.value));
const change = computed(() => props.item.changes?.find((entry: { path: string }) => entry.path === props.changePath));
const changes = computed(() => (item.value.changes || [])
  .filter((entry: { path: string }) => !props.changePath || entry.path === props.changePath)
  .map((entry: { path: string; kind?: string | { type?: string }; diff?: string | { text?: string } }) => ({ ...entry,
    file: (typeof entry.kind === 'string' ? entry.kind : entry.kind?.type) === 'delete' ? undefined : previewFile(entry.path) })));
const label = computed(() => props.changePath ? props.changePath.split(/[\\/]/).at(-1) || props.changePath : itemLabel(props.item));
let detailTimer: ReturnType<typeof setTimeout> | undefined;
let lastRequestAt = 0;
let disposed = false;

// 流式更新合并到同一个读取流程，执行结束时立即补取最终输出。
function scheduleDetails() {
  clearTimeout(detailTimer);
  if (disposed || !open.value || !props.item.detailsAvailable || loading.value || savedDetail.version.value === props.item.detailVersion) return;
  const delay = itemRunning(props.item) ? Math.max(0, 250 - (Date.now() - lastRequestAt)) : 0;
  detailTimer = setTimeout(loadDetails, delay);
}

async function loadDetails() {
  const threadId = props.threadId;
  const itemId = props.item.id;
  const version = props.item.detailVersion;
  lastRequestAt = Date.now();
  loading.value = true;
  detailError.value = '';
  try {
    const result = await requestItem(threadId, itemId, version);
    if (!disposed && threadId === props.threadId && itemId === props.item.id) {
      savedDetail.save(result, version);
    }
  } catch (failure) {
    if (!disposed && threadId === props.threadId && itemId === props.item.id) detailError.value = (failure as Error).message;
  } finally {
    loading.value = false;
    if (!detailError.value) scheduleDetails();
  }
}

watch(() => [open.value, props.threadId, props.item.id, props.item.detailVersion, retry.value], scheduleDetails, { immediate: true });
onUnmounted(() => {
  disposed = true;
  clearTimeout(detailTimer);
});

function assetUrl(file: string) {
  return threadUrl(props.threadId, 'attachment') + '?itemId=' + encodeURIComponent(props.item.id) + '&path=' + encodeURIComponent(file).replaceAll('(', '%28').replaceAll(')', '%29');
}

function markdown(text: string) {
  const assistant = props.item.type === 'agentMessage';
  const links = text.replace(/(!?\[[^\]]*\])\((?:<([^>]+)>|([^)\s]+))(?:\s+"[^"]*")?\)/g, (full, label, angle, plain) => {
    const file = angle || plain;
    if (file.startsWith('plugin://')) return label.slice(1, -1).replace(/^@/, '');
    const thread = /^codex:\/\/threads\/([a-zA-Z0-9-]+)$/.exec(file);
    if (thread) return label + '(/?thread=' + thread[1] + ')';
    const preview = previewFile(file);
    if (preview?.kind === 'disabled') return label.replace(/^!?\[|\]$/g, '') || preview.name;
    if (assistant && preview) {
      if (label.startsWith('!') && preview.kind === 'image' && /^(?:[A-Za-z]:[\\/]|\/(?![A-Za-z]:\/))/.test(file)) return label + '(' + mediaUrl(assetUrl(file)) + ')';
      const prefix = label.startsWith('!') && preview.kind !== 'image' ? label.slice(1) : label;
      return prefix + '(' + (prefix.startsWith('!') ? mediaUrl(assetUrl(file) + '&preview=file') : assetUrl(file) + '&preview=file') + ')';
    }
    return /^([A-Za-z]:[\\/]|\/)/.test(file) ? label + '(' + (label.startsWith('!') ? mediaUrl(assetUrl(file)) : assetUrl(file)) + ')' : full;
  });
  if (!assistant || !links.includes('\uE200visualize\uE202')) return links;
  return links.replace(/\uE200visualize\uE202([^\uE201]+)\uE201/g, (marker, json) => {
    try {
      const file = JSON.parse(json)?.path;
      if (typeof file === 'string' && previewFile(file)?.kind === 'html') return '[预览可视化](' + assetUrl(file) + '&preview=file)';
    } catch { /* 保留尚不能识别的原始标记。 */ }
    return marker;
  });
}

const userText = computed(() => (item.value.content || [])
  .filter((part: any) => part.type === 'text' || part.type === 'inputText')
  .map((part: any) => {
    const text = part.text || '';
    const reply = /^\s*<send_user_message_question_reply>\s*([\s\S]*?)\s*<\/send_user_message_question_reply>\s*$/.exec(text);
    if (reply) {
      try {
        const answers = JSON.parse(reply[1]!);
        if (Array.isArray(answers) && answers.length && answers.every(answer => typeof answer?.answer === 'string')) {
          return answers.map(answer => answers.length > 1 && typeof answer.question === 'string'
            ? answer.question + '\n' + answer.answer : answer.answer).join('\n\n');
        }
      } catch { /* Unrecognized reply content remains visible as its original text. */ }
    }
    const marker = '## My request:';
    const at = text.indexOf(marker);
    if ((item.value.content || []).some((entry: any) => entry.type === 'localImage')
      && /^\s*# Files mentioned by the user:/.test(text) && at >= 0) return text.slice(at + marker.length).trim();
    return text;
  }).join('\n'));
const userImages = computed(() => (item.value.content || []).flatMap((part: any) => {
  if (part.type === 'localImage' && part.path) return [assetUrl(part.path)];
  if (part.type === 'image' && /^(data:image\/|https?:\/\/)/.test(part.url || part.imageUrl || '')) return [part.url || part.imageUrl];
  return [];
}));
const resultImages = computed(() => {
  const content = item.value.result?.content || item.value.contentItems || [];
  const images = content.flatMap((part: any) => {
    if (part.type === 'image' && /^image\/(png|jpeg|webp|gif|avif)$/.test(part.mimeType || '')) return ['data:' + part.mimeType + ';base64,' + part.data];
    if (part.type === 'inputImage' && /^data:image\//.test(part.imageUrl || '')) return [part.imageUrl];
    return [];
  });
  const screenshot = item.value.result?._meta?.['codex/toolSurface']?.screenshot?.url;
  if (typeof screenshot === 'string' && /^data:image\//.test(screenshot)) images.push(screenshot);
  return [...new Set<string>(images)];
});
const awaitingSync = computed(() => !props.live && itemRunning(props.item));
const toolStatus = computed(() => itemFailed(props.item) ? '失败' : awaitingSync.value ? '等待同步' : itemRunning(props.item) ? '进行中' : props.item.status === 'completed' ? '完成' : '');
const activityPrefix = computed(() => {
  if (props.item.type === 'error' || isSubagentItem(props.item)) return '';
  if (itemFailed(props.item)) return '未成功';
  if (awaitingSync.value) return '等待同步';
  if (props.item.type === 'commandExecution') return itemRunning(props.item) ? '正在运行' : '已运行';
  if (props.item.type === 'fileChange') return itemRunning(props.item) ? '正在编辑' : change.value ? fileChangeLabel(change.value) : '已编辑';
  if (browserActivity(props.item) && props.item.arguments?.title) return '';
  return itemRunning(props.item) ? '正在使用' : props.item.status === 'completed' ? '已使用' : '';
});
const duration = computed(() => typeof props.item.durationMs === 'number' ? (props.item.durationMs < 1000 ? '<1s' : Math.round(props.item.durationMs / 1000) + 's') : '');
const commandOutput = computed(() => item.value.aggregatedOutput || item.value.output?.text || (typeof item.value.output === 'string' ? item.value.output : ''));

function pretty(value: unknown) {
  if (typeof value === 'string') return value;
  return JSON.stringify(value, function (key, entry) {
    if ((key === 'data' && this.type === 'image') || (typeof entry === 'string' && entry.startsWith('data:image/'))) return '图片见下方';
    return entry;
  }, 2);
}

function toolOutput() {
  const result = item.value.result;
  if (result && typeof result === 'object') {
    const { _meta, ...visible } = result;
    return pretty(visible);
  }
  return pretty(result ?? item.value.output ?? item.value.contentItems ?? item.value.error ?? item.value);
}

async function copy(value: string) {
  copied.value = false;
  copyError.value = '';
  try {
    await copyText(value);
    copied.value = true;
    setTimeout(() => { copied.value = false; }, 1500);
  } catch { copyError.value = '复制失败，请长按输出文字复制。'; }
}
</script>

<template>
  <article v-if="item.type === 'userMessage'" class="user-row">
    <div class="user-bubble">
      <MdPreview v-if="userText" :id="'user-' + item.id" :model-value="markdown(userText)" :theme="theme" preview-theme="github" no-img-zoom-in @rendered="emit('rendered')" />
      <img v-for="(image, index) in userImages" :key="index" :src="mediaUrl(image)" class="result-image" alt="消息图片" loading="lazy" role="button" tabindex="0" aria-label="查看消息图片" aria-haspopup="dialog" />
      <Transition name="pending-label" mode="out-in">
        <span v-if="item.pending" :key="item.pendingLabel || '正在发送…'" class="message-pending">{{ item.pendingLabel || '正在发送…' }}</span>
      </Transition>
    </div>
  </article>
  <article v-else-if="creationRequest !== undefined" class="user-row">
    <div class="user-bubble chat-creation-request" role="group" :aria-label="item.name === 'create_thread' ? '创建聊天请求' : '用户追加消息'">
      <div class="creation-request-label">{{ item.name === 'create_thread' ? '创建聊天请求' : '用户追加消息' }}</div>
      <MdPreview :id="'creation-' + item.id" :model-value="markdown(creationRequest)" :theme="theme" preview-theme="github" no-img-zoom-in @rendered="emit('rendered')" />
    </div>
  </article>
  <article v-else-if="item.type === 'agentMessage'" :class="['agent-row', { 'agent-progress': item.phase === 'commentary', 'reply-arrival': replyArrival && !!displayText }]" @animationend.self="replyArrival = false" @animationcancel.self="replyArrival = false">
    <MdPreview :id="'md-' + item.id" :model-value="markdown(displayText)" :theme="theme" assistant preview-theme="github" no-img-zoom-in @rendered="emit('rendered')" />
  </article>
  <article v-else-if="item.type === 'imageView'" class="agent-row">
    <img :src="mediaUrl(assetUrl(item.path))" class="result-image" alt="Codex 输出图片" loading="lazy" role="button" tabindex="0" aria-label="查看 Codex 输出图片" aria-haspopup="dialog" />
  </article>
  <div v-else-if="change && previewFile(change.path)?.kind === 'disabled'" class="file-path inactive-file" :title="change.path">{{ fileChangeLabel(change) }} {{ label }}</div>
  <details v-else-if="item.type !== 'reasoning' && item.type !== 'steered'" class="tool-card" :open="detailsOpen" :data-expanded="!!open" @toggle="syncDetails">
    <summary ref="summary" :aria-expanded="!!open" :aria-controls="contentId" @click.prevent="open = !open">
      <span class="tool-icon"><Connection v-if="awaitingSync" /><Loading v-else-if="itemRunning(props.item)" class="is-spin" /><Warning v-else-if="itemFailed(props.item)" /><Monitor v-else-if="item.type === 'commandExecution'" /><EditPen v-else-if="item.type === 'fileChange'" /><Connection v-else-if="browserActivity(props.item) || isSubagentItem(item)" /><Tools v-else /></span>
      <span :class="['tool-title', { failed: toolStatus === '失败' }]" :title="props.changePath || label"><span v-if="activityPrefix" class="activity-prefix">{{ activityPrefix }}</span><span :class="{ 'changed-file-name': change }">{{ label }}</span></span>
      <span v-if="change?.additions != null" class="file-counts"><span class="lines-added">+{{ change.additions }}</span><span class="lines-removed">−{{ change.deletions }}</span></span>
      <span v-if="duration" class="tool-duration">{{ duration }}</span><ArrowDown class="disclosure-icon" />
    </summary>
    <CollapseMotion :id="contentId" :open="!!open" :focus-target="summary" @after-close="detailsOpen = false">
    <div class="tool-body">
      <p v-if="duration" class="muted small">耗时 {{ duration }}</p>
      <p v-if="loading" class="muted small" role="status">正在读取输出…</p>
      <p v-if="detailError" class="error-text" role="alert">{{ detailError }} <button class="text-button" @click="retry++">重试</button></p>
      <template v-if="!props.item.detailsAvailable || detail">
        <template v-if="item.type === 'commandExecution'">
          <div class="tool-section-label">执行命令</div><pre>{{ item.command }}</pre>
          <div class="output-heading"><span class="tool-section-label">终端输出</span><button class="text-button copy-button" @click="copy(commandOutput)"><CopyDocument />{{ copied ? '已复制' : '复制输出' }}</button></div>
          <p v-if="copyError" class="error-text" role="alert">{{ copyError }}</p>
          <pre class="command-output">{{ commandOutput || (awaitingSync ? '等待连接同步…' : itemRunning(item) ? '等待输出…' : '无输出') }}</pre>
          <div class="tool-meta"><span v-if="item.cwd">{{ item.cwd }}</span><span v-if="item.exitCode != null">退出码 {{ item.exitCode }}</span></div>
        </template>
        <template v-else-if="item.type === 'fileChange'">
          <section v-for="(entry, index) in changes" :key="index" class="file-change">
            <button v-if="entry.file && entry.file.kind !== 'disabled'" type="button" class="file-path file-preview-link" :data-file-preview="assetUrl(entry.path) + '&preview=file'" aria-haspopup="dialog">{{ entry.path }}</button>
            <div v-else class="file-path">{{ entry.path }}</div><pre class="diff-output">{{ (typeof entry.diff === 'string' ? entry.diff : entry.diff?.text) || pretty(entry.kind) }}</pre>
          </section>
        </template>
        <SubagentActivity v-else-if="isSubagentItem(item)" :item="item" />
        <template v-else-if="item.type === 'mcpToolCall' || item.type === 'dynamicToolCall'">
          <div class="tool-section-label">输入</div><pre>{{ pretty(item.arguments || item.input || {}) }}</pre>
          <div class="tool-section-label">输出</div><pre>{{ toolOutput() }}</pre>
        </template>
        <pre v-else>{{ pretty(item) }}</pre>
        <img v-for="(image, index) in resultImages" :key="index" :src="image" class="result-image" alt="工具输出图片" loading="lazy" role="button" tabindex="0" aria-label="查看工具输出图片" aria-haspopup="dialog" />
      </template>
    </div>
    </CollapseMotion>
  </details>
</template>

<style scoped>
.creation-request-label { font-size: 11px; color: var(--muted); margin-bottom: 6px; }
.tool-card .disclosure-icon { transform: rotate(0); transition: transform .24s var(--motion-ease); }
.tool-card[data-expanded="true"] .disclosure-icon { transform: rotate(180deg); }
@media (prefers-reduced-motion: reduce) {
  .tool-card .disclosure-icon { transition: none; }
}
</style>
