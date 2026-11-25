<script setup lang="ts">
import { computed, nextTick, reactive, ref, useId, watch } from 'vue';
import { ArrowDown, ArrowLeft, ArrowRight, ChatDotRound, Check, Close, EditPen, Loading } from '@element-plus/icons-vue';
import { request, threadUrl } from '../../api';
import CollapseMotion from '../../components/CollapseMotion.vue';
import { InteractionReplyState, questionAnswers, type InteractionDraft, type PendingInteraction } from './interactions';

const props = defineProps<{ threadId: string; interactions: PendingInteraction[]; disabled?: boolean; compact?: boolean; keyboardOpen?: boolean }>();
const emit = defineEmits<{ desktop: []; expand: []; state: [value: { expanded: boolean; pending: boolean }] }>();
const prefix = useId();
const replies = reactive(new InteractionReplyState());
const entry = ref<PendingInteraction>();
const draft = ref<InteractionDraft>();
const minimized = computed(() => !!props.compact || !!draft.value?.minimized);
const reopen = ref<HTMLButtonElement>();
const responseBody = ref<HTMLElement>();
const answerInput = ref<HTMLTextAreaElement | HTMLInputElement>();
const replying = ref(false);
watch(() => props.keyboardOpen, open => { if (!open) replying.value = false; });
watch([() => props.threadId, () => props.interactions], ([threadId, entries]) => replies.sync(threadId, entries), { immediate: true });
const active = computed(() => replies.pending[0]);
watch(active, (value) => {
  if (!value) return;
  entry.value = value;
  draft.value = replies.drafts[value.id];
}, { immediate: true });
const questions = computed(() => entry.value?.questions || []);
const question = computed(() => questions.value[Math.min(draft.value?.step || 0, Math.max(0, questions.value.length - 1))]);
watch([answerInput, () => draft.value?.text[question.value?.id || '']], async ([field]) => {
  await nextTick();
  if (!(field instanceof HTMLTextAreaElement)) return;
  field.style.height = 'auto';
  field.style.height = Math.min(field.scrollHeight, 96) + 'px';
}, { flush: 'post' });
const inactive = computed(() => props.disabled || !active.value || !!draft.value?.busy);
const currentAnswered = computed(() => !!(entry.value && draft.value && question.value && questionAnswers(entry.value, draft.value)[question.value.id]?.length));
const lastQuestion = computed(() => (draft.value?.step || 0) >= questions.value.length - 1);
watch([active, minimized], () => {
  if (!active.value || minimized.value) replying.value = false;
  emit('state', { pending: !!active.value, expanded: !!active.value && !minimized.value });
}, { immediate: true });
defineExpose({ minimize });

function afterClose() {
  if (!active.value) { entry.value = undefined; draft.value = undefined; }
}

async function minimize() {
  if (!draft.value || draft.value.busy) return;
  draft.value.minimized = true;
  await nextTick();
  reopen.value?.focus({ preventScroll: true });
}

function selectOption(index: number) {
  if (!draft.value || !question.value) return;
  draft.value.choices[question.value.id] = index;
  delete draft.value.skipped[question.value.id];
  draft.value.error = '';
}

function customInput() {
  if (!draft.value || !question.value) return;
  draft.value.choices[question.value.id] = -1;
  delete draft.value.skipped[question.value.id];
  draft.value.error = '';
}

async function changeQuestion(step: number) {
  if (!draft.value || inactive.value) return;
  draft.value.step = step;
  draft.value.error = '';
  await nextTick();
  responseBody.value?.focus({ preventScroll: true });
}

function skipQuestion() {
  if (!entry.value?.skipSupported || !draft.value || !question.value || inactive.value) return;
  if (entry.value.source !== 'async-message') { void submit('skip'); return; }
  draft.value.skipped[question.value.id] = true;
  delete draft.value.choices[question.value.id];
  delete draft.value.text[question.value.id];
  if (lastQuestion.value) void submit();
  else void changeQuestion(draft.value.step + 1);
}

