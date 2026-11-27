<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue';
import { ArrowDown, Close, FullScreen, Key, Loading, Monitor, Iphone } from '@element-plus/icons-vue';
import NativeSheet from '../../components/NativeSheet.vue';
import { bindRelayDesktop, RelayError } from '../../transport';
import { listenForRelayMedia } from '../../relayMedia';
import { createPhoneIdentity, RelayConnection } from './connection';
import DesktopApprovalStatus from './DesktopApprovalStatus.vue';
import RelayDeviceRow from './RelayDeviceRow.vue';
import RelayServerLatency from './RelayServerLatency.vue';
import RelayWelcomePreview from './RelayWelcomePreview.vue';
import { requestDesktopApproval } from './desktopApproval';
import { forgetDesktop, linkDesktop, loadDesktops, readPairingInvitation, removeAccountDesktop, saveDesktop, type Desktop, type PairingInvitation, type PairingTarget } from './devices';
import { createPasskey, signInWithPasskey, signUpWithPasskey, supportsPasskeys } from './passkeys';
import { useRelayLatency } from './useRelayLatency';

const App = defineAsyncComponent(() => import('../../App.vue'));
const QrConnect = defineAsyncComponent(() => import('../pairing/QrConnect.vue'));
const accountPath = '/relay/account';
const accountChangeKey = 'codex-mobile-account-change';
const selectionKey = (accountId: string) => 'codex-mobile-active-desktop:' + accountId;
interface AccountDirectory {
  id: string;
  name: string;
  hasPasskey?: boolean;
  desktops: { id: string; name: string; online: boolean }[];
}
const account = shallowRef<AccountDirectory>();
const directoryHeading = ref<HTMLElement>();
const desktops = shallowRef<Desktop[]>([]);
const active = shallowRef<Desktop>();
const pickerOpen = ref(false);
const scanOpen = ref(false);
const pairingTarget = shallowRef<PairingTarget>();
const loading = ref(true);
const pairing = ref(false);
const approval = shallowRef<{ name: string; code: string }>();
const error = ref('');
const accountError = ref('');
const loginKey = ref('');
const loginField = ref<HTMLInputElement>();
const passkeysSupported = supportsPasskeys();
const showLoginKey = ref(!passkeysSupported);
const showOtherLogin = ref(false);
const accountAction = ref<'' | 'key' | 'passkey' | 'sign-up' | 'create' | 'sign-out'>('');
const changingAccount = computed(() => !!accountAction.value);
const showingWelcome = computed(() => !account.value && !approval.value && !pendingPairingLink.value);
const passkeyCreated = ref(false);
const hasPasskey = computed(() => passkeyCreated.value || account.value?.hasPasskey);
const accountLabel = computed(() => {
  const current = account.value;
  if (!current) return '';
  return current.name === 'Codex Mobile ' + current.id.slice(0, 12) ? '我的账号' : current.name;
});
const passkeyError = ref('');
const link = ref('');
const remove = ref('');
const removing = ref('');
const pageVisible = ref(!document.hidden);
const statusKnown = ref(false);
const deviceRows = computed(() => {
  const paired = new Map(desktops.value.map(desktop => [desktop.id, desktop]));
  return (account.value?.desktops || []).map(({ id, name, online }) => ({
    id, name, paired: paired.get(id),
    online: statusKnown.value && online,
    status: statusKnown.value ? (online ? '在线' : '离线') : '状态未知',
  }));
});
let releaseConnection: (() => void) | undefined;
let activeConnection: RelayConnection | undefined;
let initializationFailed = false;
let approvalRequest: AbortController | undefined;
const pendingPairingLink = shallowRef<PairingInvitation>();
const directoryFeedback = ref<HTMLElement>();
const pickerFeedback = ref<HTMLElement>();
let removalTrigger: HTMLButtonElement | undefined;

const { serverLatency, desktopLatency } = useRelayLatency({
  visibleAccount: () => pageVisible.value && !loading.value && !scanOpen.value && !pairing.value && !changingAccount.value
    && (pickerOpen.value || !active.value) ? account.value?.id : undefined,
  desktops: () => deviceRows.value.flatMap(row => row.online && row.paired ? [row.paired] : []),
  activeConnection: id => id === active.value?.id ? activeConnection : undefined,
});

