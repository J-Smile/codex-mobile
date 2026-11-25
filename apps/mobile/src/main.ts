import './style.css';
import { request } from './api';
import { relayMode } from './transport';

const cloud = relayMode();
const initialSession = cloud || new URLSearchParams(location.hash.slice(1)).get('key')?.trim()
  ? undefined : request('/api/session');
// 页面下载期间的 401 也交给 App 处理，避免在接管前产生未处理的拒绝。
void initialSession?.catch(() => {});

const loadCloud = () => import('./features/relay/RelayHome.vue');
const loadLocal = () => import('./App.vue');
const page = cloud ? loadCloud() : loadLocal();
const runtime = import('vue').then(({ createApp }) => createApp);
void Promise.all([runtime, page])
  .then(([createApp, { default: Page }]) => createApp(Page, { initialSession }).mount('#app'))
  .catch(() => window.dispatchEvent(new Event('codex-startup-failed')));
