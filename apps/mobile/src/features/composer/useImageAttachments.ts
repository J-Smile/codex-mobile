import { computed, onScopeDispose, reactive, ref, watch } from 'vue';
import { ApiError, request } from '../../api.ts';
import { desktopStorageKey } from '../../transport.ts';

export interface UploadedImage {
  id: string;
  name: string;
  url: string;
  size: number;
  type: string;
}

export interface DraftImage extends UploadedImage {
  status: 'uploading' | 'ready' | 'error';
  error?: string;
  previewUrl?: string;
  unavailable?: boolean;
}

const storageKey = 'codex-mobile-image-drafts';
const maxImageBytes = 20 * 1024 * 1024;
const supportedTypes = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);

function readDrafts(): Record<string, DraftImage[]> {
  try {
    const saved = JSON.parse(localStorage.getItem(desktopStorageKey(storageKey)) || '{}');
    if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {};
    return Object.fromEntries(Object.entries(saved).flatMap(([key, value]) => {
      if (!Array.isArray(value)) return [];
      const images = value.filter((image): image is UploadedImage => image && typeof image.id === 'string'
        && /^[a-f0-9]{32}\.(png|jpg|webp|gif)$/.test(image.id) && image.url === '/api/images/' + image.id
        && typeof image.name === 'string' && typeof image.size === 'number' && supportedTypes.has(image.type));
      return images.length ? [[key, images.map(image => {
        const { id, name, url, size, type } = image;
        const unavailable = (image as DraftImage).unavailable === true;
        return { id, name, url, size, type, unavailable, status: unavailable ? 'error' as const : 'ready' as const };
      })]] : [];
    }));
  } catch { return {}; }
}