watch(
  () => pageVisible.value && !loading.value && !removing.value ? account.value?.id : undefined,
  (accountId, _, onCleanup) => {
    if (!accountId) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let request: AbortController | undefined;
    async function refresh() {
      const controller = new AbortController();
      request = controller;
      const deadline = setTimeout(() => controller.abort(), 8_000);
      try {
        const current = await readAccount(controller.signal);
        if (stopped) return;
        if (!current || current.id !== accountId) {
          location.reload();
          return;
        }
        account.value = current;
        statusKnown.value = true;
        if (active.value && !current.desktops.some(item => item.id === active.value?.id)) {
          releaseConnection?.();
          choose();
        }
      } catch {
        if (!stopped) statusKnown.value = false;
      } finally {
        clearTimeout(deadline);
        if (!stopped) timer = setTimeout(refresh, 10_000);
      }
    }

    onCleanup(() => {
      stopped = true;
      request?.abort();
      clearTimeout(timer);
    });
    void refresh();
  },
);

async function readAccount(signal?: AbortSignal): Promise<AccountDirectory | undefined> {
  const response = await fetch(accountPath, { cache: 'no-store', signal });
  if (response.status === 401) return;
  if (!response.ok) throw new Error('无法读取云端账号。');
  return response.json();
}

async function revealLoginKey() {
  showLoginKey.value = !showLoginKey.value;
  if (showLoginKey.value) {
    await nextTick();
    loginField.value?.focus();
  }
}

async function signIn(method: 'passkey' | 'key' | 'sign-up') {
  if (changingAccount.value || (method === 'key' && !loginKey.value.trim())) return;
  accountError.value = '';
  accountAction.value = method;
  try {
    if (method === 'passkey') {
      await signInWithPasskey();
    } else if (method === 'sign-up') {
      await signUpWithPasskey();
    } else {
      const response = await fetch(accountPath, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ loginKey: loginKey.value.trim() }),
      });
      if (response.status === 401) throw new Error('网页登录密钥无效。');
      if (!response.ok) throw new Error('登录失败，请稍后再试。');
    }
    loginKey.value = '';
    localStorage.setItem(accountChangeKey, crypto.randomUUID());
    await initialize();
    if (account.value && !active.value && !pickerOpen.value) {
      await nextTick();
      directoryHeading.value?.focus({ preventScroll: true });
    }
  } catch (failure) {
    accountError.value = (failure as Error).message;
  } finally { accountAction.value = ''; }
}

async function enrollPasskey() {
  if (changingAccount.value) return;
  accountAction.value = 'create';
  passkeyError.value = '';
  try {
    await createPasskey();
    passkeyCreated.value = true;
  } catch (failure) {
    passkeyError.value = (failure as Error).message;
  } finally { accountAction.value = ''; }
}

async function signOut() {
  if (!account.value || changingAccount.value || removing.value) return;
  accountAction.value = 'sign-out';
  error.value = '';
  try {
    const response = await fetch(accountPath, { method: 'DELETE' });
    if (!response.ok) throw new Error('退出失败，请重试。');
    releaseConnection?.();
    localStorage.setItem(accountChangeKey, crypto.randomUUID());
    location.replace('/');
  } catch (failure) {
    error.value = (failure as Error).message;
    accountAction.value = '';
  }
}

function choose(desktop?: Desktop, accountId = account.value?.id) {
  if (!accountId) return;
  const key = selectionKey(accountId);
  if (desktop) localStorage.setItem(key, desktop.id);
  else localStorage.removeItem(key);
  const url = new URL('/', location.origin);
  if (desktop) url.searchParams.set('desktop', desktop.id);
  location.assign(url.href);
}

function selectDesktop(target: PairingTarget, desktop?: Desktop) {
  if (removing.value) return;
  if (!desktop) {
    error.value = '';
    pairingTarget.value = target;
    scanOpen.value = true;
  } else if (desktop.id === active.value?.id) {
    closePicker();
  } else {
    choose(desktop);
  }
}