async function submit(action = 'accept') {
  const current = entry.value;
  const currentDraft = draft.value;
  if (!current || !currentDraft || inactive.value || !current.replySupported) return;
  let payload: unknown;
  let localSkip = false;
  if (current.kind === 'questions') {
    if (action === 'skip' && current.skipSupported) payload = { skip: true };
    else {
      const answers = questionAnswers(current, currentDraft);
      const skipped = (current.questions || []).filter(value => currentDraft.skipped[value.id]).map(value => value.id);
      const missing = (current.questions || []).findIndex((value) => !currentDraft.skipped[value.id] && !answers[value.id]?.length);
      if (missing >= 0) {
        void changeQuestion(missing);
        currentDraft.error = '请选择一个选项，或填写回复。';
        return;
      }
      payload = { answers, ...(skipped.length ? { skipped } : {}) };
      localSkip = current.source === 'async-message' && !Object.keys(answers).length;
    }
  } else if (current.kind === 'form') {
    const content = Object.fromEntries((current.fields || []).flatMap((field) => {
      const value = currentDraft.fields[field.name];
      if (value === undefined || value === '') return [];
      return [[field.name, ['integer', 'number'].includes(field.type) ? Number(value) : value]];
    }));
    payload = { action, content };
  } else payload = { decision: action };
  const submission = replies.begin(current);
  if (!submission) return;
  if (localSkip) { replies.finish(submission); return; }
  try {
    await request(threadUrl(submission.threadId, 'interactions/' + encodeURIComponent(current.id)), { method: 'POST', body: JSON.stringify(payload) });
    replies.finish(submission);
  } catch (failure) {
    replies.fail(submission, (failure as Error).message);
    await nextTick();
    if (draft.value === submission.draft) responseBody.value?.scrollTo({ top: 0, behavior: 'instant' });
  }
}

function advance() {
  if (!draft.value || inactive.value) return;
  if (entry.value?.kind === 'questions' && !lastQuestion.value) {
    if (!currentAnswered.value) return;
    void changeQuestion(draft.value.step + 1);
  } else void submit();
}
</script>

