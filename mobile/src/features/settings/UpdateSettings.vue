<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ArrowRight, Close, Hide, TopRight, View } from '@element-plus/icons-vue';
import NativeSheet from '../../components/NativeSheet.vue';
import CollapseMotion from '../../components/CollapseMotion.vue';
import { request } from '../../api';
import type { DesktopUpdateSettings } from './useDesktopUpgrade';

const props = defineProps<{ open: boolean; upgrading: boolean; startUpgrade: (version: string) => Promise<void> }>();
const emit = defineEmits<{ close: []; closed: [] }>();
const status = ref<DesktopUpdateSettings>();
const loading = ref(false);
const busy = ref<'saving' | 'removing' | 'starting' | ''>('');
const token = ref('');
const reveal = ref(false);
const sourceOpen = ref(false);
const guideOpen = ref(false);
const confirmRemove = ref(false);
const error = ref('');
const notice = ref('');
const canUpgrade = computed(() => !!status.value?.update && status.value.mode !== 'source' && status.value.configured);
let controller: AbortController | undefined;

async function load() {
  controller?.abort();
  const pending = new AbortController();
  controller = pending;
  loading.value = true; error.value = ''; status.value = undefined;
  try {
    status.value = await request<DesktopUpdateSettings>('/api/update-settings', { signal: pending.signal });
  } catch (failure) {
    if (!pending.signal.aborted) error.value = (failure as Error).message;
  } finally {
    if (controller === pending) loading.value = false;
  }
}
async function save() {
  if (busy.value) return;
  busy.value = 'saving'; error.value = ''; notice.value = '';
  try {
    status.value = await request<DesktopUpdateSettings>('/api/update-settings', { method: 'POST', body: JSON.stringify({ token: token.value }) });
    token.value = ''; reveal.value = false; confirmRemove.value = false;
    notice.value = 'Token 已验证并保存。';
  } catch (failure) { error.value = (failure as Error).message; }
  finally { busy.value = ''; }
}
async function remove() {
  if (busy.value) return;
  busy.value = 'removing'; error.value = ''; notice.value = '';
  try {
    status.value = await request<DesktopUpdateSettings>('/api/update-settings', { method: 'DELETE' });
    token.value = ''; reveal.value = false; confirmRemove.value = false;
    notice.value = 'Token 已移除，自动升级需要重新配置。';
  } catch (failure) { error.value = (failure as Error).message; }
  finally { busy.value = ''; }
}
async function upgrade() {
  if (!status.value || !canUpgrade.value || busy.value || props.upgrading) return;
  busy.value = 'starting'; error.value = ''; notice.value = '';
  try {
    await props.startUpgrade(status.value.version);
    emit('close');
  } catch (failure) { error.value = (failure as Error).message; }
  finally { busy.value = ''; }
}
function clearForm() {
  token.value = ''; reveal.value = false; sourceOpen.value = false; guideOpen.value = false; confirmRemove.value = false;
  error.value = ''; notice.value = '';
}
watch(() => props.open, open => {
  if (open) void load();
  else controller?.abort();
});
onBeforeUnmount(() => { controller?.abort(); token.value = ''; });
</script>