async function add(value: string | PairingInvitation = link.value) {
  if (pairing.value || removing.value) return;
  scanOpen.value = false;
  error.value = '';
  accountError.value = '';
  pairing.value = true;
  let pending: RelayConnection | undefined;
  try {
    const invitation = typeof value === 'string' ? await readPairingInvitation(value, pairingTarget.value) : value;
    const { id } = invitation;
    const currentAccount = account.value?.id;
    const accountId = invitation.version === 2 ? invitation.accountId : currentAccount;
    if (!accountId) {
      pendingPairingLink.value = invitation;
      return;
    }
    if (currentAccount && currentAccount !== accountId) {
      throw new Error('这台电脑属于其他账号，请退出当前账号后重试。');
    }
    const saved = (currentAccount ? desktops.value : await loadDesktops(accountId)).find(desktop => desktop.id === id);
    const desktop: Desktop = { id, publicKey: invitation.publicKey, name: invitation.name, phone: saved?.phone || createPhoneIdentity() };
    const name = /iPhone/i.test(navigator.userAgent) ? 'iPhone' : /iPad/i.test(navigator.userAgent) ? 'iPad'
      : /Android/i.test(navigator.userAgent) ? 'Android 手机' : '网页设备';
    if (invitation.version === 2) {
      approvalRequest = new AbortController();
      await requestDesktopApproval(invitation, desktop.phone, name, approvalRequest.signal, code => {
        approval.value = { name: desktop.name, code };
      });
    } else {
      if (invitation.version === 3) {
        if (!account.value?.desktops.some(device => device.id === id)) {
          const updated = await readAccount();
          if (updated?.id !== accountId) throw new Error('账号已变化，请重新登录后再扫码。');
          account.value = updated;
          if (!updated.desktops.some(device => device.id === id)) {
            if (!invitation.claim) throw new Error('这台电脑属于其他账号，请切换到它的账号后再扫码。');
            await linkDesktop(id, invitation.claim);
            const linked = await readAccount();
            if (linked?.id !== accountId) throw new Error('账号已变化，请重新登录后再扫码。');
            account.value = linked;
          }
        }
      }
      pending = new RelayConnection(desktop);
      const deadline = Date.now() + 8_000;
      for (;;) {
        try {
          await pending.pair(invitation.ticket, name);
          break;
        } catch (failure) {
          if (invitation.version !== 3 || !invitation.claim
            || !(failure instanceof RelayError && failure.status === 404)
            || Date.now() >= deadline) throw failure;
          await new Promise(resolve => setTimeout(resolve, 500));
        }
      }
    }
    await saveDesktop(accountId, desktop);
    if (invitation.version === 2) localStorage.setItem(accountChangeKey, crypto.randomUUID());
    link.value = '';
    choose(desktop, accountId);
  } catch (failure) {
    if (!approvalRequest?.signal.aborted) {
      if (account.value) error.value = (failure as Error).message;
      else accountError.value = (failure as Error).message;
    }
  } finally {
    pending?.close();
    approvalRequest = undefined;
    approval.value = undefined;
    pairing.value = false;
    if (error.value && account.value) {
      await nextTick();
      (pickerOpen.value ? pickerFeedback.value : directoryFeedback.value)?.focus();
    }
  }
}

function incomingLink() {
  const url = new URL(location.href);
  if (pairing.value || !new URLSearchParams(url.hash.slice(1)).has('pair')) return false;
  const invitation = url.href;
  url.hash = '';
  history.replaceState(history.state, '', url.href);
  if (account.value) pickerOpen.value = true;
  void add(invitation);
  return true;
}

function cancelApproval() {
  approvalRequest?.abort();
}

function cancelWithFeedback() {
  cancelApproval();
  const message = '已取消授权。重试时，请在电脑上点击「重新生成二维码」。';
  if (account.value) error.value = message;
  else accountError.value = message;
}

function toggleRemoval(id: string, event: MouseEvent) {
  remove.value = remove.value === id ? '' : id;
  removalTrigger = remove.value ? event.currentTarget as HTMLButtonElement : undefined;
}

async function cancelRemoval() {
  remove.value = '';
  await nextTick();
  removalTrigger?.focus({ preventScroll: true });
  removalTrigger = undefined;
}