<template>
  <CollapseMotion class="pending-interactions" :class="{ 'has-pending': !!active || !!entry, 'is-minimized': minimized, 'keyboard-open': keyboardOpen, 'is-replying': replying }" :open="!!active" @focusin="replying = !!active && !minimized" @after-close="afterClose">
    <section v-if="entry && draft" :key="draft.requestKey" class="pending-interaction" :aria-labelledby="prefix + '-title'" :aria-busy="draft.busy">
      <button v-if="minimized" ref="reopen" type="button" class="interaction-reopen" :aria-expanded="false" @click="draft.minimized = false; emit('expand')">
        <ChatDotRound /><span :id="prefix + '-title'">{{ entry.kind === 'questions' ? '有问题待回复' : entry.title }}</span><ArrowDown />
      </button>
      <div v-else class="interaction-heading">
        <ChatDotRound /><h3 :id="prefix + '-title'">{{ entry.kind === 'questions' ? '问题' : entry.title }}</h3>
        <span v-if="entry.kind === 'questions' && questions.length > 1" class="question-progress" :aria-label="'第 ' + (draft.step + 1) + ' 题，共 ' + questions.length + ' 题'">{{ draft.step + 1 }} / {{ questions.length }}</span>
        <span v-else-if="replies.pending.length > 1" class="question-progress">{{ replies.pending.length }} 项待处理</span>
        <button type="button" class="interaction-close" :disabled="draft.busy" :aria-label="entry.kind === 'questions' ? '收起待回复的问题' : '收起待处理请求'" @click="minimize"><Close /></button>
      </div>
      <CollapseMotion class="interaction-body" :open="!minimized" :focus-target="reopen">
        <template v-if="!entry.replySupported">
          <div class="interaction-scroll">
          <p v-if="entry.message" class="interaction-message">{{ entry.message }}</p>
          <pre v-if="entry.detail" class="interaction-detail">{{ entry.detail }}</pre>
          <p class="muted small">{{ entry.desktopMessage || '请在电脑上的 Codex 继续。' }}</p>
          </div>
          <div class="interaction-actions"><button type="button" class="interaction-desktop" @click="emit('desktop')">在电脑上打开<ArrowRight /></button></div>
        </template>
        <form v-else class="interaction-form" :class="{ 'question-form': entry.kind === 'questions' }" @submit.prevent="advance">
          <fieldset v-if="entry.kind === 'questions' && question" :key="question.id" :disabled="inactive" class="question-fieldset">
            <legend class="sr-only">{{ question.question }}</legend>
            <div ref="responseBody" class="question-scroll" tabindex="-1" role="group" :aria-label="question.question">
            <p v-if="draft.error" class="error-text" role="alert">{{ draft.error }}</p>
            <p v-else-if="disabled" class="interaction-status" role="status">重新连接后即可回复。</p>
            <p v-if="entry.message" class="interaction-message">{{ entry.message }}</p>
            <pre v-if="entry.detail" class="interaction-detail">{{ entry.detail }}</pre>
            <p class="question-prompt" aria-hidden="true">{{ question.question }}</p>
            <div class="question-options">
              <label v-for="(option, optionIndex) in question.options" :key="optionIndex" class="interaction-choice question-choice">
                <input :checked="draft.choices[question.id] === optionIndex" type="radio" :name="prefix + entry.id + question.id" :value="optionIndex" @change="selectOption(optionIndex)" />
                <span class="choice-number" aria-hidden="true">{{ optionIndex + 1 }}</span>
                <span class="choice-copy"><strong>{{ option.label }}</strong><small v-if="option.description">{{ option.description }}</small></span>
                <Check v-if="draft.choices[question.id] === optionIndex" class="choice-check" aria-hidden="true" />
              </label>
            </div>
            </div>
            <div class="question-footer" :class="{ 'multiple-questions': questions.length > 1 }">
            <label class="custom-answer" :class="{ chosen: draft.choices[question.id] === -1 && !!draft.text[question.id] }">
              <EditPen aria-hidden="true" />
              <input v-if="question.secret" ref="answerInput" v-model="draft.text[question.id]" type="password" autocomplete="off" :aria-label="question.options.length ? '自行撰写回复' : '你的回复'" :placeholder="question.options.length ? '或自行撰写回复' : '填写你的回复'" @input="customInput" />
              <textarea v-else ref="answerInput" v-model="draft.text[question.id]" rows="1" :aria-label="question.options.length ? '自行撰写回复' : '你的回复'" :placeholder="question.options.length ? '或自行撰写回复' : '填写你的回复'" @input="customInput" />
            </label>
            <div class="interaction-actions">
              <button v-if="draft.step > 0" type="button" class="interaction-previous" :disabled="inactive" aria-label="上一题" @click="changeQuestion(draft.step - 1)"><ArrowLeft /></button>
              <button v-if="entry.skipSupported" type="button" class="interaction-skip" :disabled="inactive" @click="skipQuestion">{{ entry.source !== 'async-message' && questions.length > 1 ? '跳过全部' : '跳过' }}</button>
              <button type="submit" class="interaction-submit" :disabled="inactive || !currentAnswered"><Loading v-if="draft.busy" class="is-spin" />{{ draft.busy ? '正在发送…' : lastQuestion ? '发送' : '下一题' }}<ArrowRight v-if="!lastQuestion && !draft.busy" /></button>
            </div>
            </div>
          </fieldset>
          <div v-if="entry.kind !== 'questions'" ref="responseBody" class="interaction-scroll">
          <p v-if="draft.error" class="error-text" role="alert">{{ draft.error }}</p>
          <p v-else-if="disabled" class="interaction-status" role="status">重新连接后即可回复。</p>
          <p v-if="entry.message" class="interaction-message">{{ entry.message }}</p>
          <pre v-if="entry.detail" class="interaction-detail">{{ entry.detail }}</pre>
          <template v-if="entry.kind === 'form'">
            <fieldset v-for="field in entry.fields" :key="field.name" :disabled="inactive">
              <legend>{{ field.label }}<span v-if="!field.required" class="field-optional">（选填）</span></legend>
              <p v-if="field.description" class="field-description">{{ field.description }}</p>
              <template v-if="field.type === 'boolean'">
                <label class="interaction-choice"><input v-model="draft.fields[field.name]" type="radio" :name="prefix + entry.id + field.name" :value="true" :required="field.required" /><span>是</span></label>
                <label class="interaction-choice"><input v-model="draft.fields[field.name]" type="radio" :name="prefix + entry.id + field.name" :value="false" :required="field.required" /><span>否</span></label>
              </template>
              <select v-else-if="field.options" v-model="draft.fields[field.name]" :multiple="field.type === 'array'" :required="field.required" :aria-label="field.label">
                <option v-if="field.type !== 'array'" value="" disabled>请选择</option>
                <option v-for="(option, index) in field.options" :key="option" :value="option">{{ field.optionNames?.[index] || option }}</option>
              </select>
              <input v-else-if="field.type === 'number' || field.type === 'integer'" v-model="draft.fields[field.name]" type="number" :step="field.type === 'integer' ? 1 : 'any'" :min="field.minimum" :max="field.maximum" :required="field.required" :aria-label="field.label" />
              <input v-else v-model="draft.fields[field.name]" :type="field.format === 'email' ? 'email' : 'text'" :minlength="field.minLength" :maxlength="field.maxLength" :required="field.required" :aria-label="field.label" autocomplete="off" />
            </fieldset>
          </template>
          </div>
          <div v-if="entry.kind !== 'questions'" class="interaction-actions">
            <button type="button" class="interaction-decline" :disabled="inactive" @click="submit('decline')">拒绝</button>
            <button type="submit" class="interaction-submit" :disabled="inactive"><Loading v-if="draft.busy" class="is-spin" />{{ draft.busy ? '正在发送…' : entry.approveLabel || '确认' }}</button>
          </div>
        </form>
      </CollapseMotion>
    </section>
  </CollapseMotion>
