<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue';
import { ArrowDown, ArrowUp, Delete, EditPen, VideoPlay, List, Loading } from '@element-plus/icons-vue';
import type { QueueActions, QueueSnapshot } from './useMessageQueue';
import { splitDraftContext } from './composerContext';
import CollapseMotion from '../../components/CollapseMotion.vue';
import NativeSheet from '../../components/NativeSheet.vue';

const props = defineProps<{ queue: QueueSnapshot | null; loading: boolean; busy: boolean; error: string; actions: QueueActions; disabled?: boolean; compact?: boolean; capabilities?: Record<string, boolean>; active?: boolean }>();
const emit = defineEmits<{ reload: []; expand: [] }>();
const id = useId();
const expanded = ref(false);
const editing = ref<{ id: string; prompt: string; revision: string }>();
function edit(message: { id: string; prompt: string }) {
  if (props.queue) editing.value = { id: message.id, prompt: message.prompt, revision: props.queue.revision };
}
async function save() {
  const draft = editing.value;
  if (draft && await props.actions.edit?.(draft.id, draft.prompt, draft.revision)) editing.value = undefined;
}
function move(index: number) {
  if (!props.queue || index <= 0) return;
  const ids = props.queue.messages.map(message => message.id);
  [ids[index - 1], ids[index]] = [ids[index]!, ids[index - 1]!];
  void props.actions.reorder?.(ids, props.queue.revision);
}
const rows = computed(() => (props.queue?.messages || []).map(message => ({ ...message, preview: splitDraftContext(message.prompt) })));
const revealed = computed(() => expanded.value && !props.compact);
watch(() => rows.value.length, count => { if (!count) expanded.value = false; });
function toggle() {
  if (props.compact) { emit('expand'); expanded.value = true; }
  else expanded.value = !expanded.value;
}
</script>

<template>
  <section v-if="rows.length || loading || error" class="message-queue" aria-label="待执行消息">
    <button type="button" class="queue-toggle" :aria-expanded="revealed" :aria-controls="id" @click="toggle">
      <span><List aria-hidden="true" />待执行 <span class="queue-count">{{ rows.length }}</span></span>
      <ArrowDown :class="{ expanded: revealed }" aria-hidden="true" />
    </button>
    <p v-if="loading" class="muted small" role="status"><Loading class="is-spin" />正在读取队列…</p>
    <p v-if="error" class="error-text small" role="alert">{{ error }} <button type="button" class="text-button" :disabled="busy || loading" @click="emit('reload')">重新读取</button></p>
    <CollapseMotion :open="revealed">
      <ol :id="id" class="queue-list">
        <li v-for="(message, index) in rows" :key="message.id" class="queue-row">
          <div class="queued-content">
            <span v-for="context in message.preview.contexts" :key="context.mention" class="muted small">{{ context.label }} </span>
            <p>{{ message.preview.text || '附件消息' }}</p>
            <span v-if="message.attachmentCount" class="muted small">{{ message.attachmentCount }} 个附件</span>
          </div>
          <div class="queue-actions">
            <button v-if="capabilities?.queueStart" type="button" class="icon-button" :disabled="disabled || busy || loading" :title="active ? '立即追加到当前任务' : '立即执行'" :aria-label="'立即发送第 ' + (index + 1) + ' 条消息'" @click="actions.send?.(message.id)"><VideoPlay /></button>
            <button v-if="capabilities?.queueEdit" type="button" class="icon-button" :disabled="disabled || busy || loading" :aria-label="'编辑第 ' + (index + 1) + ' 条消息'" @click="edit(message)"><EditPen /></button>
            <button v-if="capabilities?.queueReorder && index > 0" type="button" class="icon-button" :disabled="disabled || busy || loading" :aria-label="'上移第 ' + (index + 1) + ' 条消息'" @click="move(index)"><ArrowUp /></button>
            <button type="button" class="icon-button" :disabled="disabled || busy || loading" :aria-label="'删除第 ' + (index + 1) + ' 条消息'" @click="actions.remove(message.id)"><Delete /></button>
          </div>
        </li>
      </ol>
      <p class="muted small">队列由电脑继续执行，手机断开不影响已提交的任务。</p>
    </CollapseMotion>
  </section>
  <NativeSheet :open="!!editing" labelledby="queue-edit-title" @close="editing = undefined">
    <form v-if="editing" @submit.prevent="save">
      <h2 id="queue-edit-title">编辑待执行消息</h2>
      <textarea v-model="editing.prompt" aria-label="待执行内容" class="queue-editor" :disabled="disabled || busy" />
      <p class="muted small">已附图片保留。队列变化后请重新打开编辑。</p>
      <p v-if="error" role="alert" class="error-text">{{ error }}</p>
      <button type="button" class="text-button" @click="editing = undefined">取消</button>
      <button type="submit" class="text-button" :disabled="disabled || busy">保存</button>
    </form>
  </NativeSheet>
</template>

<style scoped>
.message-queue { flex-shrink: 1; width: 100%; max-width: 780px; min-width: 0; margin: 0 auto 4px; padding: 0 8px; border-radius: 12px; background: var(--sidebar); color: var(--text); overflow: auto; max-height: 36dvh; }
.queue-toggle { display: flex; align-items: center; justify-content: space-between; width: 100%; min-height: 44px; padding: 8px 4px; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 13px; font-weight: 600; text-align: left; }
.queue-toggle span { display: inline-flex; align-items: center; gap: 7px; }
.queue-toggle svg { width: 16px; height: 16px; }
.queue-toggle > svg { transition: transform .2s; }
.queue-toggle > svg.expanded { transform: rotate(180deg); }
.queue-count { color: var(--signal); font-variant-numeric: tabular-nums; }
.queue-list { list-style: none; margin: 0; padding: 0; }
.queue-row { display: flex; align-items: flex-start; gap: 8px; border-top: 1px solid var(--border); }
.queue-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; max-width: 96px; }
.queue-editor { width: 100%; min-height: 120px; padding: 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--sidebar); color: var(--text); font: inherit; resize: vertical; }
.queued-content { flex: 1; min-width: 0; padding: 10px 4px; overflow-wrap: anywhere; }
.queued-content p { margin: 0; white-space: pre-wrap; font-size: 14px; line-height: 1.5; }
.message-queue > p, .message-queue .muted { overflow-wrap: anywhere; }
@media (prefers-reduced-motion: reduce) { .queue-toggle > svg { transition: none; } }
</style>
