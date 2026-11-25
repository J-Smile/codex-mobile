<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue';
import type QrScanner from 'qr-scanner';
import { Camera, Close, Loading, Picture, Refresh } from '@element-plus/icons-vue';
import { parseConnectionKey } from './connectionLink';
import { readPairingInvitation, type PairingInvitation, type PairingTarget } from '../relay/devices';

const props = defineProps<{ relay?: boolean; login?: boolean; target?: PairingTarget }>();
const emit = defineEmits<{ close: []; connect: [key: string]; pair: [invitation: PairingInvitation] }>();
const dialog = ref<HTMLDialogElement>();
const video = ref<HTMLVideoElement>();
const imageInput = ref<HTMLInputElement>();
const cameraIssue = ref('');
const scanIssue = ref('');
const starting = ref(false);
const cameraReady = ref(false);
const readingImage = ref(false);
const canUseCamera = window.isSecureContext && !!navigator.mediaDevices?.getUserMedia;
let accepted = false;
let decoding = false;
let scanner: QrScanner | undefined;
let disposed = false;

async function decoded(value: string) {
  if (disposed || accepted || decoding) return;
  decoding = true;
  try {
    const credential = props.relay ? await readPairingInvitation(value, props.target) : parseConnectionKey(value);
    if (disposed) return;
    accepted = true;
    scanIssue.value = '';
    scanner?.stop();
    cameraReady.value = false;
    if (typeof credential === 'string') emit('connect', credential);
    else emit('pair', credential);
  } catch (failure) {
    if (!disposed) scanIssue.value = (failure as Error).message;
  } finally {
    decoding = false;
  }
}

async function startCamera() {
  if (starting.value || disposed) return;
  if (!canUseCamera) {
    cameraIssue.value = window.isSecureContext ? '当前浏览器无法使用相机。' : '网页需要 HTTPS 地址才能打开相机。';
    return;
  }
  starting.value = true;
  cameraIssue.value = '';
  scanIssue.value = '';
  try {
    const Scanner = (await import('qr-scanner')).default;
    if (disposed || !video.value) return;
    scanner ||= new Scanner(video.value, (result) => {
      if (!readingImage.value) void decoded(result.data);
    }, {
      preferredCamera: 'environment', maxScansPerSecond: 5, returnDetailedScanResult: true,
    });
    await scanner.start();
    if (disposed || accepted) return;
    if (document.hidden) pauseCamera();
    else cameraReady.value = true;
  } catch {
    if (disposed || accepted) return;
    cameraIssue.value = '相机暂时不可用，请检查网页的相机权限。';
  } finally { starting.value = false; }
}

async function readImage(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file || readingImage.value) return;
  readingImage.value = true;
  scanIssue.value = '';
  try {
    const Scanner = (await import('qr-scanner')).default;
    const result = await Scanner.scanImage(file, { returnDetailedScanResult: true });
    await decoded(result.data);
  } catch { if (!disposed) scanIssue.value = '未识别到连接二维码，请选择完整、清晰的二维码图片。'; }
  finally { readingImage.value = false; input.value = ''; }
}

function pauseCamera() {
  scanner?.stop();
  cameraReady.value = false;
  cameraIssue.value = '扫描已暂停，点击重试继续。';
}

function onVisibility() {
  if (document.hidden && cameraReady.value) pauseCamera();
}

onMounted(() => {
  dialog.value?.showModal();
  void startCamera();
  document.addEventListener('visibilitychange', onVisibility);
});
onBeforeUnmount(() => {
  disposed = true;
  scanner?.destroy();
  dialog.value?.close();
  document.removeEventListener('visibilitychange', onVisibility);
});
</script>

<template>
  <dialog ref="dialog" class="scanner-backdrop" aria-labelledby="scan-title" aria-describedby="scan-description"
    @click.self="emit('close')" @cancel.prevent="emit('close')">
    <section class="scanner-sheet">
      <div class="modal-heading scanner-heading">
        <h2 id="scan-title">{{ login ? '扫码登录' : '扫码连接' }}</h2>
        <button class="icon-button scanner-close" type="button" aria-label="关闭扫码" autofocus @click="emit('close')"><Close /></button>
      </div>
      <p v-if="target" id="scan-description" class="scanner-intro">扫描「{{ target.name }}」上通过「手机扫码连接」显示的二维码。</p>
      <p v-else id="scan-description" class="scanner-intro">{{ relay ? '在电脑端点「手机扫码连接」，扫描显示的二维码。' : '扫描桌面程序中显示的局域网二维码。' }}</p>
      <div class="scanner-preview" :class="{ 'is-ready': cameraReady }" aria-hidden="true">
        <video ref="video" playsinline muted></video>
        <Camera v-if="!cameraReady" class="scanner-camera-symbol" />
        <div class="scanner-reticle"><span></span><span></span><span></span><span></span></div>
      </div>
      <div class="scanner-controls">
        <p v-if="readingImage || starting" class="scanner-status" role="status">
          <Loading class="is-spin" aria-hidden="true" />{{ readingImage ? '正在识别二维码…' : '正在打开相机…' }}
        </p>
        <p v-else-if="scanIssue || cameraIssue" class="scanner-status error-text" role="alert">{{ scanIssue || cameraIssue }}</p>
        <p v-else class="scanner-status" role="status">将二维码放入取景框</p>
        <div class="scanner-actions">
          <button class="secondary-button scanner-album" :class="{ 'is-fallback': !cameraReady }" type="button"
            :disabled="readingImage" @click="imageInput?.click()"><Picture aria-hidden="true" />{{ readingImage ? '识别中…' : '从相册选择' }}</button>
          <button v-if="canUseCamera && !cameraReady && !starting" class="secondary-button" type="button"
            :disabled="readingImage" @click="startCamera"><Refresh aria-hidden="true" />重试相机</button>
        </div>
      </div>
      <input ref="imageInput" type="file" accept="image/*" class="hidden-input" aria-label="选择二维码图片" @change="readImage" />
    </section>
  </dialog>