async function forget(desktop: Desktop) {
  if (!account.value || removing.value) return;
  error.value = '';
  try {
    await forgetDesktop(account.value.id, desktop.id);
    desktops.value = desktops.value.filter(item => item.id !== desktop.id);
    remove.value = '';
    removalTrigger = undefined;
    if (desktop.id === active.value?.id) choose();
  } catch {
    error.value = '无法移除本机保存的配对，请重试。';
  }
}

async function removeFromAccount(id: string) {
  const current = account.value;
  if (!current || removing.value || pairing.value || changingAccount.value) return;
  removing.value = id;
  error.value = '';
  try {
    await removeAccountDesktop(id);
    account.value = { ...current, desktops: current.desktops.filter(item => item.id !== id) };
    desktops.value = desktops.value.filter(item => item.id !== id);
    remove.value = '';
    removalTrigger = undefined;
    try { await forgetDesktop(current.id, id); }
    catch { error.value = '电脑已从云账号移除，但本机配对缓存未清理；请刷新页面。'; }
    if (id === active.value?.id) {
      releaseConnection?.();
      choose();
    }
  } catch (failure) {
    error.value = (failure as Error).message;
  } finally { removing.value = ''; }
}

async function initialize() {
  initializationFailed = false;
  loading.value = true;
  accountError.value = '';
  try {
    if (!window.isSecureContext || !navigator.serviceWorker) throw new Error('请用 HTTPS 打开手机网页。');
    const current = await readAccount();
    if (!current) { incomingLink(); return; }
    await navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' });
    if (!navigator.serviceWorker.controller) {
      await new Promise<void>((resolve, reject) => {
        const controller = new AbortController();
        const timeout = setTimeout(() => {
          controller.abort();
          reject(new Error('网页组件加载失败，请刷新后重试。'));
        }, 15_000);
        const ready = () => {
          clearTimeout(timeout);
          controller.abort();
          resolve();
        };
        navigator.serviceWorker.addEventListener('controllerchange', ready, { once: true, signal: controller.signal });
        if (navigator.serviceWorker.controller) ready();
      });
    }
    desktops.value = await loadDesktops(current.id);
    account.value = current;
    statusKnown.value = true;
    if (incomingLink()) return;
    if (pendingPairingLink.value) {
      const invitation = pendingPairingLink.value;
      pendingPairingLink.value = undefined;
      pickerOpen.value = true;
      void add(invitation);
      return;
    }
    const url = new URL(location.href);
    const requested = url.searchParams.get('desktop') || localStorage.getItem(selectionKey(current.id));
    const desktop = desktops.value.find(item => item.id === requested);
    const listed = current.desktops.find(item => item.id === desktop?.id);
    if (desktop && listed) {
      url.searchParams.set('desktop', desktop.id);
      history.replaceState(history.state, '', url.href);
      const connection = new RelayConnection(desktop, location.origin, () => location.reload());
      const releaseTransport = bindRelayDesktop(current.id, desktop.id, connection.request.bind(connection));
      const releaseMedia = listenForRelayMedia(navigator.serviceWorker);
      activeConnection = connection;
      releaseConnection = () => {
        releaseMedia();
        releaseTransport();
        connection.close();
        activeConnection = undefined;
      };
      active.value = { ...desktop, name: listed.name };
    }
  } catch (failure) {
    initializationFailed = true;
    accountError.value = (failure as Error).message || '无法读取手机保存的设备，请刷新后重试。';
  } finally { loading.value = false; }
}

function openPicker() {
  error.value = '';
  pairingTarget.value = undefined;
  pickerOpen.value = true;
}

function scanForDesktop() {
  if (removing.value) return;
  error.value = '';
  pairingTarget.value = undefined;
  scanOpen.value = true;
}

function closePicker() {
  cancelApproval();
  pickerOpen.value = false;
  pairingTarget.value = undefined;
  remove.value = '';
  removalTrigger = undefined;
}

function updateVisibility() {
  pageVisible.value = !document.hidden;
  retryInitialization();
}

function retryInitialization() {
  if (pageVisible.value && initializationFailed && !loading.value && !changingAccount.value) void initialize();
}

function accountChanged(event: StorageEvent) {
  if (event.key === accountChangeKey) location.reload();
}

