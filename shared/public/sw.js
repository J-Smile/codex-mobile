const buildVersion = '__BUILD_VERSION__';
const cacheName = 'codex-mobile-shell-v2-' + buildVersion;
const buildAssets = __BUILD_ASSETS__;
const relayWeb = '__RELAY_WEB__' === 'true';
self.addEventListener('install', (event) => {
  event.waitUntil((async () => {
    if (relayWeb && new URL(self.registration.scope).pathname !== '/') return self.skipWaiting();
    const cache = await caches.open(cacheName);
    try {
      await cache.addAll(['/', '/startup.js', '/icon.svg', '/icon-192.png', '/manifest.webmanifest', ...buildAssets]);
      // 仅迁移旧布局：旧 Worker 只查自己的缓存；v2 已能跨缓存读取，不再互相复制。
      const previousCaches = (await caches.keys()).filter((key) => key.startsWith('codex-mobile-shell-') && !key.startsWith('codex-mobile-shell-v2-'));
      await Promise.all(previousCaches.map(async (key) => {
        const previous = await caches.open(key);
        for (const asset of buildAssets.filter((path) => path.startsWith('/assets/'))) {
          await previous.put(asset, await cache.match(asset));
        }
      }));
    } catch (cause) {
      // 失败候选没有页面可用，只释放本次资源；旧页面仍使用原来的缓存。
      await caches.delete(cacheName);
      throw cause;
    }
    // 首次安装立即可用；升级等待旧页面退出，避免丢失草稿时抢占并清理其资源。
    if (!self.registration.active) await self.skipWaiting();
  })());
});
self.addEventListener('message', (event) => {
  if (event.data?.type === 'codex-mobile-build') event.ports[0]?.postMessage({ version: buildVersion });
});
self.addEventListener('activate', (event) => {
  if (relayWeb && new URL(self.registration.scope).pathname !== '/') return;
  event.waitUntil(Promise.all([
    self.clients.claim(),
    caches.keys().then((keys) => Promise.all(keys.filter((key) => key.startsWith('codex-mobile-') && key !== cacheName).map((key) => caches.delete(key)))),
  ]));
});
self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);
  if (relayWeb && url.origin === self.location.origin && url.pathname.startsWith('/relay-media/')) {
    event.respondWith(relayMedia(event, url.pathname));
    return;
  }
  // 版本检查必须来自网络，聊天、配对码和附件不写入离线缓存。
  if (event.request.method !== 'GET' || url.origin !== self.location.origin || url.pathname.startsWith('/api/') || url.pathname.startsWith('/relay/') || /^\/pair\/?$/.test(url.pathname) || url.pathname === '/version.json') return;
  // /assets/ 只发布带内容哈希的静态资源；同一 URL 的内容不会随升级而变。
  if (event.request.mode !== 'navigate' && url.pathname.startsWith('/assets/')) {
    event.respondWith((async () => {
      const cache = await caches.open(cacheName);
      // 等待激活的版本也已预缓存，新页面可以离线读取相同 URL 的哈希资源。
      const cached = await caches.match(event.request);
      if (cached) return cached;
      const response = await fetch(event.request);
      if (response.ok && !response.headers.get('content-type')?.includes('text/html')) {
        const copy = response.clone();
        event.waitUntil(cache.put(event.request, copy).catch(() => {}));
      }
      return response;
    })());
    return;
  }
  event.respondWith(fetch(event.request, event.request.mode === 'navigate' ? { cache: 'no-cache' } : undefined).then((response) => {
    if (response.ok) {
      const copy = response.clone();
      event.waitUntil(caches.open(cacheName).then((cache) => cache.put(event.request, copy)).catch(() => {}));
    }
    return response;
  }).catch(async () => {
    const cache = await caches.open(cacheName);
    return await cache.match(event.request) || (event.request.mode === 'navigate' && await cache.match('/')) || Response.error();
  }));
});

async function relayMedia(event, handle) {
  const client = await self.clients.get(event.clientId);
  if (!client) return new Response(null, { status: 404 });
  const { port1: port, port2 } = new MessageChannel();
  return new Promise(resolve => {
    let body;
    let pendingPull;
    const close = () => {
      clearTimeout(timeout);
      event.request.signal.removeEventListener('abort', cancel);
      port.close();
    };
    function cancel() { port.postMessage('cancel'); pendingPull?.(); close(); resolve(new Response(null, { status: 499 })); }
    const timeout = setTimeout(() => {
      resolve(new Response(null, { status: 504 }));
      cancel();
    }, 35_000);
    event.request.signal.addEventListener('abort', cancel, { once: true });
    port.onmessage = ({ data }) => {
      if ('status' in data) {
        clearTimeout(timeout);
        const headers = new Headers(data.headers);
        headers.set('cache-control', 'private, no-store');
        headers.set('x-content-type-options', 'nosniff');
        headers.set('content-security-policy', "sandbox; default-src 'none'");
        const stream = data.body && event.request.method !== 'HEAD' ? new ReadableStream({
          start(controller) { body = controller; },
          pull() {
            return new Promise(ready => { pendingPull = ready; port.postMessage('pull'); });
          },
          cancel,
        }) : null;
        resolve(new Response(stream, { status: data.status, headers }));
        if (!stream) cancel();
      } else {
        if (data.chunk) body?.enqueue(data.chunk);
        if (data.done || data.error) {
          if (data.done) body?.close();
          else body?.error(new Error('加密文件未能读取。'));
          close();
        }
        pendingPull?.();
      }
    };
    client.postMessage({ type: 'codex-relay-media', handle, head: event.request.method === 'HEAD', range: event.request.headers.get('range') }, [port2]);
  });
}

self.addEventListener('push', (event) => {
  if (!event.data) return;
  const notice = event.data.json();
  const [, , account, desktop] = new URL(self.registration.scope).pathname.split('/');
  event.waitUntil((async () => {
    if (relayWeb) {
      if (!desktop) return;
      const response = await fetch('/relay/account', { cache: 'no-store' }).catch(() => null);
      if (!response?.ok || (await response.json()).id !== account) return;
    }
    await self.registration.showNotification(notice.title || 'Codex', {
      body: notice.body || '', tag: notice.tag, icon: '/icon-192.png',
      data: { threadId: notice.threadId, ...(relayWeb && { account, desktop }) },
    });
  })());
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = new URL('/', self.location.origin);
  const desktop = event.notification.data?.desktop;
  if (desktop) url.searchParams.set('desktop', desktop);
  if (event.notification.data?.threadId) {
    if (desktop) url.hash = new URLSearchParams({ thread: event.notification.data.threadId }).toString();
    else url.searchParams.set('thread', event.notification.data.threadId);
  }
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const existing = windows.find((client) => {
      const current = new URL(client.url);
      return current.origin === url.origin && current.pathname === url.pathname
        && (!desktop || current.searchParams.get('desktop') === desktop);
    });
    const target = existing ? await existing.focus() : await self.clients.openWindow(url.href);
    if (!target || !event.notification.data?.threadId) return target;
    // 先唤醒页面；旧版本或尚未加载的页面无法确认时，使用带对话参数的导航。
    const received = await new Promise((resolve) => {
      const channel = new MessageChannel();
      const timeout = setTimeout(() => { channel.port1.close(); resolve(false); }, 1000);
      channel.port1.onmessage = () => { clearTimeout(timeout); channel.port1.close(); resolve(true); };
      target.postMessage({ type: 'codex-open-thread', threadId: event.notification.data.threadId, ...(desktop && { desktop, account: event.notification.data.account }) }, [channel.port2]);
    });
    if (!received) return target.navigate(url.href);
    return target;
  })());
});
