<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref } from 'vue';
import { Close, Loading, ZoomIn, ZoomOut } from '@element-plus/icons-vue';
import { constrainImage, MAX_IMAGE_SCALE, zoomImage, type ImagePoint, type ImageTransform } from './imageZoom';

const selected = ref<{ src: string; alt: string } | null>(null);
const dialog = ref<HTMLDialogElement>();
const stage = ref<HTMLElement>();
const image = ref<HTMLImageElement>();
const status = ref<'loading' | 'ready' | 'error'>('loading');
const transform = ref<ImageTransform>({ scale: 1, x: 0, y: 0 });
const pointers = new Map<number, ImagePoint>();
let observer: ResizeObserver | undefined;
let resizeFrame = 0;

async function openImage(event: MouseEvent | KeyboardEvent) {
  const target = event.target;
  if (!(target instanceof HTMLImageElement) || target.getAttribute('role') !== 'button' || selected.value) return;
  event.preventDefault();
  target.focus({ preventScroll: true });
  status.value = 'loading';
  reset();
  selected.value = { src: target.currentSrc || target.src, alt: target.alt || '图片' };
  await nextTick();
  dialog.value?.showModal();
  if (stage.value) {
    observer = new ResizeObserver(() => {
      cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(constrain);
    });
    observer.observe(stage.value);
  }
}

function onImageKey(event: KeyboardEvent) {
  if (event.key === 'Enter' || event.key === ' ') void openImage(event);
}

function closed() {
  cancelAnimationFrame(resizeFrame);
  observer?.disconnect();
  pointers.clear();
  selected.value = null;
}

function reset() {
  transform.value = { scale: 1, x: 0, y: 0 };
}

function constrain() {
  if (!image.value || !stage.value) return;
  transform.value = constrainImage(transform.value,
    { width: image.value.offsetWidth, height: image.value.offsetHeight },
    { width: stage.value.clientWidth, height: stage.value.clientHeight });
}

function zoom(scale: number, from?: ImagePoint, to?: ImagePoint) {
  if (status.value !== 'ready') return;
  transform.value = zoomImage(transform.value, scale, from, to);
  constrain();
}

function relativePoint(point: ImagePoint): ImagePoint {
  const bounds = stage.value!.getBoundingClientRect();
  return { x: point.x - bounds.left - bounds.width / 2, y: point.y - bounds.top - bounds.height / 2 };
}

function pointerDown(event: PointerEvent) {
  if (status.value !== 'ready' || (event.pointerType === 'mouse' && event.button !== 0)) return;
  stage.value!.setPointerCapture(event.pointerId);
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
}

function pointerMove(event: PointerEvent) {
  if (!pointers.has(event.pointerId)) return;
  const before = [...pointers.values()].slice(0, 2);
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  const after = [...pointers.values()].slice(0, 2);
  if (before.length === 1) {
    transform.value = { ...transform.value, x: transform.value.x + after[0]!.x - before[0]!.x, y: transform.value.y + after[0]!.y - before[0]!.y };
    constrain();
  } else {
    const center = (points: ImagePoint[]) => relativePoint({ x: (points[0]!.x + points[1]!.x) / 2, y: (points[0]!.y + points[1]!.y) / 2 });
    const distance = (points: ImagePoint[]) => Math.hypot(points[0]!.x - points[1]!.x, points[0]!.y - points[1]!.y);
    zoom(transform.value.scale * distance(after) / Math.max(1, distance(before)), center(before), center(after));
  }
}

function pointerEnd(event: PointerEvent) {
  pointers.delete(event.pointerId);
}

function wheel(event: WheelEvent) {
  zoom(transform.value.scale * Math.exp(-event.deltaY * 0.002), relativePoint({ x: event.clientX, y: event.clientY }));
}

function panWithKeyboard(event: KeyboardEvent) {
  if (status.value !== 'ready' || transform.value.scale <= 1 || event.altKey || event.ctrlKey || event.metaKey) return;
  switch (event.key) {
    case 'ArrowLeft': transform.value.x += 64; break;
    case 'ArrowRight': transform.value.x -= 64; break;
    case 'ArrowUp': transform.value.y += 64; break;
    case 'ArrowDown': transform.value.y -= 64; break;
    default: return;
  }
  event.preventDefault();
  constrain();
}

