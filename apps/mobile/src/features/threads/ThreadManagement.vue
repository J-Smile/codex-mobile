<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue';
import { Close, Delete, FolderOpened, Loading, Top } from '@element-plus/icons-vue';
import { request, threadUrl } from '../../api';
import type { DesktopThread } from '../../types';
import NativeSheet from '../../components/NativeSheet.vue';

const props = defineProps<{ open: boolean; thread: DesktopThread | null; capabilities: Record<string, boolean>; connected: boolean }>();
const emit = defineEmits<{ close: []; 'after-close': []; changed: [value: { threadId: string; title?: string; pinned?: boolean; archived?: boolean; deleted?: boolean }] }>();
const titleId = useId();
const title = ref('');
const busy = ref('');
const error = ref('');
const confirmingDelete = ref(false);
let generation = 0;
const canRename = computed(() => props.connected && props.capabilities.threadRename && !busy.value
  && !!title.value.trim() && title.value.trim() !== props.thread?.title && [...title.value.trim()].length <= 200);
watch([() => props.open, () => props.thread?.id], () => {
  generation++;
  title.value = props.thread?.title || '';
  error.value = '';
  confirmingDelete.value = false;
});
onBeforeUnmount(() => { generation++; });

async function apply(action: 'rename' | 'pin' | 'archive' | 'delete') {
  const thread = props.thread;
  const capability = { rename: 'threadRename', pin: 'threadPin', archive: 'threadArchive', delete: 'threadDelete' }[action];
  if (!props.open || !thread || busy.value || !props.connected || !props.capabilities[capability]
    || (action === 'rename' && !canRename.value) || (action === 'delete' && !confirmingDelete.value)) return;
  const input = action === 'rename' ? { title: title.value.trim() } : action === 'pin' ? { pinned: !thread.pinnedIndex }
    : action === 'archive' ? { archived: !thread.archived } : { confirm: 'delete-thread' };
  busy.value = action;
  const current = generation;
  error.value = '';
  try {
    const result = await request<{ threadId: string; title?: string; pinned?: boolean; archived?: boolean; deleted?: boolean }>(threadUrl(thread.id, 'manage') + '?action=' + action, { method: 'POST', body: JSON.stringify(input), timeoutMs: 65_000 });
    if (current === generation) emit('close');
    emit('changed', result);
  } catch (failure) {
    if (current === generation) error.value = (failure as Error).message;
  } finally { busy.value = ''; }
}
</script>

<template>
  <NativeSheet :open="open" :labelledby="titleId" :dismissible="!busy" panel-class="settings-modal thread-management" @close="emit('close')" @after-close="emit('after-close')">
    <section>
      <header class="modal-heading"><h2 :id="titleId">管理聊天</h2><button type="button" class="icon-button" aria-label="关闭聊天管理" :disabled="!!busy" @click="emit('close')"><Close /></button></header>
      <template v-if="thread">
        <form v-if="capabilities.threadRename" class="thread-rename" @submit.prevent="apply('rename')">
          <label :for="titleId + '-title'">聊天名称</label>
          <input :id="titleId + '-title'" v-model="title" maxlength="200" :disabled="!!busy" autocomplete="off" />
          <button type="submit" class="text-button" :disabled="!canRename"><Loading v-if="busy === 'rename'" class="is-spin" />保存名称</button>
        </form>
        <div class="thread-actions">
          <button v-if="capabilities.threadPin" type="button" class="settings-row settings-action" :disabled="!connected || !!busy" @click="apply('pin')"><span><Top />{{ thread.pinnedIndex ? '取消置顶' : '置顶聊天' }}</span><Loading v-if="busy === 'pin'" class="is-spin" /></button>
          <button v-if="capabilities.threadArchive" type="button" class="settings-row settings-action" :disabled="!connected || !!busy" @click="apply('archive')"><span><FolderOpened />{{ thread.archived ? '恢复聊天' : '归档聊天' }}</span><Loading v-if="busy === 'archive'" class="is-spin" /></button>
          <button v-if="capabilities.threadDelete && !confirmingDelete" type="button" class="settings-row settings-action destructive" :disabled="!connected || !!busy" @click="confirmingDelete = true"><span><Delete />删除聊天</span></button>
        </div>
        <div v-if="confirmingDelete" class="thread-delete-confirm" role="group" aria-label="确认删除聊天">
          <p>永久删除“{{ thread.title }}”？其子对话和附属聊天也会删除，无法恢复。</p>
          <div><button type="button" class="text-button" :disabled="!!busy" @click="confirmingDelete = false">取消</button><button type="button" class="text-button destructive" :disabled="!connected || !!busy" @click="apply('delete')"><Loading v-if="busy === 'delete'" class="is-spin" />确认删除</button></div>
          <p v-if="busy === 'delete'" class="muted small" role="status">等待官方删除结果；如果官方要求确认，请完成确认后等待同步。</p>
        </div>
        <p v-if="!connected" class="muted small" role="status">连接恢复后可管理聊天，已填写的名称会保留。</p>
        <p v-if="error" class="error-text" role="alert">{{ error }}</p>
      </template>
    </section>
  </NativeSheet>
</template>

<style scoped>
.thread-rename { display: flex; flex-wrap: wrap; gap: 8px; padding-block: 12px; }
.thread-rename label { width: 100%; margin: 0; font-size: 14px; }
.thread-rename input { flex: 1; min-width: 0; min-height: 44px; font-size: 16px; }
.thread-rename button { flex: none; min-height: 44px; }
.thread-actions { border-block: 1px solid var(--border); }
.thread-actions button { width: 100%; }
.thread-actions button > span { display: flex; align-items: center; gap: 12px; }
.thread-actions svg { width: 18px; height: 18px; }
.destructive { color: var(--error); }
.thread-delete-confirm p { font-size: 14px; line-height: 1.6; overflow-wrap: anywhere; }
.thread-delete-confirm > div { display: flex; justify-content: flex-end; gap: 16px; }
.thread-delete-confirm button { min-height: 44px; }
</style>
