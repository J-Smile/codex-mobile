<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ArrowDown, Close, Refresh } from '@element-plus/icons-vue';
import type { DesktopThread } from '../../types';
import { request, threadUrl } from '../../api';
import NativeSheet from '../../components/NativeSheet.vue';

const props = defineProps<{ thread: DesktopThread | null; connected: boolean; newThread?: boolean; projectId?: string }>();
const emit = defineEmits<{ applied: [value: DesktopThread] }>();
const open = ref(false);
const presented = ref(false);
const loading = ref(false);
const saving = ref(false);
const fieldsDisabled = computed(() => !loaded.value || saving.value || !props.connected);
const error = ref('');
const loaded = ref(false);
const model = ref('');
const effort = ref('');
const recentModels = ref<string[]>([]);
const customModel = ref(false);
const modelName = (value: string) => value.replace(/^gpt-/, '').replace(/(^|[- ])([a-z])/g, (_, prefix, char) => (prefix ? ' ' : '') + char.toUpperCase());
const modelLabel = computed(() => modelName(props.thread?.model || 'Codex'));
const effortLabel = computed(() => {
  const value = props.thread?.reasoningEffort || '';
  return value ? value[0]!.toUpperCase() + value.slice(1) : '';
});
const defaultModel = ref('');
const defaultEffort = ref('');
const defaultsLoading = ref(false);
const defaultsError = ref('');
const defaultEffortLabel = computed(() => defaultEffort.value ? defaultEffort.value[0]!.toUpperCase() + defaultEffort.value.slice(1) : '');
let defaultRequest: AbortController | undefined;

async function readDefaults() {
  if (!props.newThread || !props.connected || defaultsLoading.value) return;
  const controller = new AbortController();
  defaultRequest = controller;
  defaultsLoading.value = true;
  defaultsError.value = '';
  try {
    const params = new URLSearchParams();
    if (props.projectId) params.set('projectId', props.projectId);
    const data = await request<{ model: string; effort: string | null }>('/api/new-thread-settings' + (params.size ? '?' + params.toString() : ''), { signal: controller.signal });
    if (controller.signal.aborted) return;
    if (typeof data.model !== 'string' || !data.model.trim()) throw new Error('未能读取桌面默认模型。');
    defaultModel.value = data.model.trim();
    defaultEffort.value = typeof data.effort === 'string' ? data.effort.trim() : '';
  } catch (failure) {
    if (!controller.signal.aborted) defaultsError.value = (failure as Error).message;
  } finally {
    if (defaultRequest === controller) { defaultRequest = undefined; defaultsLoading.value = false; }
  }
}

watch([() => props.newThread, () => props.connected, () => props.projectId], ([creating, connected, projectId], previous) => {
  if (!creating || projectId !== previous?.[2]) {
    defaultRequest?.abort();
    defaultRequest = undefined;
    defaultsLoading.value = false;
    defaultModel.value = '';
    defaultEffort.value = '';
    defaultsError.value = '';
  }
  if (creating && connected && !defaultModel.value) void readDefaults();
}, { immediate: true });
onBeforeUnmount(() => defaultRequest?.abort());

watch([() => props.thread?.id, () => props.newThread], () => { open.value = false; });
let settingsRequest: AbortController | undefined;
async function readSettings() {
  if (!open.value || props.newThread || !props.thread) return;
  settingsRequest?.abort();
  const controller = new AbortController();
  settingsRequest = controller;
  loading.value = true;
  loaded.value = false;
  error.value = '';
  customModel.value = false;
  try {
    const data = await request<{ model: string; effort: string | null; mode: string; recentModels?: string[] }>(threadUrl(props.thread.id, 'settings'), { signal: controller.signal });
    if (controller.signal.aborted) return;
    if (typeof data.model !== 'string' || !data.model.trim()) throw new Error('桌面没有返回当前模型，请重新读取。');
    model.value = data.model;
    effort.value = data.effort || '';
    recentModels.value = [...new Set([data.model, ...(data.recentModels || [])].filter(Boolean))];
    loaded.value = true;
  } catch (failure) {
    if (!controller.signal.aborted) error.value = (failure as Error).message;
  } finally { if (!controller.signal.aborted) loading.value = false; }
}
watch(open, (value) => {
  if (value) { presented.value = true; void readSettings(); }
  else { settingsRequest?.abort(); loading.value = false; }
});
onBeforeUnmount(() => settingsRequest?.abort());
function chooseModel(event: Event) {
  const selected = (event.target as HTMLSelectElement).value;
  customModel.value = selected === '__custom__';
  if (!customModel.value) model.value = selected;
}
async function save() {
  if (props.newThread || !props.thread || !props.connected || !loaded.value || loading.value || saving.value) return;
  const id = props.thread.id;
  saving.value = true;
  error.value = '';
  try {
    const result = await request<{ applied: boolean }>(threadUrl(id, 'settings'), { method: 'POST', body: JSON.stringify({ model: model.value.trim(), effort: effort.value || null }) });
    if (!result.applied) throw new Error('模型设置尚未应用，请重试。');
    if (props.thread?.id === id) {
      emit('applied', { ...props.thread, model: model.value.trim(), reasoningEffort: effort.value || null });
      open.value = false;
    }
  } catch (failure) { if (props.thread?.id === id) error.value = (failure as Error).message; }
  finally { saving.value = false; }
}
</script>

