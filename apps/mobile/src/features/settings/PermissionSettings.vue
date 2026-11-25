<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue';
import { Check, Close, Loading, Lock, Warning } from '@element-plus/icons-vue';
import { request, threadUrl } from '../../api';
import type { DesktopThread } from '../../types';
import NativeSheet from '../../components/NativeSheet.vue';

const props = withDefaults(defineProps<{ thread: DesktopThread | null; connected: boolean; active: boolean; newThread?: boolean; editable?: boolean }>(), { editable: true });
const titleId = useId();
const open = ref(false);
const presented = ref<'thread' | 'defaults' | null>(null);
const saving = ref(false);
const error = ref('');
type Permissions = { mode: string; profile: string | null };
const permissions = ref<Permissions | null>(null);
const selected = ref('');
const choices = [
  { id: 'read-only', label: '只读', detail: '查看文件，需要修改时询问你' },
  { id: 'workspace-write', label: '工作区访问', detail: '可修改当前项目，超出工作区时询问你' },
  { id: 'full-access', label: '完全访问', detail: '可访问电脑文件和网络，不再逐项询问' },
];
const reading = computed(() => !permissions.value && !error.value);
const canChoose = computed(() => !!permissions.value && props.connected && !saving.value);
const canApply = computed(() => canChoose.value && selected.value !== permissions.value?.mode
  && choices.some(choice => choice.id === selected.value));
const fullAccess = computed(() => !props.newThread && (props.thread?.permissionProfile === ':danger-full-access'
  || (!props.thread?.permissionProfile && props.thread?.permissionMode === 'dangerFullAccess')));
const label = computed(() => props.newThread ? '桌面默认' : fullAccess.value ? '完全访问'
  : props.thread?.permissionProfile === ':read-only' || (!props.thread?.permissionProfile && props.thread?.permissionMode === 'readOnly') ? '只读'
  : props.thread?.permissionProfile === ':workspace' || (!props.thread?.permissionProfile && props.thread?.permissionMode === 'workspaceWrite') ? '工作区访问' : '访问权限');

watch([() => props.thread?.id, () => props.newThread], () => { open.value = false; });
let settingsRequest: AbortController | undefined;
async function readSettings() {
  if (!open.value || props.newThread || !props.thread || props.editable === false) return;
  settingsRequest?.abort();
  const controller = new AbortController();
  settingsRequest = controller;
  permissions.value = null;
  selected.value = '';
  error.value = '';
  try {
    const data = await request<{ permissions: Permissions }>(threadUrl(props.thread.id, 'settings'), { signal: controller.signal });
    if (controller.signal.aborted) return;
    const current = data?.permissions;
    if (!current || (current.mode !== 'custom' && !choices.some(choice => choice.id === current.mode))) {
      throw new Error('未能读取当前权限，请重新读取。');
    }
    permissions.value = current;
    selected.value = current.mode;
  } catch (failure) { if (!controller.signal.aborted) error.value = (failure as Error).message; }
}
watch(open, visible => {
  if (visible) { presented.value = props.newThread ? 'defaults' : 'thread'; void readSettings(); }
  else settingsRequest?.abort();
});
onBeforeUnmount(() => settingsRequest?.abort());

async function apply() {
  if (!open.value || props.newThread || !props.thread || !canApply.value) return;
  const threadId = props.thread.id;
  saving.value = true;
  error.value = '';
  try {
    const result = await request<{ applied: boolean }>(threadUrl(threadId, 'settings'), { method: 'POST', body: JSON.stringify({ permissions: selected.value }) });
    if (!result.applied) throw new Error('权限设置尚未应用，请稍后重试。');
    if (props.thread?.id === threadId) open.value = false;
  } catch (failure) { if (props.thread?.id === threadId) error.value = (failure as Error).message; }
  finally { saving.value = false; }
}
</script>

