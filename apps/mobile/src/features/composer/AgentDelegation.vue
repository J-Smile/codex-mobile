<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { ArrowDown, Close, Loading } from '@element-plus/icons-vue';
import { delegationPrompt, type DelegationRequest, type SubagentTarget } from './subagentDelegation';
import NativeSheet from '../../components/NativeSheet.vue';

const props = withDefaults(defineProps<{
  open: boolean;
  targets: SubagentTarget[];
  initialTask?: string;
  busy?: boolean;
  error?: string;
  unavailable?: string;
  queueSupported?: boolean;
}>(), { initialTask: '', busy: false, error: '', unavailable: '', queueSupported: true });
const emit = defineEmits<{ submit: [request: DelegationRequest]; close: []; 'after-close': [] }>();
const task = ref(props.initialTask);
const targetId = ref('');
const mode = ref<'now' | 'queue'>('now');
const target = computed(() => props.targets.find((agent) => agent.id === targetId.value));
const canSubmit = computed(() => !props.busy && !props.unavailable && !!task.value.trim() && (!targetId.value || !!target.value));

function close() {
  if (!props.busy) emit('close');
}

function submit() {
  if (!canSubmit.value || (mode.value === 'queue' && props.queueSupported === false)) return;
  emit('submit', { prompt: delegationPrompt(task.value, target.value), mode: mode.value, target: target.value });
}

watch(() => props.open, (open) => {
  if (!open) return;
  task.value = props.initialTask;
  targetId.value = '';
  mode.value = 'now';
}, { immediate: true });
watch(() => props.queueSupported, supported => { if (supported === false) mode.value = 'now'; });
</script>

<template>
  <NativeSheet :open="open" labelledby="delegation-title" :dismissible="!busy" panel-class="settings-modal" @close="close" @after-close="emit('after-close')">
      <section class="delegation-sheet">
        <div class="modal-heading">
          <h2 id="delegation-title">交给子智能体</h2>
          <button type="button" class="icon-button" aria-label="关闭子智能体指派" :disabled="busy" @click="close"><Close /></button>
        </div>
        <form :aria-busy="busy" @submit.prevent="submit">
          <label for="delegation-task">任务内容</label>
          <textarea id="delegation-task" v-model="task" class="delegation-task" :disabled="busy" rows="4" required autofocus aria-describedby="delegation-hint" placeholder="交给它什么任务？"></textarea>
          <div class="delegation-options">
            <div class="delegation-field-row">
              <label for="delegation-target">执行者</label>
              <select id="delegation-target" v-model="targetId" :disabled="busy">
                <option value="">新建子智能体</option>
                <optgroup v-if="targets.length" label="已有子智能体">
                  <option v-for="agent in targets" :key="agent.id" :value="agent.id">{{ agent.name }} · {{ agent.status }}</option>
                </optgroup>
              </select><ArrowDown />
            </div>
            <div class="delegation-field-row">
              <label for="delegation-mode">处理时间</label>
              <select id="delegation-mode" v-model="mode" :disabled="busy">
                <option value="now">现在</option>
                <option v-if="queueSupported" value="queue">当前任务之后</option>
              </select><ArrowDown />
            </div>
          </div>
          <p id="delegation-hint" class="muted small delegation-hint" role="status">{{ unavailable || '结果会汇总到当前对话。' }}</p>
          <p v-if="error" class="error-text" role="alert">{{ error }}</p>
          <button type="submit" class="primary-button" :disabled="!canSubmit">
            <Loading v-if="busy" class="is-spin" />{{ busy ? '正在提交…' : '委派任务' }}
          </button>
        </form>
      </section>
  </NativeSheet>
</template>

<style scoped>
.delegation-options { margin: 16px 0; padding-inline: 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--page); }
.delegation-field-row { display: flex; align-items: center; gap: 12px; min-height: 52px; }
.delegation-field-row + .delegation-field-row { border-top: 1px solid var(--border); }
.delegation-field-row label { flex: none; margin: 0; font-size: 14px; color: var(--muted); }
.delegation-field-row select { flex: 1; min-width: 0; min-height: 44px; width: auto; padding: 8px 0; border: 0; border-radius: 0; background: transparent; text-align: right; text-align-last: right; font-size: 16px; appearance: none; }
.delegation-field-row > svg { width: 12px; height: 12px; color: var(--muted); pointer-events: none; }
.delegation-task { display: block; width: 100%; min-height: 112px; max-height: 32dvh; padding: 12px; border: 1px solid var(--border); border-radius: 12px; color: var(--text); background: var(--panel); font: inherit; font-size: 16px; line-height: 1.6; resize: none; overflow-wrap: anywhere; }
.delegation-task::placeholder { color: var(--muted); }
.delegation-task:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.delegation-hint { margin: 12px 0 20px; }
.delegation-sheet form > label { margin-block: 12px 8px; font-weight: 600; }
@media (max-height: 450px) { .delegation-task { min-height: 84px; } }
</style>
