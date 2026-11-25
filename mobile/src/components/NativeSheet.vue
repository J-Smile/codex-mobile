<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { animateMini, spring, type AnimationPlaybackControls } from 'motion-v';

defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{
  open: boolean;
  labelledby: string;
  panelClass?: string;
  dismissible?: boolean;
}>(), { panelClass: '', dismissible: true });
const emit = defineEmits<{ close: []; 'after-close': [] }>();
const dialog = ref<HTMLDialogElement>();
const panel = ref<HTMLElement>();
const shade = ref<HTMLElement>();
const present = ref(false);
const ready = ref(false);
const dragging = ref(false);
const reduced = matchMedia('(prefers-reduced-motion: reduce)');
let restingWidth = window.visualViewport?.width || window.innerWidth;
let restingHeight = Math.max(window.innerHeight, document.documentElement.clientHeight);
let animations: AnimationPlaybackControls[] = [];
let transition = 0;
let suppressHandleClick = false;
let pressedBackdrop = false;
let previousFocus: HTMLElement | null = null;
let drag: {
  id: number; target: HTMLElement; startX: number; startY: number; offset: number; height: number; shadeOpacity: number;
  lastY: number; lastTime: number; velocity: number; distance: number; horizontalDistance: number;
} | undefined;

function rememberViewport() {
  if (props.open) return;
  const viewport = window.visualViewport;
  const width = viewport?.width || window.innerWidth;
  const height = Math.max(window.innerHeight, document.documentElement.clientHeight, viewport?.height || 0);
  // resizes-content can shrink both innerHeight and the layout viewport with
  // the keyboard. Keep the closed sheet's height reference until width changes.
  restingHeight = width === restingWidth ? Math.max(restingHeight, height) : height;
  restingWidth = width;
}

function stopAnimation() {
  transition++;
  for (const animation of animations) animation.stop();
  animations = [];
}

function finishClose() {
  if (props.open) return;
  dialog.value?.close();
  present.value = false;
  ready.value = false;
  // WebKit can leave focus on the body when the departing panel becomes inert.
  if (previousFocus?.isConnected && previousFocus.getClientRects().length) previousFocus.focus({ preventScroll: true });
  previousFocus = null;
  emit('after-close');
}

function animateSheet(open: boolean, openingShade?: AnimationPlaybackControls) {
  const surface = panel.value, scrim = shade.value;
  if (!surface || !scrim) return Promise.resolve(false);
  if (!openingShade) stopAnimation();
  const version = transition;
  const reduceMotion = reduced.matches;
  let movement: AnimationPlaybackControls;
  if (reduceMotion) {
    surface.style.transform = 'none';
    movement = animateMini(surface, { opacity: [openingShade ? 0 : Number(surface.style.opacity || 1), open ? 1 : 0] },
      { duration: .1, ease: 'easeOut' });
  } else {
    // 首次按自身高度移动，反向或回弹从 Motion 停下的位置接续。
    const away = 'translateY(calc(100% + 24px))';
    const start = openingShade ? away : surface.style.transform || 'none';
    surface.style.opacity = '1';
    movement = animateMini(surface, { transform: [start, open ? 'none' : away] },
      open ? { type: spring, duration: .3, bounce: 0 } : { duration: .2, ease: 'easeIn' });
  }
  const backdrop = openingShade ?? animateMini(scrim, { opacity: [Number(scrim.style.opacity || 1), open ? 1 : 0] },
    { duration: reduceMotion ? .1 : open ? .15 : .2, ease: 'easeOut' });
  animations = [movement, backdrop];
  return Promise.all(animations.map(animation => animation.finished)).then(() => {
    if (version !== transition) return false;
    animations = [];
    if (!open) finishClose();
    return true;
  });
}

function releasePointer() {
  const previous = drag;
  drag = undefined;
  dragging.value = false;
  if (previous?.target.hasPointerCapture(previous.id)) previous.target.releasePointerCapture(previous.id);
}

function prepareLayout() {
  const viewport = window.visualViewport;
  const keyboardVisible = () => {
    const width = viewport?.width || window.innerWidth;
    const height = Math.max(window.innerHeight, document.documentElement.clientHeight, width === restingWidth ? restingHeight : 0);
    return viewport && height - viewport.height > 100;
  };
  let frame = 0;
  let quiet: ReturnType<typeof setTimeout> | undefined;
  let resolve!: () => void;
  const finished = new Promise<void>(done => { resolve = done; });
  const clear = () => {
    clearTimeout(quiet);
    viewport?.removeEventListener('resize', changed);
    viewport?.removeEventListener('scroll', changed);
    window.removeEventListener('resize', changed);
  };
  const finish = () => {
    clear();
    frame = requestAnimationFrame(() => { frame = 0; resolve(); });
  };
  const changed = () => {
    clearTimeout(quiet);
    quiet = setTimeout(finish, keyboardVisible() ? 1000 : 120);
  };
  // showModal moves focus and dismisses the old keyboard. Its resize must finish
  // before the bottom edge becomes the origin of the entry animation.
  if (keyboardVisible()) {
    viewport?.addEventListener('resize', changed);
    viewport?.addEventListener('scroll', changed);
    window.addEventListener('resize', changed);
    changed();
  } else finish();
  return { finished, cancel() { clear(); cancelAnimationFrame(frame); resolve(); } };
}