</template>

<style scoped>
.scanner-backdrop {
  position: fixed; inset: var(--viewport-top, 0px) 0 auto; width: 100%; height: var(--app-height, 100dvh); max-width: none; max-height: none;
  margin: 0; padding: max(12px, var(--safe-top-visible)) max(12px, var(--safe-right)) max(12px, var(--safe-bottom-visible)) max(12px, var(--safe-left));
  border: 0; overflow: hidden; color: var(--text); background: rgb(0 0 0 / .55);
}
.scanner-backdrop[open] { display: grid; place-items: center; }
.scanner-backdrop::backdrop { background: transparent; }
.scanner-sheet {
  display: grid; align-content: start; grid-template: 'heading' 'intro' 'camera' 'controls' / minmax(0, 1fr);
  gap: 12px; width: min(100%, 420px); max-height: 100%; padding: 16px;
  overflow-y: auto; overscroll-behavior: contain; border: 1px solid var(--border); border-radius: 23px;
  background: var(--page); box-shadow: var(--sheet-shadow);
}
.scanner-heading { grid-area: heading; }
.scanner-heading h2 { font-size: 20px; }
.scanner-close { background: var(--panel); }
.scanner-close svg { width: 16px; height: 16px; }
.scanner-intro { grid-area: intro; margin: 0; color: var(--muted); font-size: 14px; line-height: 1.55; overflow-wrap: anywhere; }
.scanner-preview {
  grid-area: camera; justify-self: center; align-self: start; position: relative; isolation: isolate;
  display: grid; place-items: center; width: min(100%, 280px, max(148px, calc(var(--app-height, 100dvh) - 262px))); aspect-ratio: 1;
  overflow: hidden; border: 1px solid #404040; border-radius: 16px; background: #191919; color: #e5e5e5;
}
.scanner-preview video { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; opacity: 0; }
.scanner-preview.is-ready video { opacity: 1; }
.scanner-reticle {
  grid-area: 1 / 1; z-index: 1; display: grid; grid-template: repeat(2, 1fr) / repeat(2, 1fr);
  width: 66%; height: 66%; pointer-events: none;
}
.scanner-reticle span { width: 22px; height: 22px; border: solid currentColor; border-width: 2px 0 0 2px; border-start-start-radius: 9px; }
.scanner-reticle span:nth-child(2) { justify-self: end; transform: rotate(90deg); }
.scanner-reticle span:nth-child(3) { align-self: end; transform: rotate(-90deg); }
.scanner-reticle span:nth-child(4) { align-self: end; justify-self: end; transform: rotate(180deg); }
.scanner-camera-symbol { grid-area: 1 / 1; width: 28px; height: 28px; }
.scanner-controls { grid-area: controls; display: grid; align-content: start; gap: 9px; min-width: 0; }
.scanner-status { display: flex; align-items: center; gap: 7px; min-height: 21px; margin: 0; color: var(--muted); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
.scanner-status.error-text { color: var(--error); }
.scanner-status svg { width: 14px; height: 14px; }
.scanner-actions { display: flex; flex-wrap: wrap; gap: 8px; }
.scanner-actions button { flex: 1; min-height: 44px; padding: 8px 10px; font-size: 14px; white-space: nowrap; }
.scanner-actions button svg { width: 16px; height: 16px; }
.scanner-album.is-fallback { border-color: transparent; background: var(--signal); color: var(--signal-ink); }
@media (min-width: 560px) and (max-height: 500px) {
  .scanner-sheet {
    width: min(100%, 620px); gap: 10px 16px;
    grid-template: 'heading heading' auto 'camera intro' auto 'camera controls' 1fr / minmax(0, 1fr) minmax(0, 1.25fr);
  }
  .scanner-preview { width: min(100%, 230px, max(132px, calc(var(--app-height, 100dvh) - 116px))); }
}
@media (prefers-reduced-motion: no-preference) {
  .scanner-sheet { animation: scanner-arrive 250ms var(--motion-ease) both; }
  @keyframes scanner-arrive { from { opacity: .45; transform: translateY(8px); } }
}
</style>