<template>
  <NativeSheet :open="open" labelledby="update-settings-title" panel-class="settings-modal update-settings" :dismissible="!busy" @close="emit('close')" @after-close="clearForm(); emit('closed')">
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
      <p v-else-if="!status.configured" class="update-mode muted">配置 GitHub token 后，即可在手机上发起升级。</p>
      <button v-if="canUpgrade" type="button" class="primary-button" :disabled="!!busy || upgrading" :aria-busy="upgrading" @click="upgrade">{{ upgrading ? '正在后台升级…' : '立即升级' }}</button>
      <button v-if="status.configured" type="button" class="settings-row settings-action update-source" :aria-expanded="sourceOpen" aria-controls="update-source-content" @click="sourceOpen = !sourceOpen"><span>更新来源</span><ArrowRight :class="{ 'guide-expanded': sourceOpen }" /></button>
      <CollapseMotion :open="!status.configured || sourceOpen">
        <div id="update-source-content">
          <p v-if="status.source === 'environment'" class="update-mode muted">GitHub token 由电脑的 CODEX_MOBILE_GITHUB_TOKEN 环境变量提供。</p>
          <form v-else :aria-busy="busy === 'saving'" @submit.prevent="save">
            <label for="update-token">{{ status.configured ? '替换 token' : 'GitHub token' }}</label>
            <div class="update-token-field">
              <input id="update-token" v-model="token" :type="reveal ? 'text' : 'password'" :disabled="!!busy || upgrading" placeholder="粘贴你的 GitHub token" autocomplete="off" autocapitalize="none" :spellcheck="false" required aria-describedby="update-token-hint" />
              <button type="button" class="icon-button" :aria-label="reveal ? '隐藏输入的 token' : '显示输入的 token'" :aria-pressed="reveal" :disabled="!!busy || upgrading" @click="reveal = !reveal"><Hide v-if="reveal" /><View v-else /></button>
            </div>
            <p id="update-token-hint" class="muted small">凭据只保存在电脑上，用于读取私有仓库。</p>
            <button type="submit" class="primary-button" :disabled="!!busy || upgrading || !token.trim()">{{ busy === 'saving' ? '正在验证并保存…' : '保存并验证' }}</button>
          </form>
          <div v-if="status.source === 'file'" class="update-remove">
            <button class="text-button" :disabled="!!busy || upgrading" :aria-expanded="confirmRemove" @click="confirmRemove = !confirmRemove">移除已保存的 token</button>
            <CollapseMotion :open="confirmRemove"><div class="update-remove-confirm">
              <p class="small">移除后需要重新配置 token 才能升级。</p>
              <div class="key-actions" :aria-busy="busy === 'removing'"><button class="secondary-button" :disabled="!!busy || upgrading" @click="remove">{{ busy === 'removing' ? '正在移除…' : '确认移除' }}</button><button class="text-button" :disabled="!!busy" @click="confirmRemove = false">取消</button></div>
            </div></CollapseMotion>
          </div>
          <div v-if="status.source !== 'environment'" class="update-guide">
            <button class="settings-row settings-action" :aria-expanded="guideOpen" aria-controls="update-guide-content" @click="guideOpen = !guideOpen"><span>如何生成 token</span><ArrowRight :class="{ 'guide-expanded': guideOpen }" /></button>
            <CollapseMotion :open="guideOpen"><div id="update-guide-content" class="update-guide-content">
              <ol>
                <li>打开 GitHub 的 token 创建页，填写名称和有效期。</li>
                <li>在 Repository access 选择 Only select repositories，仅勾选 <strong>J-Smile/codex-mobile</strong>。</li>
                <li>在 Repository permissions 将 <strong>Contents</strong> 设为 <strong>Read-only</strong>，其余保持默认。</li>
                <li>点击 Generate token，复制生成的 token，回到这里粘贴并保存。</li>
              </ol>
              <a class="secondary-button update-github-link" href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noopener noreferrer">打开 GitHub 生成 token<TopRight /></a>
              <p class="muted small">Token 只在生成时显示一次；到期后在此替换新的 token。</p>
            </div></CollapseMotion>
          </div>
        </div>
      </CollapseMotion>
    </template>
    <p v-if="error" class="error-text" role="alert">{{ error }}</p>
    <button v-if="!loading && !status" class="secondary-button" @click="load">重新读取</button>
    <p v-if="notice" class="update-notice" role="status">{{ notice }}</p>
  </NativeSheet>
</template>

<style scoped>
.update-versions { margin: 0; padding-inline: 12px; border-radius: 12px; border: 1px solid var(--border); background: var(--page); }
.update-status { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-block: 12px; font-size: 14px; }
.update-status + .update-status { border-top: 1px solid var(--border); }
.update-status dt { flex-shrink: 0; font-weight: 600; }
.update-status dd { min-width: 0; margin: 0; color: var(--muted); text-align: right; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }
.update-mode { margin: 8px 0 16px; font-size: 13px; line-height: 1.65; overflow-wrap: anywhere; }
.update-token-field { position: relative; }
.update-token-field input { padding-right: 52px; }
.update-token-field .icon-button { position: absolute; right: 2px; top: 2px; }
.update-remove { margin-top: 8px; }
.update-remove > .text-button { min-height: 44px; font-size: 13px; color: var(--muted); }
.update-remove-confirm p { margin: 0 0 12px; }
.update-notice { margin: 14px 0 0; padding: 10px 12px; background: var(--panel); border-radius: 10px; color: var(--online); font-size: 13px; line-height: 1.65; }
.update-guide, .update-source { margin-top: 16px; border-top: 1px solid var(--border); }
.update-guide .settings-row, .update-source { font-size: 14px; border-bottom: 0; }
.update-guide .settings-row > svg, .update-source > svg { transition: transform var(--motion-fast) ease; }
.update-guide .guide-expanded, .update-source .guide-expanded { transform: rotate(90deg); }
.update-guide-content { padding: 0 0 4px; font-size: 14px; line-height: 1.7; overflow-wrap: anywhere; }
.update-guide-content ol { margin: 0 0 16px; padding-left: 22px; }
.update-guide-content li + li { margin-top: 12px; }
.update-github-link { text-decoration: none; }
.update-github-link:hover { background: var(--hover); }
.update-github-link svg { width: 16px; height: 16px; flex-shrink: 0; }
.update-guide-content > p { margin: 12px 0 0; }
@media (prefers-reduced-motion: reduce) { .update-guide .settings-row > svg, .update-source > svg { transition: none; } }
</style>