<template>
  <div v-if="newThread" class="model-default default-model" :aria-busy="defaultsLoading" :title="defaultsError || [defaultModel, defaultEffortLabel].filter(Boolean).join(' · ')">
    <span class="model-name" role="status">{{ defaultsLoading ? '读取模型…' : defaultsError ? '模型读取失败' : defaultModel ? modelName(defaultModel) : '等待读取模型' }}</span><span v-if="defaultModel && !defaultsError" class="model-effort">{{ defaultEffortLabel }}</span>
    <button v-if="defaultsError" type="button" class="icon-button default-model-retry" aria-label="重新读取默认模型" :disabled="!connected" @click="readDefaults"><Refresh /></button>
  </div>
  <button v-else type="button" class="model-button" :disabled="!thread || !connected" aria-label="模型与思考强度" aria-haspopup="dialog" :aria-expanded="presented" :title="thread?.model || '模型与思考强度'" @click="open = true">
    <span class="model-name">{{ modelLabel }}</span><span class="model-effort">{{ effortLabel }}</span><ArrowDown />
  </button>
  <NativeSheet :open="open" :dismissible="!saving" labelledby="model-title" panel-class="settings-modal" @close="open = false" @after-close="presented = false">
        <div class="modal-heading"><h2 id="model-title">模型与思考强度</h2><button class="icon-button" aria-label="关闭模型设置" :disabled="saving" @click="open = false"><Close /></button></div>
        <p class="model-settings-hint muted small">下次发送消息时生效。</p>
        <form class="model-settings-form" :aria-busy="loading || saving" @submit.prevent="save">
          <div class="model-settings-field">
            <label for="chat-model">模型</label><select id="chat-model" :value="!loaded ? '' : customModel ? '__custom__' : model" :disabled="fieldsDisabled" @change="chooseModel"><option v-if="!loaded" value="">—</option><option v-for="entry in recentModels" :key="entry" :value="entry">{{ modelName(entry) }}</option><option value="__custom__">其他模型…</option></select>
          </div>
          <div v-if="customModel" class="model-settings-field">
            <label for="custom-chat-model">模型 ID</label><input id="custom-chat-model" v-model="model" :disabled="saving || !connected" required autocomplete="off" autocapitalize="none" spellcheck="false" />
          </div>
          <div class="model-settings-field">
            <label for="chat-effort">思考强度</label><select id="chat-effort" :value="loaded ? effort : ''" :disabled="fieldsDisabled" @change="effort = ($event.target as HTMLSelectElement).value"><option value="">{{ loaded ? '桌面默认' : '—' }}</option><option v-for="value in ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra']" :key="value" :value="value">{{ value[0]!.toUpperCase() + value.slice(1) }}</option></select>
          </div>
          <p v-if="error" class="error-text" role="alert">{{ error }}</p>
          <p v-if="!connected" class="muted small" role="status">连接已断开，恢复后可应用设置。</p>
          <button v-if="!loaded && error" type="button" class="primary-button" :disabled="!connected" @click="readSettings">重新读取设置</button>
          <button v-else class="primary-button" :disabled="fieldsDisabled || !model.trim()" aria-live="polite">{{ !loaded ? '正在读取桌面设置…' : saving ? '正在保存…' : '应用设置' }}</button>
        </form>
  </NativeSheet>
</template>

<style scoped>
.default-model { align-items: center; gap: 5px; flex: 0 1 auto; min-width: 0; max-width: 190px; min-height: 44px; }
.default-model-retry { margin-right: -8px; }
.default-model-retry svg { width: 16px; height: 16px; }
.model-settings-hint { margin: 0 0 16px; }
.model-settings-form { display: grid; gap: 14px; }
.model-settings-field label { margin: 0 0 6px; font-weight: 600; }
.model-settings-form > .primary-button { margin-top: 4px; }
</style>
