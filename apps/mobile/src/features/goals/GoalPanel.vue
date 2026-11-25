<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch } from 'vue';
import { Close, EditPen, Loading, Refresh, VideoPause, VideoPlay } from '@element-plus/icons-vue';
import { request, threadUrl } from '../../api';
import NativeSheet from '../../components/NativeSheet.vue';
import { goalStatusLabels, type GoalInput, type GoalState, type ThreadGoal } from './goals';

const props = defineProps<{
  open: boolean;
  threadId: string;
  goal?: ThreadGoal | null;
  completedGoal?: ThreadGoal | null;
  initialObjective?: string;
  createGoal?: (input: GoalInput) => Promise<void>;
}>();
const emit = defineEmits<{
  close: [];
  'after-close': [];
  changed: [value: GoalState & { threadId: string }];
}>();
const titleId = useId();
const goal = ref<ThreadGoal | null>(null);
const completedGoal = ref<ThreadGoal | null>(null);
const loading = ref(false);
const ready = ref(false);
const busy = ref<'' | 'saving' | 'pausing' | 'resuming' | 'ending'>('');
const loadError = ref('');
const actionError = ref('');
const formOpen = ref(false);
const objective = ref('');
const tokenBudget = ref<string | number>('');
const objectiveInput = ref<HTMLTextAreaElement>();
const shownGoal = computed(() => goal.value || completedGoal.value);
const editingExisting = computed(() => !!goal.value && goal.value.status !== 'complete');
const canResume = computed(() => !!goal.value && (['paused', 'blocked', 'usageLimited'].includes(goal.value.status)
  || (goal.value.status === 'budgetLimited' && (goal.value.tokenBudget === null || goal.value.tokenBudget > goal.value.tokensUsed))));
const budgetNumber = computed(() => tokenBudget.value === '' ? null : Number(tokenBudget.value));
const budgetValid = computed(() => budgetNumber.value === null || (Number.isInteger(budgetNumber.value) && budgetNumber.value > 0));
const canSave = computed(() => formOpen.value && ready.value && !loading.value && !busy.value && !!objective.value.trim() && budgetValid.value
  && !!(props.threadId || props.createGoal));
let reader: AbortController | undefined;
let creatingThread = false;

async function load(preserveDraft = false) {
  if (!props.open) return;
  reader?.abort();
  const controller = new AbortController();
  reader = controller;
  const threadId = props.threadId;
  const parentGoal = props.goal;
  const parentCompleted = props.completedGoal;
  loading.value = true;
  ready.value = false;
  loadError.value = '';
  if (!preserveDraft) actionError.value = '';
  try {
    let state: GoalState = { goal: null, completedGoal: null };
    if (threadId) state = await request<GoalState>(threadUrl(threadId, 'goal'), { signal: controller.signal });
    else {
      const service = await request<{ ready: boolean }>('/api/goal-service', { signal: controller.signal });
      if (!service.ready) throw new Error('目标服务尚未就绪，请重试。');
    }
    if (controller.signal.aborted) return;
    goal.value = threadId && props.goal !== parentGoal && props.goal !== undefined ? props.goal : state.goal;
    completedGoal.value = threadId && props.completedGoal !== parentCompleted && props.completedGoal !== undefined
      ? props.completedGoal : state.completedGoal ?? null;
    if (!preserveDraft || !formOpen.value) {
      objective.value = goal.value?.objective || props.initialObjective || '';
      tokenBudget.value = goal.value?.tokenBudget == null ? '' : String(goal.value.tokenBudget);
      formOpen.value = !goal.value && !completedGoal.value;
    }
    ready.value = true;
  } catch (failure) {
    if (!controller.signal.aborted) loadError.value = (failure as Error).message;
  } finally {
    if (!controller.signal.aborted) loading.value = false;
  }
}

watch([() => props.open, () => props.threadId], ([open]) => {
  if (creatingThread) return;
  reader?.abort();
  if (!open) return;
  goal.value = props.goal ?? null;
  completedGoal.value = props.completedGoal ?? null;
  formOpen.value = false;
  void load();
}, { immediate: true });
watch([() => props.goal, () => props.completedGoal], ([value, completed]) => {
  if (!props.open) return;
  if (value !== undefined) goal.value = value;
  if (completed !== undefined) completedGoal.value = completed;
  if (value?.status === 'complete') formOpen.value = false;
}, { deep: true });
onBeforeUnmount(() => reader?.abort());

async function edit(newGoal = false) {
  objective.value = newGoal ? props.initialObjective || '' : goal.value?.objective || props.initialObjective || '';
  tokenBudget.value = newGoal || goal.value?.tokenBudget == null ? '' : String(goal.value.tokenBudget);
  actionError.value = '';
  formOpen.value = true;
  await nextTick();
  objectiveInput.value?.focus({ preventScroll: true });
}