<template>
  <button type="button" class="permission-control permission-indicator" :class="{ 'full-access': fullAccess }" :disabled="(!newThread && !thread) || !connected" :aria-label="'访问权限：' + label" :title="'访问权限：' + label" aria-haspopup="dialog" :aria-expanded="!!presented" @click="open = true"><Warning v-if="fullAccess" /><Lock v-else /><span>{{ label }}</span></button>
  <NativeSheet :open="open" :labelledby="titleId" :dismissible="!saving" panel-class="settings-modal" style="--sheet-width: 420px" @close="open = false" @after-close="presented = null">
      <section>
        <header class="modal-heading"><h2 :id="titleId">访问权限</h2><button class="icon-button" type="button" aria-label="关闭访问权限" :disabled="saving" @click="open = false"><Close /></button></header>
        <template v-if="presented === 'defaults' || editable === false">
          <p class="permission-description">{{ newThread ? '新对话使用桌面默认权限。' : '手机沿用当前对话的桌面权限。' }}{{ editable === false ? '当前连接不支持修改权限，请在桌面调整。' : '创建后可在这里调整。' }}</p>
          <footer><button type="button" class="primary-button permission-apply" @click="open = false">知道了</button></footer>
        </template>
        <template v-else>
          <p class="permission-description">{{ active ? '当前任务继续使用已有权限，下次发送时生效。' : '下次发送消息时生效。' }}</p>
          <form @submit.prevent="apply">
            <p v-if="permissions?.mode === 'custom'" class="permission-description">当前使用自定义权限{{ permissions.profile ? '：' + permissions.profile : '' }}。</p>
            <div class="permission-choices" role="radiogroup" aria-label="权限范围" :aria-busy="reading || saving">
              <label v-for="choice in choices" :key="choice.id" class="permission-choice">
                <input v-model="selected" type="radio" :name="titleId" :value="choice.id" :disabled="!canChoose" />
                <span><strong>{{ choice.label }}</strong><small>{{ choice.detail }}</small></span>
                <span class="permission-mark" aria-hidden="true"><Check v-if="selected === choice.id" /></span>
              </label>
            </div>
            <p v-if="error" class="permission-error" role="alert">{{ error }}</p>
            <p v-if="!connected" class="permission-description" role="status">连接已断开，恢复后可应用权限。</p>
            <footer aria-live="polite">
              <button v-if="!permissions && error" type="button" class="primary-button permission-apply" :disabled="!connected" @click="readSettings">重新读取权限</button>
              <button v-else type="submit" class="primary-button permission-apply" :disabled="!canApply"><Loading v-if="saving" class="is-spin" />{{ reading ? '正在读取权限…' : saving ? '正在应用…' : '应用权限' }}</button>
            </footer>
          </form>
        </template>
      </section>
  </NativeSheet>
</template>

<style scoped>
.permission-control { display: inline-flex; align-items: center; gap: 5px; min-width: 44px; min-height: 44px; padding: 0 6px; border: 0; border-radius: 8px; background: transparent; color: var(--muted); font-size: 11px; white-space: nowrap; }
.permission-control svg { width: 18px; height: 18px; }
.permission-control.full-access { color: var(--permission); }
.permission-description { margin: 8px 0 16px; color: var(--muted); font-size: 13px; line-height: 1.6; overflow-wrap: anywhere; }
.permission-choices { display: grid; gap: 8px; }
.permission-choices .permission-choice { display: grid; grid-template-columns: minmax(0, 1fr) 20px; align-items: center; gap: 12px; min-height: 66px; margin: 0; padding: 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--page); cursor: pointer; }
.permission-choice input { position: absolute; width: 1px; height: 1px; min-height: 0; margin: 0; padding: 0; opacity: 0; }
.permission-choice:has(input:disabled) { cursor: default; opacity: .5; }
.permission-choice:has(input:focus-visible) { outline: 2px solid var(--text); outline-offset: -3px; }
.permission-choice strong { display: block; font-size: 15px; line-height: 1.5; font-weight: 600; }
.permission-choice small { display: block; margin-top: 3px; color: var(--muted); font-size: 12px; line-height: 1.5; }
.permission-mark { display: grid; place-items: center; width: 20px; height: 20px; border: 1px solid var(--border); border-radius: 50%; }
.permission-mark svg { width: 14px; height: 14px; }
.permission-choice:has(input:checked) .permission-mark { border-color: var(--signal); color: var(--signal); }
.permission-error { margin: 12px 0 0; color: var(--error); font-size: 12px; line-height: 1.6; }
footer { display: flex; justify-content: flex-end; padding-top: 16px; }
.permission-apply { margin-top: 0; }
.permission-apply svg { width: 15px; height: 15px; }
button:disabled { opacity: .5; cursor: default; }
button:focus-visible { outline: 2px solid var(--text); outline-offset: 2px; }
</style>
