<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ArrowDown, Close } from '@element-plus/icons-vue';
import type { DesktopThread, MessageModel } from '../../types';
import { request } from '../../api';
import NativeSheet from '../../components/NativeSheet.vue';

const props = defineProps<{ thread: DesktopThread | null; connected: boolean; newThread?: boolean; selection: MessageModel }>();
const emit = defineEmits<{ 'update:selection': [value: MessageModel] }>();
const open = ref(false);
const presented = ref(false);
const loading = ref(false);
const loaded = ref(false);
const error = ref('');
const model = ref('');
const effort = ref('');
const recentModels = ref<string[]>([]);
const efforts = ref<string[]>([]);
const customModel = ref(false);
const fieldsDisabled = computed(() => !loaded.value || loading.value || !props.connected);
const inheritLabel = computed(() => props.newThread ? '桌面默认' : '沿用当前对话');
const modelName = (value: string) => value.replace(/^gpt-/, '').replace(/(^|[- ])([a-z])/g, (_, prefix, char) => (prefix ? ' ' : '') + char.toUpperCase());
const effortName = (value: string) => value.charAt(0).toUpperCase() + value.slice(1);
const currentModel = computed(() => props.newThread ? '' : props.thread?.model || '');
const currentEffort = computed(() => props.newThread ? '' : props.thread?.reasoningEffort || '');
const modelLabel = computed(() => modelName(props.selection.model || currentModel.value || (props.newThread ? '桌面默认' : 'Codex')));
const effortLabel = computed(() => effortName(props.selection.thinking || currentEffort.value));
let optionsRequest: AbortController | undefined;
async function readOptions() {
  if (!open.value || !props.connected) return;
  optionsRequest?.abort();
  const controller = new AbortController();
  optionsRequest = controller;
  loading.value = true;
  loaded.value = false;
  error.value = '';
  try {
    const data = await request<{ recentModels: string[]; efforts: string[] }>('/api/model-options', { signal: controller.signal });
    if (controller.signal.aborted) return;
    recentModels.value = [...new Set([currentModel.value, ...data.recentModels].filter((value): value is string => !!value))];
    efforts.value = data.efforts;
    customModel.value = !!model.value && !recentModels.value.includes(model.value);
    loaded.value = true;
  } catch (failure) {
    if (!controller.signal.aborted) error.value = (failure as Error).message;
  } finally { if (!controller.signal.aborted) loading.value = false; }
}
watch(open, value => {
  if (value) {
    presented.value = true;
    model.value = props.selection.model || '';
    effort.value = props.selection.thinking || '';
    void readOptions();
  } else { optionsRequest?.abort(); loading.value = false; }
});
onBeforeUnmount(() => optionsRequest?.abort());
function chooseModel(event: Event) {
  const selected = (event.target as HTMLSelectElement).value;
  customModel.value = selected === '__custom__';
  model.value = customModel.value ? '' : selected;
}
function save() {
  if (fieldsDisabled.value) return;
  const value = model.value.trim();
  if ((customModel.value || value) && !/^[a-zA-Z0-9_.:/-]{1,150}$/.test(value)) {
    error.value = '请输入有效的模型 ID。';
    return;
  }
  if (effort.value && !efforts.value.includes(effort.value)) {
    error.value = '请选择当前桌面支持的思考强度。';
    return;
  }
  emit('update:selection', { ...(value ? { model: value } : {}), ...(effort.value ? { thinking: effort.value } : {}) });
  open.value = false;
}
</script>

<template>
  <button type="button" class="model-button" :disabled="!connected" aria-label="模型与思考强度" aria-haspopup="dialog" :aria-expanded="presented" :title="selection.model || currentModel || (newThread ? '桌面默认' : '模型与思考强度')" @click="open = true">
    <span class="model-name">{{ modelLabel }}</span><span class="model-effort">{{ effortLabel }}</span><ArrowDown />
  </button>
  <NativeSheet :open="open" labelledby="model-title" panel-class="settings-modal" @close="open = false" @after-close="presented = false">
    <div class="modal-heading"><h2 id="model-title">模型与思考强度</h2><button class="icon-button" aria-label="关闭模型设置" @click="open = false"><Close /></button></div>
    <p class="model-settings-hint muted small">选择随下次消息发送；未选择的项目{{ newThread ? '使用桌面默认值' : '沿用当前对话' }}。</p>
    <form class="model-settings-form" :aria-busy="loading" @submit.prevent="save">
      <div class="model-settings-field">
        <label for="chat-model">模型</label>
        <select id="chat-model" :value="customModel ? '__custom__' : model" :disabled="fieldsDisabled" @change="chooseModel">
          <option value="">{{ currentModel ? modelName(currentModel) + '（当前）' : inheritLabel }}</option>
          <option v-for="entry in recentModels" :key="entry" :value="entry">{{ modelName(entry) }}</option>
          <option value="__custom__">输入模型 ID…</option>
        </select>
        <p class="muted small">列表来自最近使用记录，是否可用由桌面在发送时确认。</p>
      </div>
      <div v-if="customModel" class="model-settings-field">
        <label for="custom-chat-model">模型 ID</label><input id="custom-chat-model" v-model="model" :disabled="fieldsDisabled" required maxlength="150" autocomplete="off" autocapitalize="none" spellcheck="false" />
      </div>
      <div class="model-settings-field">
        <label for="chat-effort">思考强度</label>
        <select id="chat-effort" v-model="effort" :disabled="fieldsDisabled">
          <option value="">{{ currentEffort ? effortName(currentEffort) + '（当前）' : inheritLabel }}</option>
          <option v-for="value in efforts" :key="value" :value="value">{{ effortName(value) }}</option>
        </select>
      </div>
      <p v-if="error" class="error-text" role="alert">{{ error }}</p>
      <p v-if="!connected" class="muted small" role="status">连接恢复后可选择模型。</p>
      <button v-if="!loaded && error" type="button" class="primary-button" :disabled="!connected" @click="readOptions">重新读取选项</button>
      <button v-else class="primary-button" :disabled="fieldsDisabled" aria-live="polite">{{ loading ? '正在读取选项…' : '用于下次发送' }}</button>
    </form>
  </NativeSheet>
</template>

<style scoped>
.model-settings-hint { margin: 0 0 16px; }
.model-settings-form { display: grid; gap: 14px; }
.model-settings-field label { margin: 0 0 6px; font-weight: 600; }
.model-settings-field p { margin: 6px 0 0; }
.model-settings-form > .primary-button { margin-top: 4px; }
</style>