onMounted(() => {
  window.addEventListener('hashchange', incomingLink);
  window.addEventListener('storage', accountChanged);
  window.addEventListener('online', retryInitialization);
  window.addEventListener('pageshow', retryInitialization);
  document.addEventListener('visibilitychange', updateVisibility);
  void initialize();
});
onUnmounted(() => {
  cancelApproval();
  releaseConnection?.();
  window.removeEventListener('hashchange', incomingLink);
  window.removeEventListener('storage', accountChanged);
  window.removeEventListener('online', retryInitialization);
  window.removeEventListener('pageshow', retryInitialization);
  document.removeEventListener('visibilitychange', updateVisibility);
});
</script>

<template>
  <App v-if="active" :desktop-name="active.name" :device-picker-open="pickerOpen || scanOpen" @choose-desktop="openPicker" />
  <main v-else class="login-screen relay-entry" :class="{ 'relay-welcome': showingWelcome, 'relay-directory': account }">
    <section class="login-card relay-card">
      <header class="relay-intro">
        <div class="relay-brand"><img src="/icon.svg" alt="" width="28" height="28" /><span v-if="account">Codex Mobile</span></div>
        <h1 ref="directoryHeading" tabindex="-1">{{ account ? '电脑' : 'Codex Mobile' }}</h1>
        <p v-if="!account && !approval && pendingPairingLink">登录后会继续连接「{{ pendingPairingLink.name }}」。</p>
        <p v-else-if="showingWelcome">在手机上继续电脑里的 Codex 对话。</p>
      </header>
      <RelayWelcomePreview v-if="showingWelcome" />
      <div class="relay-body">
        <p v-if="loading" class="relay-loading" role="status"><Loading class="is-spin" aria-hidden="true" />正在读取账号…</p>
        <DesktopApprovalStatus v-else-if="approval" v-bind="approval" @cancel="cancelWithFeedback" />
        <template v-else-if="account">
          <p v-if="error && !pickerOpen" ref="directoryFeedback" class="relay-feedback error-text" tabindex="-1" role="alert">{{ error }}</p>
          <RelayServerLatency :sample="serverLatency" />
          <div v-if="!deviceRows.length" class="relay-directory-empty">
            <div class="relay-guide-icons" aria-hidden="true"><span><Monitor /></span><i></i><span><Iphone /></span></div>
            <h2>连接第一台电脑</h2>
            <p>在电脑端点「手机扫码连接」，再用手机扫描显示的二维码。</p>
          </div>
          <section v-else class="relay-directory-list" aria-labelledby="relay-directory-title">
            <div class="relay-directory-heading"><h2 id="relay-directory-title">已添加</h2><span>{{ deviceRows.length }} 台</span></div>
            <div class="relay-device-list">
              <RelayDeviceRow v-for="{ id, name, paired, online, status } in deviceRows" :key="id"
                :name="name" :paired="!!paired" :online="online" :status="status" :latency="desktopLatency.get(id)" :disabled="changingAccount || pairing || !!removing"
                :confirm-removal="remove === id" :removing="removing === id" @select="selectDesktop({ id, name }, paired)"
                @toggle-removal="toggleRemoval(id, $event)" @cancel-removal="cancelRemoval" @forget="paired && forget(paired)" @remove-account="removeFromAccount(id)" />
            </div>
          </section>
          <button class="primary-button relay-primary" type="button" :disabled="pairing || changingAccount || !!removing" :aria-busy="pairing" @click="scanForDesktop">
            <Loading v-if="pairing" class="is-spin" aria-hidden="true" /><FullScreen v-else aria-hidden="true" />{{ pairing ? '正在连接电脑…' : '添加电脑' }}
          </button>
          <div class="relay-account-row">
            <span class="relay-account-name">{{ accountLabel }}</span>
            <div class="relay-account-actions">
              <button v-if="passkeysSupported && !hasPasskey" class="text-button" type="button" :disabled="changingAccount || pairing" @click="enrollPasskey">{{ accountAction === 'create' ? '正在创建…' : '创建通行密钥' }}</button>
              <button class="text-button" type="button" :disabled="changingAccount || pairing" @click="signOut">{{ accountAction === 'sign-out' ? '正在退出…' : '退出账号' }}</button>
            </div>
          </div>
          <p v-if="passkeyCreated" class="relay-feedback muted" role="status">通行密钥已创建，下次可用设备解锁登录。</p>
          <p v-if="passkeyError" class="relay-feedback error-text" role="alert">{{ passkeyError }}</p>
        </template>
        <div v-else class="relay-login" :aria-busy="changingAccount || pairing">
          <template v-if="passkeysSupported">
            <button class="primary-button relay-primary" type="button" :disabled="changingAccount || pairing" @click="signIn('sign-up')">
              <Loading v-if="accountAction === 'sign-up'" class="is-spin" aria-hidden="true" /><Key v-else aria-hidden="true" />
              {{ accountAction === 'sign-up' ? '正在创建…' : '用通行密钥创建账号' }}
            </button>
            <button class="secondary-button" type="button" :disabled="changingAccount || pairing" @click="signIn('passkey')">
              {{ accountAction === 'passkey' ? '正在验证…' : '用通行密钥登录' }}
            </button>
          </template>
          <p v-else class="muted relay-feedback">请用支持通行密钥的系统浏览器打开，创建账号。</p>
          <button class="text-button relay-login-toggle" type="button" :disabled="changingAccount || pairing"
            :aria-expanded="showOtherLogin" aria-controls="cloud-other-login" @click="showOtherLogin = !showOtherLogin"><span>其他登录方式</span><ArrowDown aria-hidden="true" /></button>
          <Transition name="relay-options">
            <div v-show="showOtherLogin" id="cloud-other-login" class="relay-login relay-login-alternatives" :inert="!showOtherLogin">
              <button class="secondary-button" type="button" :disabled="changingAccount || pairing" @click="scanOpen = true"><FullScreen aria-hidden="true" />扫码登录旧账号</button>
              <button v-if="passkeysSupported" class="text-button relay-login-toggle" type="button" :disabled="changingAccount || pairing"
                :aria-expanded="showLoginKey" aria-controls="cloud-key-form" @click="revealLoginKey"><span>使用网页登录密钥</span><ArrowDown aria-hidden="true" /></button>
              <form v-show="showLoginKey" id="cloud-key-form" class="relay-login" @submit.prevent="signIn('key')">
                <label for="cloud-login-key">网页登录密钥</label>
                <input id="cloud-login-key" ref="loginField" v-model="loginKey" type="password" autocomplete="current-password"
                  autocapitalize="none" spellcheck="false" :readonly="changingAccount" maxlength="80" />
                <button class="secondary-button" type="submit" :disabled="changingAccount || pairing || !loginKey.trim()">
                  {{ accountAction === 'key' ? '正在登录…' : '登录' }}
                </button>
              </form>
            </div>
          </Transition>
          <p v-if="accountError" class="relay-feedback error-text" role="alert">{{ accountError }}</p>
        </div>
      </div>
    </section>
  </main>
  <NativeSheet v-if="account" :open="pickerOpen" labelledby="desktop-picker-title" panel-class="settings-modal app-settings" @close="closePicker">
    <div class="modal-heading relay-sheet-heading">
      <h2 id="desktop-picker-title">{{ active ? '切换电脑' : '电脑' }}</h2>
      <button class="icon-button" type="button" aria-label="关闭设备列表" @click="closePicker"><Close /></button>
    </div>
    <p v-if="error" ref="pickerFeedback" class="relay-feedback relay-picker-feedback error-text" tabindex="-1" role="alert">{{ error }}</p>
    <RelayServerLatency class="relay-picker-latency" :sample="serverLatency" />
    <div v-if="deviceRows.length" class="relay-device-list">
      <RelayDeviceRow v-for="{ id, name, paired, online, status } in deviceRows" :key="id"
        :name="name" :paired="!!paired" :online="online" :status="status" :latency="desktopLatency.get(id)" :current="id === active?.id"
        :disabled="pairing || changingAccount || !!removing" :confirm-removal="remove === id" :removing="removing === id"
        @select="selectDesktop({ id, name }, paired)" @toggle-removal="toggleRemoval(id, $event)" @cancel-removal="cancelRemoval"
        @forget="paired && forget(paired)" @remove-account="removeFromAccount(id)" />
    </div>
    <div class="relay-picker-actions">
      <DesktopApprovalStatus v-if="approval" v-bind="approval" @cancel="cancelWithFeedback" />
      <template v-else>
        <button class="primary-button relay-primary" type="button" :disabled="pairing || changingAccount || !!removing" :aria-busy="pairing" @click="scanOpen = true">
          <Loading v-if="pairing" class="is-spin" aria-hidden="true" /><FullScreen v-else aria-hidden="true" />
          {{ pairing ? '正在连接电脑…' : pairingTarget ? '扫码配对「' + pairingTarget.name + '」' : '扫码添加电脑' }}
        </button>
        <details class="relay-link-disclosure">
          <summary>使用连接链接<ArrowDown aria-hidden="true" /></summary>
          <form class="relay-pair-form" @submit.prevent="add()">
            <label for="cloud-pair-link">电脑端复制的连接链接</label>
            <input id="cloud-pair-link" v-model="link" type="password" autocomplete="off" inputmode="url" :readonly="pairing" maxlength="1500" />
            <button class="secondary-button" type="submit" :disabled="pairing || changingAccount || !link.trim()">通过链接连接</button>
          </form>
        </details>
      </template>
    </div>
    <div v-if="active" class="relay-account-row">
      <span class="relay-account-name">{{ accountLabel }}</span>
      <div class="relay-account-actions">
        <button v-if="passkeysSupported && !hasPasskey" class="text-button" type="button" :disabled="changingAccount || pairing" @click="enrollPasskey">{{ accountAction === 'create' ? '正在创建…' : '创建通行密钥' }}</button>
        <button class="text-button" type="button" :disabled="changingAccount || pairing" @click="signOut">{{ accountAction === 'sign-out' ? '正在退出…' : '退出账号' }}</button>
      </div>
    </div>
    <p v-if="active && passkeyCreated" class="relay-feedback muted" role="status">通行密钥已创建，下次可用设备解锁登录。</p>
    <p v-if="active && passkeyError" class="relay-feedback error-text" role="alert">{{ passkeyError }}</p>
  </NativeSheet>
  <QrConnect v-if="scanOpen" relay :login="!account" :target="pairingTarget" @close="scanOpen = false" @pair="add" />