async function save() {
  if (!canSave.value) return;
  const threadId = props.threadId;
  const input: GoalInput = { objective: objective.value.trim(),
    ...(budgetNumber.value !== null ? { tokenBudget: budgetNumber.value } : editingExisting.value ? { tokenBudget: null } : {}) };
  busy.value = 'saving';
  creatingThread = !threadId;
  actionError.value = '';
  try {
    if (creatingThread) await props.createGoal!(input);
    else {
      const state = await request<GoalState>(threadUrl(threadId, 'goal'), {
        method: 'PATCH', body: JSON.stringify({ ...input, ...(!editingExisting.value ? { status: 'active' } : {}) }),
      });
      goal.value = state.goal;
      completedGoal.value = state.completedGoal ?? null;
      emit('changed', { threadId, ...state });
    }
    emit('close');
  } catch (failure) {
    actionError.value = (failure as Error).message;
    if (creatingThread && props.threadId) await load(true);
  } finally {
    creatingThread = false;
    busy.value = '';
  }
}

async function setStatus(status: 'active' | 'paused') {
  if (!ready.value || busy.value || !goal.value || !props.threadId
    || (status === 'active' ? !canResume.value : goal.value.status !== 'active')) return;
  const threadId = props.threadId;
  busy.value = status === 'active' ? 'resuming' : 'pausing';
  actionError.value = '';
  try {
    const state = await request<GoalState>(threadUrl(threadId, 'goal'), { method: 'PATCH', body: JSON.stringify({ status }) });
    goal.value = state.goal;
    completedGoal.value = state.completedGoal ?? null;
    emit('changed', { threadId, ...state });
  } catch (failure) { actionError.value = (failure as Error).message; }
  finally { busy.value = ''; }
}

async function endGoal() {
  if (!ready.value || busy.value || !goal.value || goal.value.status === 'complete' || !props.threadId) return;
  const threadId = props.threadId;
  busy.value = 'ending';
  actionError.value = '';
  try {
    const state = await request<GoalState>(threadUrl(threadId, 'goal'), { method: 'DELETE' });
    goal.value = state.goal;
    completedGoal.value = state.completedGoal ?? null;
    emit('changed', { threadId, ...state });
    emit('close');
  } catch (failure) { actionError.value = (failure as Error).message; }
  finally { busy.value = ''; }
}
</script>

<template>
  <NativeSheet :open="open" :labelledby="titleId" :dismissible="!busy" panel-class="settings-modal goal-panel" @close="emit('close')" @after-close="emit('after-close')">
    <section class="goal-content">
    <header class="modal-heading"><h2 :id="titleId">目标</h2><button type="button" class="icon-button" aria-label="关闭目标" :disabled="!!busy" @click="emit('close')"><Close /></button></header>
    <p v-if="loading" class="goal-description" role="status">{{ threadId ? '正在读取目标…' : '正在连接目标服务…' }}</p>
    <div v-else-if="!ready" class="goal-read-error">
      <p class="error-text" role="alert">{{ loadError }}</p>
      <button type="button" class="secondary-button" @click="load(true)"><Refresh />重新读取目标</button>
    </div>
    <template v-else>
      <template v-if="shownGoal && !formOpen">
        <div class="goal-overview">
          <p class="goal-status" :data-status="shownGoal.status" role="status">{{ goalStatusLabels[shownGoal.status] }}</p>
          <p class="goal-objective">{{ shownGoal.objective }}</p>
          <meter v-if="shownGoal.tokenBudget !== null" class="goal-budget"
            :max="shownGoal.tokenBudget" :value="Math.min(shownGoal.tokensUsed, shownGoal.tokenBudget)"
            :aria-label="'已用 ' + shownGoal.tokensUsed.toLocaleString('zh-CN') + ' / ' + shownGoal.tokenBudget.toLocaleString('zh-CN') + ' Token'" />
        </div>
        <dl class="goal-metrics">
          <div><dt>已用 Token</dt><dd>{{ shownGoal.tokensUsed.toLocaleString('zh-CN') }}</dd></div>
          <div><dt>Token 预算</dt><dd>{{ shownGoal.tokenBudget == null ? '未设置' : shownGoal.tokenBudget.toLocaleString('zh-CN') }}</dd></div>
          <div><dt>已用时间</dt><dd>{{ shownGoal.timeUsedSeconds.toLocaleString('zh-CN') }} 秒</dd></div>
        </dl>
        <p v-if="goal?.status === 'budgetLimited'" class="goal-description">{{ canResume ? '预算已调整，可以继续目标。' : '预算已用完，调整预算后再继续。' }}</p>
        <p v-else-if="goal?.status === 'usageLimited'" class="goal-description">用量恢复后可以继续目标。</p>
        <p v-else-if="goal?.status === 'blocked'" class="goal-description">处理阻碍后，可以继续这个目标。</p>
        <div v-if="goal && goal.status !== 'complete'" class="goal-actions" :aria-busy="!!busy">
          <button v-if="goal.status === 'active'" type="button" class="primary-button" :disabled="!!busy" @click="setStatus('paused')"><Loading v-if="busy === 'pausing'" class="is-spin" /><VideoPause v-else />{{ busy === 'pausing' ? '正在暂停…' : '暂停目标' }}</button>
          <button v-else-if="canResume" type="button" class="primary-button" :disabled="!!busy" @click="setStatus('active')"><Loading v-if="busy === 'resuming'" class="is-spin" /><VideoPlay v-else />{{ busy === 'resuming' ? '正在继续…' : '继续目标' }}</button>
          <button type="button" class="secondary-button" :disabled="!!busy" @click="edit()"><EditPen />编辑目标</button>
          <button type="button" class="text-button goal-end" :disabled="!!busy" @click="endGoal">{{ busy === 'ending' ? '正在结束…' : '结束目标' }}</button>
        </div>
        <button v-else type="button" class="primary-button goal-new" :disabled="!!busy" @click="edit(true)">开始新目标</button>
      </template>
      <form v-if="formOpen" :aria-busy="busy === 'saving'" @submit.prevent="save">
        <label :for="titleId + '-objective'">{{ editingExisting ? '目标内容' : '希望完成什么' }}</label>
        <textarea :id="titleId + '-objective'" ref="objectiveInput" v-model="objective" rows="4" :disabled="!!busy" required placeholder="描述希望达成的结果" />
        <label :for="titleId + '-budget'">Token 预算<span class="goal-optional">选填</span></label>
        <input :id="titleId + '-budget'" v-model="tokenBudget" type="number" min="1" step="1" inputmode="numeric" :disabled="!!busy" :aria-invalid="!budgetValid" :aria-describedby="titleId + '-budget-hint'" placeholder="不设预算" />
        <p :id="titleId + '-budget-hint'" class="goal-description">{{ budgetValid ? editingExisting ? '留空会取消当前预算。' : '留空不设置 Token 预算。' : '请输入正整数，或留空不设预算。' }}</p>
        <div class="goal-form-actions">
          <button type="submit" class="primary-button" :disabled="!canSave"><Loading v-if="busy === 'saving'" class="is-spin" />{{ busy === 'saving' ? '正在保存…' : editingExisting ? '保存目标' : '设置目标' }}</button>
          <button type="button" class="text-button" :disabled="!!busy" @click="shownGoal ? formOpen = false : emit('close')">取消</button>
        </div>
      </form>
      <p v-if="actionError" class="error-text" role="alert">{{ actionError }}</p>
    </template>
    </section>
  </NativeSheet>
