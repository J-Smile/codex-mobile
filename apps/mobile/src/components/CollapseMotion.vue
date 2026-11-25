<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, watch } from 'vue';

const props = defineProps<{ open: boolean; focusTarget?: HTMLElement | null }>();
const emit = defineEmits<{ 'after-close': [] }>();
const container = ref<HTMLElement>();
const mounted = ref(props.open);
const expanded = ref(props.open);
let closeTimer: ReturnType<typeof setTimeout> | undefined;

function finishClose() {
  if (props.open) return;
  clearTimeout(closeTimer);
  mounted.value = false;
  emit('after-close');
}

watch(() => props.open, async (open) => {
  clearTimeout(closeTimer);
  if (open) {
    const wasMounted = mounted.value;
    mounted.value = true;
    await nextTick();
    if (!props.open) return;
    // Establish the closed grid before its first expansion. Reversals keep the current frame.
    if (!wasMounted) container.value?.getBoundingClientRect();
    expanded.value = true;
    return;
  }
  if (container.value?.contains(document.activeElement)) props.focusTarget?.focus({ preventScroll: true });
  const wasExpanded = expanded.value;
  expanded.value = false;
  if (!wasExpanded) { finishClose(); return; }
  // Empty content and background tabs may not produce transitionend.
  closeTimer = setTimeout(finishClose, 320);
});

function onTransitionEnd(event: TransitionEvent) {
  if (event.target !== container.value) return;
  if (event.propertyName === 'grid-template-rows'
    || (event.propertyName === 'opacity' && matchMedia('(prefers-reduced-motion: reduce)').matches)) finishClose();
}

onBeforeUnmount(() => clearTimeout(closeTimer));
</script>

<template>
  <div ref="container" class="collapse-motion" :class="{ 'collapse-expanded': expanded }" :inert="!open" :aria-hidden="!open || undefined" @transitionend="onTransitionEnd">
    <div class="collapse-clip"><slot v-if="mounted" /></div>
  </div>
</template>

<style scoped>
.collapse-motion { display: grid; grid-template-rows: 0fr; min-width: 0; opacity: 0; transition: grid-template-rows .3s cubic-bezier(0, 0, .2, 1), opacity .3s cubic-bezier(0, 0, .2, 1); }
.collapse-motion.collapse-expanded { grid-template-rows: 1fr; opacity: 1; }
.collapse-clip { min-height: 0; min-width: 0; overflow: hidden; }
@media (prefers-reduced-motion: reduce) {
  .collapse-motion, .collapse-motion.collapse-expanded { transition: opacity .1s linear; }
}
</style>