onBeforeUnmount(() => {
  cancelAnimationFrame(resizeFrame);
  observer?.disconnect();
  dialog.value?.close();
});
</script>

<template>
  <div @click="openImage" @keydown="onImageKey">
    <slot />
    <Teleport to="body">
      <dialog v-if="selected" ref="dialog" class="image-preview" role="dialog" aria-label="图片预览" @close="closed" @keydown="panWithKeyboard">
        <header class="image-preview-toolbar">
          <div class="image-preview-controls" role="group" aria-label="缩放图片">
            <button type="button" class="icon-button" aria-label="缩小图片" :disabled="status !== 'ready' || transform.scale <= 1" @click="zoom(transform.scale / 1.5)"><ZoomOut /></button>
            <output class="image-preview-scale" aria-label="缩放比例">{{ Math.round(transform.scale * 100) }}%</output>
            <button type="button" class="icon-button" aria-label="放大图片" :disabled="status !== 'ready' || transform.scale >= MAX_IMAGE_SCALE" @click="zoom(transform.scale * 1.5)"><ZoomIn /></button>
          </div>
          <button type="button" class="text-button image-preview-reset" :disabled="status !== 'ready' || transform.scale === 1" @click="reset">还原</button>
          <button type="button" class="icon-button image-preview-close" aria-label="关闭图片预览" autofocus @click="dialog?.close()"><Close /></button>
        </header>
        <div ref="stage" class="image-preview-stage" :class="{ 'image-preview-zoomed': transform.scale > 1 }"
          @pointerdown.prevent="pointerDown" @pointermove="pointerMove" @pointerup="pointerEnd" @pointercancel="pointerEnd" @lostpointercapture="pointerEnd"
          @wheel.prevent="wheel" @dblclick.prevent="zoom(transform.scale > 1 ? 1 : 2)">
          <img ref="image" :src="selected.src" :alt="selected.alt" draggable="false"
            :style="{ visibility: status === 'ready' ? 'visible' : 'hidden', transform: `translate(${transform.x}px, ${transform.y}px) scale(${transform.scale})` }"
            @load="status = 'ready'" @error="status = 'error'" />
          <div v-if="status === 'loading'" class="image-preview-status" role="status"><Loading class="is-spin" />正在加载图片…</div>
          <p v-else-if="status === 'error'" class="image-preview-status error-text" role="alert">图片加载失败，请关闭后重试。</p>
        </div>
        <p class="image-preview-hint">双指缩放，拖动查看</p>
      </dialog>
    </Teleport>
  </div>
</template>

<style scoped>
.image-preview-toolbar {
  gap: 8px;
  padding: max(8px, var(--safe-top-visible)) max(clamp(12px, 4vw, 24px), var(--safe-right)) 8px max(clamp(12px, 4vw, 24px), var(--safe-left));
  background: var(--sidebar);
}
.image-preview-controls { display: flex; align-items: center; border-radius: 22px; background: var(--panel); }
.image-preview-controls .icon-button:hover:not(:disabled), .image-preview-close:hover { background: var(--hover); }
.image-preview-scale { min-width: 46px; font-weight: 600; color: var(--signal); }
.image-preview-reset { border-radius: 10px; }
.image-preview-reset:not(:disabled) { color: var(--signal); }
.image-preview-close { background: var(--panel); }
.image-preview-stage { background: var(--sidebar); }
.image-preview-status { padding: 10px 14px; border-radius: 12px; background: var(--page); text-align: center; overflow-wrap: anywhere; }
.image-preview-status.error-text { color: var(--error); }
.image-preview-hint { padding-top: 10px; border-top: 1px solid var(--border); background: var(--sidebar); }
@media (max-height: 450px) { .image-preview-hint { padding-top: 6px; padding-bottom: max(6px, var(--safe-bottom-visible)); } }
@media (prefers-reduced-motion: no-preference) {
  .image-preview[open] .image-preview-toolbar { animation: image-preview-ready 200ms ease-out; }
  @keyframes image-preview-ready { from { opacity: .35; } }
}
</style>
