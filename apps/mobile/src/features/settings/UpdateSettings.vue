<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { Close } from '@element-plus/icons-vue';
import NativeSheet from '../../components/NativeSheet.vue';
import { request } from '../../api';
import type { DesktopUpdateSettings } from './useDesktopUpgrade';

const props = defineProps<{ open: boolean; upgrading: boolean; progress?: DesktopUpdateSettings['update']; startUpgrade: (version: string) => Promise<void> }>();
const emit = defineEmits<{ close: []; closed: [] }>();
const status = ref<DesktopUpdateSettings>();
const loading = ref(false);
const busy = ref<'starting' | ''>('');
const error = ref('');
const canUpgrade = computed(() => !!status.value?.update && status.value.mode !== 'source');
let controller: AbortController | undefined;

async function load() {
  controller?.abort();
  const pending = new AbortController();
  controller = pending;
  loading.value = true; error.value = ''; status.value = undefined;
  try {
    const loaded = await request<DesktopUpdateSettings>('/api/update-settings', { signal: pending.signal });
    if (!pending.signal.aborted) status.value = loaded;
  } catch (failure) {
    if (!pending.signal.aborted) error.value = (failure as Error).message;
  } finally {
    if (controller === pending) loading.value = false;
  }
}
async function upgrade() {
  if (!status.value || !canUpgrade.value || busy.value || props.upgrading) return;
  busy.value = 'starting'; error.value = '';
  try {
    await props.startUpgrade(status.value.version);
    emit('close');
  } catch (failure) { error.value = (failure as Error).message; }
  finally { busy.value = ''; }
}
watch(() => props.open, open => {
  if (open) void load();
  else controller?.abort();
});
onBeforeUnmount(() => controller?.abort());
</script>

<template>
  <NativeSheet :open="open" labelledby="update-settings-title" panel-class="settings-modal update-settings" :dismissible="!busy" @close="emit('close')" @after-close="error = ''; emit('closed')">
    <div class="modal-heading"><h2 id="update-settings-title">在线升级</h2><button class="icon-button" aria-label="关闭在线升级" :disabled="!!busy" @click="emit('close')"><Close /></button></div>
    <p v-if="loading" class="muted" role="status">正在读取版本…</p>
    <template v-else-if="status">
      <dl class="update-versions">
        <div class="update-status"><dt>Codex Mobile</dt><dd>v{{ status.version }}</dd></div>
        <div class="update-status">
          <dt>App Server</dt>
          <dd v-if="status.appServer?.version">v{{ status.appServer.version }}</dd>
          <dd v-else>{{ status.appServer?.connected === false ? '未连接' : '版本未知' }}</dd>
        </div>
      </dl>
      <p v-if="status.mode === 'source'" class="update-mode muted">电脑正在从源码运行。安装版支持在线升级。</p>
      <p v-else-if="!status.update" class="update-mode muted">请先在电脑上更新 Codex Mobile，即可从手机发起升级。</p>
      <p v-else class="update-mode muted">自动获取正式版本。</p>
      <button v-if="canUpgrade" type="button" class="primary-button" :disabled="!!busy || upgrading" :aria-busy="upgrading" @click="upgrade">{{ upgrading ? '正在后台升级…' : '立即升级' }}</button>
      <p v-if="upgrading && progress?.message" class="update-mode muted" role="status">{{ progress.message }}</p>
    </template>
    <p v-if="error" class="error-text" role="alert">{{ error }}</p>
    <button v-if="!loading && !status" class="secondary-button" @click="load">重新读取</button>
  </NativeSheet>
</template>

<style scoped>
.update-versions { margin: 0; padding-inline: 12px; border-radius: 12px; border: 1px solid var(--border); background: var(--page); }
.update-status { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-block: 12px; font-size: 14px; }
.update-status + .update-status { border-top: 1px solid var(--border); }
.update-status dt { flex-shrink: 0; font-weight: 600; }
.update-status dd { min-width: 0; margin: 0; color: var(--muted); text-align: right; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }
.update-mode { margin: 8px 0 16px; font-size: 13px; line-height: 1.65; overflow-wrap: anywhere; }
</style>