watch(() => props.open, async (open, _, cleanup) => {
  let cancelled = false;
  let cancelPreparation: (() => void) | undefined;
  cleanup(() => { cancelled = true; cancelPreparation?.(); });
  releasePointer();
  if (open) {
    present.value = true;
    await nextTick();
    if (cancelled || !props.open || !dialog.value) return;
    let openingShade: AnimationPlaybackControls | undefined;
    if (!dialog.value.open) {
      ready.value = false;
      const layout = prepareLayout();
      cancelPreparation = layout.cancel;
      previousFocus = document.activeElement as HTMLElement | null;
      dialog.value.showModal();
      openingShade = animateMini(shade.value!, { opacity: [0, 1] }, { duration: reduced.matches ? .1 : .15, ease: 'easeOut' });
      animations = [openingShade];
      await layout.finished;
      if (cancelled || !props.open || !dialog.value?.open) return;
    }
    const entering = animateSheet(true, openingShade);
    ready.value = true;
    if (!(await entering) || cancelled || !props.open || !dialog.value?.open) return;
    await nextTick();
    // 面板落位后再聚焦，避免 WebKit 滚动仍在移动的 dialog，并保留用户已点选的输入框。
    if (cancelled || !props.open || panel.value?.contains(document.activeElement)) return;
    const target = panel.value?.querySelector<HTMLElement>('[autofocus]:not(:disabled)')
      || panel.value?.querySelector<HTMLElement>('.native-sheet-handle:not(:disabled)');
    target?.focus({ preventScroll: true });
  } else if (dialog.value?.open) {
    if (ready.value) animateSheet(false);
    else { stopAnimation(); finishClose(); }
  }
  else present.value = false;
}, { immediate: true, flush: 'post' });

function requestClose() {
  if (props.open && props.dismissible) emit('close');
}

function nativeClosed() {
  // A queued close event must not close a sheet that has already reopened.
  if (dialog.value?.open) return;
  if (props.open) emit('close');
}

function beginDrag(event: PointerEvent) {
  if (!props.open || !props.dismissible || !event.isPrimary || event.button !== 0 || !panel.value || !shade.value) return;
  suppressHandleClick = false;
  stopAnimation();
  const matrix = new DOMMatrixReadOnly(getComputedStyle(panel.value).transform);
  const target = event.currentTarget as HTMLElement;
  drag = { id: event.pointerId, target, startX: event.clientX, startY: event.clientY, offset: Math.max(0, matrix.m42),
    height: Math.max(1, panel.value.offsetHeight), shadeOpacity: Number(shade.value.style.opacity || 1),
    lastY: event.clientY, lastTime: event.timeStamp, velocity: 0, distance: 0, horizontalDistance: 0 };
  target.setPointerCapture(event.pointerId);
  dragging.value = true;
}

function moveDrag(event: PointerEvent) {
  if (!drag || drag.id !== event.pointerId || !panel.value || !shade.value) return;
  const elapsed = event.timeStamp - drag.lastTime;
  if (elapsed > 0) drag.velocity = (event.clientY - drag.lastY) / elapsed;
  drag.lastY = event.clientY;
  drag.lastTime = event.timeStamp;
  drag.distance = Math.max(0, event.clientY - drag.startY);
  drag.horizontalDistance = Math.abs(event.clientX - drag.startX);
  if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) > 5) suppressHandleClick = true;
  const distance = Math.max(0, drag.offset + event.clientY - drag.startY);
  panel.value.style.transform = `translate3d(0, ${distance}px, 0)`;
  shade.value.style.opacity = String(Math.max(0, Math.min(1, drag.shadeOpacity - (distance - drag.offset) / drag.height)));
}

function endDrag(event: PointerEvent, cancelled = false) {
  if (!drag || drag.id !== event.pointerId) return;
  const { distance, horizontalDistance, velocity, lastTime } = drag;
  const threshold = Math.min(160, Math.max(64, drag.height * .22));
  const flick = event.timeStamp - lastTime < 100 && distance > 28 && velocity > .65;
  const dismiss = !cancelled && props.dismissible && distance > horizontalDistance && (distance >= threshold || flick);
  releasePointer();
  if (dismiss) requestClose();
  else if (props.open) animateSheet(true);
}

function handleClick(event: MouseEvent) {
  if (!suppressHandleClick || event.detail === 0) requestClose();
  suppressHandleClick = false;
}