</template>

<style scoped>
.goal-description { margin: 8px 0 16px; color: var(--muted); font-size: 14px; line-height: 1.65; overflow-wrap: anywhere; }
.goal-overview { margin-bottom: 12px; padding: 14px; border-radius: 14px; background: var(--panel); }
.goal-status { display: flex; align-items: center; gap: 7px; margin: 0 0 6px; color: var(--muted); font-size: 13px; line-height: 1.5; }
.goal-status::before { content: ''; width: 6px; height: 6px; border-radius: 50%; background: currentColor; }
.goal-status[data-status="active"] { color: var(--signal); }
.goal-status[data-status="complete"] { color: var(--online); }
.goal-objective { margin: 0; font-size: 17px; font-weight: 600; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
.goal-budget { display: block; width: 100%; height: 8px; margin-top: 12px; }
.goal-budget::-webkit-meter-bar { background: var(--page); border: 0; }
.goal-budget::-webkit-meter-optimum-value { background: var(--signal); }
.goal-budget::-moz-meter-bar { background: var(--signal); }
.goal-metrics { margin: 0 0 16px; padding: 0; }
.goal-metrics > div { display: flex; justify-content: space-between; align-items: baseline; gap: 20px; padding-block: 8px; }
.goal-metrics > div + div { border-top: 1px solid var(--border); }
.goal-metrics dt { color: var(--muted); font-size: 14px; flex: none; }
.goal-metrics dd { min-width: 0; margin: 0; font-size: 16px; text-align: right; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }
.goal-actions, .goal-form-actions { display: flex; flex-direction: column; gap: 8px; }
.goal-actions :is(.primary-button, .secondary-button), .goal-read-error .secondary-button, .goal-form-actions .primary-button, .goal-new { width: 100%; min-height: 48px; margin-top: 0; }
.goal-actions .text-button, .goal-form-actions .text-button { min-height: 44px; font-size: 14px; }
.goal-end { color: var(--error); }
.goal-content textarea { display: block; width: 100%; min-height: 112px; padding: 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--panel); color: var(--text); font: inherit; font-size: 16px; line-height: 1.5; resize: vertical; }
.goal-content :is(textarea, input) { caret-color: var(--text); }
.goal-content :is(textarea, input):focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.goal-optional { margin-left: 8px; color: var(--muted); font-size: 12px; font-weight: 400; }
.goal-content :is(.primary-button, .secondary-button) > svg { width: 16px; height: 16px; flex: none; }
.goal-form-actions { margin-top: 20px; }
.goal-read-error { padding-top: 8px; }
</style>