</template>

<style scoped>
.relay-entry {
  padding: max(24px, var(--safe-top)) max(20px, var(--safe-right)) max(24px, var(--safe-bottom)) max(20px, var(--safe-left));
}
.relay-card {
  display: grid; grid-template: 'intro' auto 'actions' auto / minmax(0, 1fr); align-items: start;
  gap: 20px; width: min(100%, 420px);
}
.relay-intro { grid-area: intro; }
.relay-body { grid-area: actions; display: grid; gap: 16px; min-width: 0; }
.relay-brand { display: flex; align-items: center; gap: 8px; margin-bottom: 14px; color: var(--muted); font-size: 13px; font-weight: 600; }
.relay-brand img { flex: none; border-radius: 8px; }
.relay-card h1 { margin: 0; font-size: clamp(28px, 8vw, 40px); font-weight: 650; line-height: 1.15; letter-spacing: -.035em; }
.relay-intro > p { margin: 9px 0 0; color: var(--muted); font-size: 15px; line-height: 1.6; overflow-wrap: anywhere; }
.relay-welcome .relay-card { grid-template-areas: 'intro' 'preview' 'actions'; }
.relay-welcome .relay-preview { grid-area: preview; }
.relay-login { display: grid; align-content: start; gap: 8px; }
.relay-login label { margin: 0; }
.relay-primary {
  width: 100%; margin-top: 0; min-height: 50px; background: var(--signal); color: var(--signal-ink);
  font-size: 15px; font-weight: 600; overflow-wrap: anywhere;
}
.relay-primary svg, .relay-login button svg { flex: none; }
.relay-login-toggle, .relay-link-disclosure summary {
  display: inline-flex; align-items: center; gap: 7px; justify-self: start; min-height: 44px; padding-block: 0; font-size: 13px;
}
.relay-login-toggle svg, .relay-link-disclosure summary svg { width: 13px; height: 13px; transition: transform 180ms var(--motion-ease); }
.relay-login-toggle[aria-expanded="true"] svg, .relay-link-disclosure[open] summary svg { transform: rotate(180deg); }
.relay-login-alternatives { padding-top: 12px; border-top: 1px solid var(--border); }
.relay-options-enter-active { transition: opacity 170ms ease, transform 220ms var(--motion-ease); }
.relay-options-leave-active { transition: opacity 100ms ease; }
.relay-options-enter-from { opacity: 0; transform: translateY(-4px); }
.relay-options-leave-to { opacity: 0; }
.relay-loading { display: flex; align-items: center; gap: 8px; min-height: 50px; margin: 0; color: var(--muted); font-size: 14px; }