</template>

<style scoped>
.question-fieldset { animation: question-arrival .2s var(--motion-ease); }
.question-choice { transition: background-color .15s ease, box-shadow .15s ease; }
.choice-check { animation: choice-confirm .18s var(--motion-ease); }
@keyframes question-arrival { from { opacity: .35; transform: translateX(5px); } }
@keyframes choice-confirm { from { opacity: 0; transform: scale(.75); } }
@media (prefers-reduced-motion: reduce) {
  .question-fieldset, .choice-check { animation: none; }
  .question-choice { transition: background-color .1s linear; }
}
.pending-interactions { width: 100%; max-width: 780px; max-height: min(440px, calc(var(--app-height, 100dvh) * .6)); min-height: 0; flex: 0 1 auto; margin: 0 auto; }
.pending-interactions:not(.has-pending) { margin-bottom: 0; }
.pending-interactions.keyboard-open.is-replying { max-height: none; }
.pending-interactions :deep(.collapse-clip) { min-height: 0; }
.pending-interactions > :deep(.collapse-clip) { display: flex; flex-direction: column; overflow: hidden; }
.pending-interaction { display: flex; flex-direction: column; min-width: 0; min-height: 0; max-height: 100%; padding: 4px 14px 14px; border: 1px solid var(--border); border-radius: 18px; background: var(--panel); overflow: hidden; }
.interaction-heading { display: flex; flex: none; align-items: center; gap: 8px; min-height: 44px; color: var(--text); }
.interaction-body { flex: 1; min-height: 0; }
.interaction-body > :deep(.collapse-clip) { display: flex; flex-direction: column; min-height: 0; }
.interaction-form { display: flex; flex-direction: column; flex: 1; min-height: 0; overflow: hidden; }
.interaction-scroll { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior-y: contain; }
.question-form { display: flex; flex-direction: column; flex: 1; overflow: hidden; }
.interaction-heading > svg { width: 17px; height: 17px; flex: none; color: var(--signal); }
.interaction-heading h3 { flex: 1; min-width: 0; font-size: 14px; line-height: 1.5; font-weight: 650; margin: 0; overflow-wrap: anywhere; }
.question-progress { color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; white-space: nowrap; }
.interaction-close, .interaction-previous { display: grid; place-items: center; width: 44px; height: 44px; padding: 0; border: 0; border-radius: 50%; background: none; color: var(--muted); flex: none; }
.interaction-close { margin-right: -10px; }
.interaction-close svg { width: 18px; height: 18px; }
.interaction-close:hover, .interaction-previous:hover { color: var(--text); }
.interaction-reopen { display: flex; align-items: center; gap: 9px; width: 100%; min-height: 44px; margin-bottom: -8px; padding: 4px 0; border: 0; background: none; color: var(--text); font-size: 13px; text-align: left; }
.interaction-reopen > span { flex: 1; }
.interaction-reopen > svg { width: 16px; height: 16px; color: var(--muted); }
.interaction-reopen > svg:first-child { color: var(--signal); }
.interaction-message { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 14px; line-height: 1.55; margin: 4px 0 10px; }
.interaction-detail { max-height: 180px; padding: 12px; margin: 8px 0; overflow-y: auto; white-space: pre-wrap; overflow-wrap: anywhere; background: var(--terminal); color: var(--terminal-text); border-radius: 10px; font-size: 12px; line-height: 1.6; }
fieldset { min-width: 0; padding: 0; margin: 12px 0 0; border: 0; }
.question-fieldset { display: flex; flex-direction: column; flex: 1; min-height: 0; margin-top: 2px; }
.question-scroll { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior-y: contain; }
.question-scroll:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.question-prompt { margin: 0 0 12px; font-size: 18px; font-weight: 600; line-height: 1.45; overflow-wrap: anywhere; }
legend:not(.sr-only) { display: block; width: 100%; padding: 0; margin-bottom: 10px; font-size: 14px; font-weight: 500; line-height: 1.55; overflow-wrap: anywhere; }
.question-options { display: grid; gap: 8px; }
.interaction-choice { position: relative; display: flex; align-items: center; gap: 9px; min-height: 44px; padding: 7px 8px; margin-inline: -8px; border-radius: 12px; cursor: pointer; font-size: 14px; line-height: 1.45; transition: background-color .12s ease; }
@media (hover: hover) { .interaction-choice:hover { background: var(--hover); } }
.interaction-choice input { flex: none; width: 17px; height: 17px; margin: 0; accent-color: var(--text); }
.question-choice input { position: absolute; width: 1px; height: 1px; opacity: 0; }
.question-choice:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: -2px; }
.choice-number { display: grid; place-items: center; width: 24px; height: 24px; flex: none; border-radius: 6px; background: var(--panel); color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; }
.choice-copy { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.interaction-choice strong { display: block; font-size: inherit; font-weight: 400; }
.interaction-choice small { display: block; margin-top: 4px; color: var(--muted); font-size: 12px; line-height: 1.5; }
.question-choice { gap: 12px; padding: 10px 12px; margin-inline: 0; min-height: 48px; border: 1px solid var(--border); background: var(--page); font-size: 16px; transition: background-color 160ms ease, transform 160ms var(--motion-ease); }
.question-choice:active:not(:has(input:disabled)) { transform: scale(.99); }
.choice-check { width: 15px; height: 15px; flex: none; color: var(--signal); }
textarea, select, input:not([type='radio']) { width: 100%; min-height: 44px; padding: 9px 11px; border: 1px solid var(--border); border-radius: 10px; background: var(--page); color: var(--text); font-family: inherit; font-size: 16px; line-height: 1.5; }
textarea { resize: none; max-height: 96px; }
textarea::placeholder, input::placeholder { color: var(--muted); }
.question-footer { display: flex; flex: none; flex-direction: column; align-items: stretch; gap: 8px; padding-top: 12px; background: var(--panel); }
.custom-answer { display: flex; flex: 1; min-width: 0; align-items: flex-start; gap: 8px; min-height: 48px; padding-inline: 12px; color: var(--muted); border: 1px solid var(--border); border-radius: 12px; background: var(--page); }
.custom-answer > svg { width: 18px; height: 18px; margin-top: 15px; flex: none; }
.custom-answer :is(textarea, input) { min-width: 0; min-height: 48px; max-height: clamp(48px, calc(var(--app-height, 100dvh) * .18), 96px); padding: 12px 0; border: 0; outline: none; border-radius: 0; background: transparent; }
.custom-answer:focus-within { outline: 2px solid var(--focus); outline-offset: 2px; }
.custom-answer.chosen { color: var(--text); }
.field-description, .field-optional { font-size: 12px; font-weight: 400; color: var(--muted); }
.field-description { line-height: 1.6; margin: 0 0 8px; }
.interaction-status { font-size: 12px; color: var(--muted); margin: 8px 0; }
.interaction-actions { display: flex; flex: none; justify-content: flex-end; align-items: center; gap: 8px; margin-top: 2px; }
.question-footer .interaction-actions { flex: none; width: 100%; margin: 0; }
.question-footer .interaction-skip { margin-right: auto; }
.interaction-previous { margin-right: auto; }
.interaction-submit, .interaction-decline, .interaction-skip, .interaction-desktop { min-height: 44px; padding: 9px 16px; border: 0; border-radius: 22px; font-size: 13px; font-weight: 500; }
.interaction-submit { display: inline-flex; align-items: center; justify-content: center; gap: 6px; background: var(--signal); color: var(--signal-ink); }
.interaction-submit svg { width: 15px; height: 15px; }
.interaction-decline, .interaction-desktop { background: var(--hover); color: var(--text); }
.interaction-skip { background: transparent; color: var(--muted); }
.interaction-skip:hover { background: var(--hover); }
.interaction-desktop { display: inline-flex; align-items: center; gap: 6px; }
.interaction-desktop svg { width: 13px; height: 13px; }
fieldset:disabled .interaction-choice { cursor: default; }
button:disabled { opacity: .45; cursor: default; }
:is(button, input, textarea, select):focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.custom-answer :is(textarea, input):focus-visible { outline: none; }
@media (max-height: 500px) { .pending-interaction { padding: 0 12px 8px; border-radius: 18px; } .interaction-heading { min-height: 36px; } }
@media (max-width: 360px) { .question-footer { flex-wrap: wrap; } .custom-answer { flex-basis: 100%; } .question-footer .interaction-actions { width: 100%; } }
@media (max-height: 500px) { .question-footer { flex-direction: row; flex-wrap: nowrap; align-items: flex-end; padding-top: 8px; } .question-footer .custom-answer { flex-basis: auto; } .question-footer .interaction-actions { width: auto; } .question-prompt { font-size: 15px; } }
@media (prefers-reduced-motion: reduce) { .interaction-choice { transition: none; } .question-choice:active { transform: none; } }
</style>