export function useImageAttachments(options: { draftKey: () => string }) {
  const scopedStorageKey = desktopStorageKey(storageKey);
  const drafts = reactive(readDrafts());
  const error = ref('');
  const controllers = new Map<string, AbortController>();
  const files = new Map<string, File>();
  let disposed = false;
  const images = computed(() => drafts[options.draftKey()] || []);
  const uploading = computed(() => images.value.some(image => image.status === 'uploading'));
  const hasPendingUploads = computed(() => Object.values(drafts).some(values => values.some(image => image.status === 'uploading')));
  const hasErrors = computed(() => images.value.some(image => image.status === 'error'));
  const canReload = computed(() => !hasPendingUploads.value
    && !Object.values(drafts).some(values => values.some(image => image.status === 'error' && !image.unavailable)));

  function saveDrafts() {
    const saved = Object.fromEntries(Object.entries(drafts).flatMap(([key, values]) => {
      const ready = values.filter(image => image.status === 'ready' || image.unavailable)
        .map(({ id, name, url, size, type, unavailable }) => ({ id, name, url, size, type, ...(unavailable ? { unavailable } : {}) }));
      return ready.length ? [[key, ready]] : [];
    }));
    try { localStorage.setItem(scopedStorageKey, JSON.stringify(saved)); return true; }
    catch { error.value = '图片已上传，但浏览器未能保存图片草稿。'; return false; }
  }
  watch(drafts, saveDrafts, { deep: true, flush: 'sync' });

  async function upload(key: string, entry: DraftImage, file: File) {
    if (disposed) return;
    const id = entry.id;
    const controller = new AbortController();
    controllers.set(id, controller);
    files.set(id, file);
    entry.status = 'uploading';
    entry.error = '';
    try {
      const uploaded = await request<UploadedImage>('/api/images', {
        method: 'POST', body: file, signal: controller.signal, timeoutMs: 60_000,
        headers: { 'Content-Type': file.type || 'application/octet-stream', 'X-File-Name': encodeURIComponent(file.name) },
      });
      if (controller.signal.aborted || !drafts[key]?.includes(entry)) return;
      // 弹层可能已打开本地预览，上传成功后保留该地址，直到移除草稿或卸载。
      Object.assign(entry, uploaded, { status: 'ready' });
      files.delete(id);
    } catch (failure) {
      if (controller.signal.aborted || !drafts[key]?.includes(entry)) return;
      entry.status = 'error';
      entry.error = (failure as Error).message;
    } finally { if (controllers.get(id) === controller) controllers.delete(id); }
  }

  async function add(selected: Iterable<File>) {
    const key = options.draftKey();
    if (disposed || !key) return;
    error.value = '';
    const additions: { entry: DraftImage; file: File }[] = [];
    for (const file of selected) {
      drafts[key] ||= [];
      const current = drafts[key]!;
      if (current.length >= 20) { error.value = '每条消息最多添加 20 张图片。'; break; }
      if (file.size > maxImageBytes) { error.value = '每张图片不能超过 20 MB。'; continue; }
      if (file.type && !supportedTypes.has(file.type)) { error.value = '请选择 PNG、JPEG、WebP 或 GIF 图片。'; continue; }
      current.push({ id: 'upload-' + crypto.randomUUID(), name: file.name, size: file.size, type: file.type,
        url: '', previewUrl: URL.createObjectURL(file), status: 'uploading' });
      additions.push({ entry: current[current.length - 1]!, file });
    }
    // 手机相册常含大图，逐张上传避免同时占满连接和服务端缓冲。
    for (const { entry, file } of additions) if (drafts[key]?.includes(entry)) await upload(key, entry, file);
  }

  function remove(id: string, key = options.draftKey()) {
    const entry = drafts[key]?.find(image => image.id === id);
    if (!entry) return;
    controllers.get(id)?.abort();
    controllers.delete(id);
    files.delete(id);
    if (entry.previewUrl) URL.revokeObjectURL(entry.previewUrl);
    drafts[key] = drafts[key]!.filter(image => image.id !== id);
    if (!drafts[key]!.length) delete drafts[key];
  }

  function clear(key = options.draftKey(), ids?: string[]) {
    for (const entry of [...drafts[key] || []]) if (!ids || ids.includes(entry.id)) remove(entry.id, key);
  }

  function clearAll() {
    for (const key of Object.keys(drafts)) clear(key);
    error.value = '';
  }

  function moveDraft(from: string, to: string) {
    if (drafts[to]?.length || drafts[from]?.some(entry => entry.status === 'uploading' || entry.status === 'error' && !entry.unavailable)) return false;
    if (drafts[from]?.length) {
      drafts[to] = drafts[from];
      delete drafts[from];
    }
    return true;
  }

  async function retry(id: string) {
    const key = options.draftKey();
    const entry = drafts[key]?.find(image => image.id === id);
    const file = files.get(id);
    if (entry?.status === 'error' && file) await upload(key, entry, file);
  }

  async function checkPreview(id: string, key = options.draftKey()) {
    const entry = drafts[key]?.find(image => image.id === id);
    if (disposed || !entry || entry.status !== 'ready' || controllers.has(id)) return;
    const controller = new AbortController();
    controllers.set(id, controller);
    try {
      await request(entry.url, { method: 'HEAD', signal: controller.signal }, async () => undefined);
    } catch (failure) {
      if (controller.signal.aborted || !drafts[key]?.includes(entry)) return;
      if (failure instanceof ApiError && failure.status === 404) {
        entry.unavailable = true;
        entry.status = 'error';
        entry.error = '电脑上的图片已失效，请移除后重新选择。';
      }
    } finally { if (controllers.get(id) === controller) controllers.delete(id); }
  }

  function readyImages(key = options.draftKey()): UploadedImage[] {
    return (drafts[key] || []).filter(image => image.status === 'ready')
      .map(({ id, name, url, size, type }) => ({ id, name, url, size, type }));
  }

  onScopeDispose(() => {
    disposed = true;
    for (const controller of controllers.values()) controller.abort();
    for (const values of Object.values(drafts)) for (const entry of values) if (entry.previewUrl) URL.revokeObjectURL(entry.previewUrl);
    controllers.clear();
    files.clear();
  });

  return { images, uploading, hasPendingUploads, hasErrors, canReload, saveDrafts, error, add, remove, clear, clearAll, moveDraft, retry, readyImages, checkPreview };
}
