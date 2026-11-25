type EncryptedFetch = (url: string, options: RequestInit) => Promise<Response>;
let relay: Readonly<{ accountId: string; desktopId: string; request: EncryptedFetch }> | undefined;

export const relayMode = () => typeof document !== 'undefined'
  && document.querySelector('meta[name="codex-mobile-relay"]') !== null;
export const relayDesktop = () => relay?.desktopId || '';
export const relayAccount = () => relay?.accountId || '';
export const desktopStorageKey = (key: string) => relay ? [key, relay.accountId, relay.desktopId].join(':') : key;

export class RelayError extends Error {
  readonly status: number;
  constructor(message: string, status = 0) { super(message); this.status = status; }
}

export function bindRelayDesktop(accountId: string, desktopId: string, request: EncryptedFetch) {
  const binding = { accountId, desktopId, request };
  relay = binding;
  return () => { if (relay === binding) relay = undefined; };
}

export function desktopFetch(url: string, options: RequestInit = {}) {
  if (!relayMode()) return fetch(url, options);
  if (!relay) return Promise.reject(new RelayError('请先选择电脑。', 401));
  return relay.request(url, options);
}

export function desktopApiPath(url: string, origin = location.origin) {
  try {
    const target = new URL(url, origin);
    if (target.origin === origin && target.pathname.startsWith('/api/')) return target.pathname + target.search;
  } catch { /* 让浏览器处理普通链接。 */ }
}