.relay-directory {
  align-items: start;
  padding-block: calc(24px + var(--safe-top)) max(24px, var(--safe-bottom));
}
.relay-directory .relay-card { width: min(100%, 520px); }
.relay-directory .relay-brand { margin-bottom: 22px; }
.relay-directory-heading { display: flex; justify-content: space-between; gap: 12px; align-items: center; margin: 0 0 9px; color: var(--muted); font-size: 12px; }
.relay-directory-heading h2 { margin: 0; color: var(--text); font-size: 14px; font-weight: 600; }
.relay-directory-heading span { flex: none; font-variant-numeric: tabular-nums; }
.relay-directory-empty {
  display: grid; justify-items: start; gap: 8px; padding: 20px; border: 1px solid var(--border);
  border-radius: 17px; background: var(--sidebar);
}
.relay-guide-icons { display: flex; align-items: center; margin: 0 0 6px; color: var(--signal); }
.relay-guide-icons span { display: grid; place-items: center; width: 44px; height: 44px; border-radius: 12px; background: var(--signal-soft); }
.relay-guide-icons svg { width: 22px; height: 22px; }
.relay-guide-icons i { width: 24px; border-top: 1px dashed var(--signal); opacity: .6; }
.relay-directory-empty h2 { margin: 0; color: var(--text); font-size: 18px; font-weight: 600; }
.relay-directory-empty p { margin: 0; color: var(--muted); font-size: 14px; line-height: 1.55; }