function backdropClick() {
  if (pressedBackdrop) requestClose();
  pressedBackdrop = false;
}

onMounted(() => {
  rememberViewport();
  window.addEventListener('resize', rememberViewport);
  window.visualViewport?.addEventListener('resize', rememberViewport);
});

onBeforeUnmount(() => {
  window.removeEventListener('resize', rememberViewport);
  window.visualViewport?.removeEventListener('resize', rememberViewport);
  releasePointer();
  stopAnimation();
  dialog.value?.close();
});
</script>

<template>
  <Teleport to="body">
    <dialog ref="dialog" v-bind="$attrs" class="native-sheet-layer" :class="{ 'sheet-ready': ready }" :aria-labelledby="labelledby" @cancel.prevent="requestClose" @close="nativeClosed">
      <template v-if="present">
        <div ref="shade" class="native-sheet-shade" aria-hidden="true" @pointerdown.self="pressedBackdrop = true" @pointercancel="pressedBackdrop = false" @click.self="backdropClick"></div>
        <section ref="panel" :class="['native-sheet-panel', { dragging }]" :inert="!open">
          <button type="button" class="native-sheet-handle" aria-label="关闭面板" :disabled="!dismissible" @pointerdown="beginDrag" @pointermove="moveDrag" @pointerup="endDrag($event)" @pointercancel="endDrag($event, true)" @lostpointercapture="endDrag($event, true)" @click="handleClick"><span aria-hidden="true"></span></button>
          <div :class="['native-sheet-content', panelClass]"><slot /></div>
        </section>
      </template>
    </dialog>
  </Teleport>
</template>

<style scoped>
.native-sheet-layer { position: fixed; inset: var(--viewport-top, 0px) 0 auto; width: 100%; height: var(--app-height, 100dvh); max-width: none; max-height: none; margin: 0; padding: 0; border: 0; overflow: clip; color: var(--text); background: transparent; }
.native-sheet-layer[open] { display: block; }
.native-sheet-layer:not(.sheet-ready) > .native-sheet-panel { visibility: hidden; }
.native-sheet-layer::backdrop { background: transparent; }
.native-sheet-shade { position: absolute; inset: 0; background: rgb(0 0 0 / .46); }
.native-sheet-panel {
  --sheet-inset: clamp(16px, 5vw, 24px);
  position: absolute; inset: auto var(--safe-right) 0 var(--safe-left);
  display: flex; flex-direction: column;
  width: min(var(--sheet-width, 520px), calc(100% - var(--safe-left) - var(--safe-right)));
  max-height: calc(100% - 12px - var(--safe-top-visible)); min-height: 0;
  margin-inline: auto; overflow: hidden;
  border: 1px solid var(--border); border-bottom: 0; border-radius: 26px 26px 0 0;
  background: var(--sidebar); box-shadow: var(--sheet-shadow);
}
.native-sheet-content { min-height: 0; width: 100%; max-width: none; max-height: none; overflow-y: auto; overscroll-behavior: contain; border: 0; border-radius: 0; background: transparent; box-shadow: none; -webkit-backdrop-filter: none; backdrop-filter: none; animation: none; }
.native-sheet-content.settings-modal { padding: 0 var(--sheet-inset) max(24px, var(--safe-bottom-visible)); }
.native-sheet-content :deep(.modal-heading) {
  position: sticky; inset-block-start: 0; z-index: 1;
  padding-block: 0 8px; margin-block-end: 8px; border-bottom: 1px solid var(--border);
  background: var(--sidebar);
}
.native-sheet-content :deep(.modal-heading h2) { min-width: 0; font-size: 21px; font-weight: 650; line-height: 1.35; overflow-wrap: anywhere; }
.native-sheet-content :deep(.modal-heading > .icon-button) { background: var(--panel); color: var(--muted); outline-offset: -3px; }
.native-sheet-content :deep(.modal-heading > .icon-button:hover:not(:disabled)) { background: var(--hover); color: var(--text); }
.native-sheet-handle { display: grid; place-items: center; align-self: center; flex: none; width: 88px; height: 44px; min-height: 44px; padding: 0; border: 0; border-radius: 18px; color: var(--muted); background: transparent; touch-action: none; cursor: grab; }
.native-sheet-handle > span { width: 34px; height: 4px; border-radius: 3px; background: currentColor; opacity: .45; }
.native-sheet-handle:focus-visible { outline: 2px solid var(--focus); outline-offset: -4px; }
.native-sheet-handle:disabled { opacity: .4; }
.dragging .native-sheet-handle { cursor: grabbing; }
@media (max-height: 450px) {
  .native-sheet-content :deep(.modal-heading) { position: static; }
  .native-sheet-content.settings-modal { padding-bottom: max(16px, var(--safe-bottom-visible)); }
}
</style>
