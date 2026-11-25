<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { editorText } from './plainText';

const props = defineProps<{ modelValue: string; placeholder?: string; multiline?: boolean }>();
const emit = defineEmits<{
  'update:modelValue': [value: string];
  input: [event: InputEvent];
  paste: [event: ClipboardEvent];
}>();
const element = ref<HTMLDivElement>();
const empty = ref(!props.modelValue);
let composing = false;

function text() {
  const field = element.value;
  if (!field) return '';
  const value = editorText(field).replace(/\r\n?/g, '\n');
  return props.multiline ? value : value.replace(/\n/g, ' ');
}

function sync() {
  empty.value = !props.modelValue;
  if (element.value && !composing && text() !== props.modelValue) element.value.textContent = props.modelValue;
}

function onInput(event: Event) {
  empty.value = !text();
  if (composing || (event as InputEvent).isComposing) return;
  emit('update:modelValue', text());
  emit('input', event as InputEvent);
}

function onCompositionEnd(event: CompositionEvent) {
  composing = false;
  onInput(event);
}

function onPaste(event: ClipboardEvent) {
  emit('paste', event);
  if (event.defaultPrevented || props.multiline) return;
  const value = event.clipboardData?.getData('text/plain');
  if (value?.includes('\n') || value?.includes('\r')) {
    event.preventDefault();
    // Preserve the browser's selection and undo stack when flattening a search paste.
    document.execCommand('insertText', false, value.replace(/\r\n?|\n/g, ' '));
  }
}

function onKeydown(event: KeyboardEvent) {
  if (!props.multiline && event.key === 'Enter' && !event.isComposing && !composing) {
    event.preventDefault();
    element.value?.blur();
  }
}

function onBeforeInput(event: InputEvent) {
  if (!props.multiline && !event.isComposing && (event.inputType === 'insertParagraph' || event.inputType === 'insertLineBreak')) {
    event.preventDefault();
    element.value?.blur();
  }
}

function selectionStart() {
  const selection = window.getSelection();
  if (!element.value || !selection?.rangeCount || !element.value.contains(selection.focusNode)) return text().length;
  const range = document.createRange();
  range.selectNodeContents(element.value);
  range.setEnd(selection.focusNode!, selection.focusOffset);
  return editorText(range.cloneContents()).replace(/\r\n?/g, '\n').length;
}

watch(() => props.modelValue, sync, { flush: 'post' });
onMounted(sync);
defineExpose({ element, selectionStart, focus: (options?: FocusOptions) => element.value?.focus(options), blur: () => element.value?.blur() });
</script>

<template>
  <div ref="element" class="plain-text-input" contenteditable="plaintext-only" role="textbox" :aria-multiline="!!multiline" :aria-placeholder="placeholder" :data-placeholder="placeholder" :data-empty="empty" :enterkeyhint="multiline ? 'enter' : 'search'" spellcheck="true" @beforeinput="onBeforeInput" @input="onInput" @compositionstart="composing = true" @compositionend="onCompositionEnd" @paste="onPaste" @keydown="onKeydown"></div>
</template>

<style scoped>
.plain-text-input { position: relative; min-width: 0; white-space: pre-wrap; overflow-wrap: anywhere; -webkit-user-select: text; user-select: text; caret-color: var(--text); }
.plain-text-input[data-empty="true"]::before { content: attr(data-placeholder); position: absolute; color: var(--muted); pointer-events: none; }
</style>