.relay-sheet-heading { margin-bottom: 10px; }
.relay-picker-latency { margin-bottom: 10px; }
.relay-device-list { overflow: hidden; border: 1px solid var(--border); border-radius: 14px; background: var(--page); }
.relay-device-entry + .relay-device-entry { border-top: 1px solid var(--border); }
.relay-picker-actions { display: grid; gap: 7px; margin-top: 14px; }
.relay-picker-feedback { margin-bottom: 10px; }
.relay-link-disclosure summary { cursor: pointer; list-style: none; color: var(--muted); }
.relay-link-disclosure summary::-webkit-details-marker { display: none; }
.relay-pair-form { display: grid; gap: 8px; padding: 2px 0 8px; }
.relay-pair-form label { margin: 0; color: var(--muted); font-size: 13px; }
.relay-account-row {
  display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 0 16px;
  min-height: 48px; padding-top: 5px; border-top: 1px solid var(--border); color: var(--muted); font-size: 13px;
}
.relay-account-row > span { min-width: 0; overflow-wrap: anywhere; }
.relay-account-actions { display: flex; flex-wrap: wrap; gap: 0 14px; }
.relay-account-actions .text-button { flex: none; min-height: 44px; font-size: 13px; }
.relay-feedback { margin: 0; font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
@media (min-width: 760px), (min-width: 620px) and (max-height: 500px) {
  .relay-welcome .relay-card {
    grid-template: 'intro preview' auto 'actions preview' 1fr / minmax(0, .88fr) minmax(0, 1.12fr);
    column-gap: clamp(24px, 5vw, 54px); width: min(100%, 820px);
  }
  .relay-welcome .relay-preview { align-self: center; }
}
@media (max-height: 500px) {
  .relay-directory { padding-block-start: max(14px, var(--safe-top)); }
  .relay-directory .relay-brand { margin-bottom: 12px; }
  .relay-directory .relay-card { gap: 12px; }
}
@media (prefers-reduced-motion: reduce) {
  .relay-login-toggle svg, .relay-link-disclosure summary svg { transition: none; }
  .relay-options-enter-active { transition: opacity 100ms ease; }
  .relay-options-enter-from { transform: none; }
}
</style>
