import { onScopeDispose, ref, watch } from 'vue';
import { request } from '../../api';
import { relayAccount, relayDesktop } from '../../transport';

function applicationKey(value: string) {
  const decoded = atob(value.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0)).buffer as ArrayBuffer;
}

export function useNotifications(authenticated: () => boolean, openThread: (threadId: string) => void) {
  const desktop = relayDesktop();
  const account = relayAccount();
  const supported = window.isSecureContext && 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;
  const enabled = ref(false);
  const busy = ref(false);
  const error = ref('');
  const notice = ref('');
  const requestedThread = ref('');
  function onNotification(event: MessageEvent) {
    if (event.data?.type !== 'codex-open-thread' || typeof event.data.threadId !== 'string' || !event.data.threadId) return;
    if (desktop && (event.data.desktop !== desktop || event.data.account !== account)) return;
    requestedThread.value = event.data.threadId;
    event.ports?.[0]?.postMessage(true);
  }
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.addEventListener('message', onNotification);
    onScopeDispose(() => navigator.serviceWorker.removeEventListener('message', onNotification));
  }
  // 登录初始化先恢复 URL，再消费通知，避免旧地址覆盖刚收到的目标对话。
  watch([authenticated, requestedThread], ([connected, threadId]) => {
    if (!connected || !threadId) return;
    openThread(threadId);
    requestedThread.value = '';
  }, { flush: 'post' });
  const hint = supported ? '所有本机 Codex 对话的任务完成或失败时，发送系统通知，自动包含新对话。' : '请在 Safari 中添加到主屏幕，再从网页 App 开启通知（iOS 16.4 或更新版本）。';

  async function registration(create = false) {
    const scope = desktop ? '/relay-push/' + account + '/' + desktop + '/' : undefined;
    let value = await navigator.serviceWorker.getRegistration(scope);
    if (scope && value?.scope !== new URL(scope, location.origin).href) {
      if (!create) return;
      value = await navigator.serviceWorker.register('/sw.js', { scope });
    }
    const worker = value?.installing || value?.waiting;
    if (create && !value?.active && worker) {
      await new Promise<void>(resolve => {
        const timeout = setTimeout(finish, 5_000);
        function finish() { clearTimeout(timeout); worker!.removeEventListener('statechange', check); resolve(); }
        function check() { if (worker!.state === 'activated' || worker!.state === 'redundant') finish(); }
        worker.addEventListener('statechange', check);
        check();
      });
    }
    if (!value?.active) throw new Error('网页 App 正在准备通知，请稍后重试。');
    return value;
  }

  watch(authenticated, async (_, __, cleanup) => {
    enabled.value = false;
    error.value = '';
    notice.value = '';
    if (!supported || !authenticated()) return;
    const controller = new AbortController();
    cleanup(() => controller.abort());
    try {
      const subscription = await (await registration())?.pushManager.getSubscription();
      if (!subscription || controller.signal.aborted) return;
      const result = await request<{ enabled: boolean; error: string }>('/api/notifications/status', {
        method: 'POST', body: JSON.stringify({ subscription: subscription.toJSON() }), signal: controller.signal,
      });
      if (!controller.signal.aborted) { enabled.value = result.enabled; error.value = result.error; }
    } catch (failure) { if (!controller.signal.aborted) error.value = (failure as Error).message; }
  }, { immediate: true });

  async function toggle() {
    if (!supported || busy.value || !authenticated()) return;
    busy.value = true;
    error.value = '';
    notice.value = '';
    try {
      // iOS 要求通知授权直接由用户点击触发，不能放到网络请求之后。
      if (!enabled.value && await Notification.requestPermission() !== 'granted') {
        throw new Error('通知未获允许。可在 iPhone 设置中为这个网页 App 开启通知。');
      }
      const worker = await registration(true);
      if (!worker) throw new Error('网页 App 正在准备通知，请稍后重试。');
      let subscription = await worker.pushManager.getSubscription();
      if (enabled.value) {
        if (subscription) {
          await request('/api/notifications/unsubscribe', {
            method: 'POST', body: JSON.stringify({ endpoint: subscription.endpoint }),
          });
          await subscription.unsubscribe();
        }
        if (authenticated()) enabled.value = false;
      } else {
        if (!subscription) {
          const { publicKey } = await request<{ publicKey: string }>('/api/notifications/key');
          subscription = await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: applicationKey(publicKey) });
        }
        await request('/api/notifications/subscribe', { method: 'POST', body: JSON.stringify({ subscription: subscription.toJSON() }) });
        if (authenticated()) enabled.value = true;
      }
    } catch (failure) { if (authenticated()) error.value = (failure as Error).message; }
    finally { busy.value = false; }
  }

  async function sendTest() {
    if (busy.value || !enabled.value) return;
    busy.value = true;
    error.value = '';
    notice.value = '';
    try {
      const subscription = await (await registration())?.pushManager.getSubscription();
      if (!subscription) throw new Error('订阅已失效，请重新开启通知。');
      await request('/api/notifications/test', { method: 'POST', body: JSON.stringify({ endpoint: subscription.endpoint }) });
      if (authenticated()) notice.value = '测试通知已发送。';
    } catch (failure) { if (authenticated()) error.value = (failure as Error).message; }
    finally { busy.value = false; }
  }

  return { supported, enabled, busy, error, notice, hint, toggle, sendTest };
}
