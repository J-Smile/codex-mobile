import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope, nextTick, ref } from 'vue';
import { recentThreads, threadActive, useThreadCatalog } from '../../mobile/src/features/threads/useThreadCatalog.ts';

const settle = async () => { await new Promise((resolve) => setImmediate(resolve)); await nextTick(); };

function fixture(t) {
  const original = Object.fromEntries(['document', 'window', 'fetch'].map((key) => [key, globalThis[key]]));
  const document = Object.assign(new EventTarget(), { hidden: false, querySelector: () => null });
  const window = new EventTarget();
  const authenticated = ref(true);
  const calls = [];
  let revision = 1;
  let expired = 0;
  Object.assign(globalThis, { document, window, fetch: async (url) => {
    calls.push(url);
    if (url === '/api/projects') return Response.json({ projects: [{ projectId: 'project', label: '项目 ' + revision }] });
    if (url === '/api/threads') return Response.json({ threads: [{ id: 'recent', title: '最新 ' + revision, projectId: 'project' }], nextCursor: 'page-' + revision });
    if (url === '/api/threads?cursor=page-' + revision) return Response.json({ threads: [{ id: 'older', title: '历史 ' + revision, projectId: 'project' }], nextCursor: 'last-' + revision });
    if (url === '/api/threads?cursor=last-1') return Response.json({ threads: [{ id: 'oldest', title: '更早', projectId: 'project' }], nextCursor: null });
    throw new Error('旧游标或重复翻页：' + url);
  } });
  const scope = effectScope();
  const catalog = scope.run(() => useThreadCatalog({ authenticated: () => authenticated.value,
    expired: () => { expired++; authenticated.value = false; } }));
  t.after(() => {
    scope.stop();
    for (const [key, value] of Object.entries(original)) {
      if (value === undefined) delete globalThis[key];
      else globalThis[key] = value;
    }
  });
  return { catalog, document, window, scope, calls, authenticated, expired: () => expired, publish: () => { revision++; } };
}

test('回前台只刷新首屏目录，项目预览保留已读记录并继续使用原游标', async (t) => {
  const f = fixture(t);
  await f.catalog.refresh();
  await f.catalog.loadProjectHistory();
  assert.deepEqual(f.catalog.latestByProject.value.get('project').map(thread => thread.title), ['最新 1', '历史 1']);
  assert.equal(f.catalog.hasMoreProjectHistory.value, true);
  f.document.hidden = true;
  f.publish();
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.equal(f.catalog.threads.value[0].title, '最新 1');
  f.document.hidden = false;
  f.document.dispatchEvent(new Event('visibilitychange'));
  await settle();
  assert.deepEqual(f.catalog.threads.value.map((thread) => thread.title), ['最新 2']);
  assert.deepEqual(f.catalog.latestByProject.value.get('project').map(thread => thread.title), ['最新 2', '历史 1']);
  assert.equal(f.catalog.projects.value[0].label, '项目 2');
  assert.deepEqual(f.calls, ['/api/projects', '/api/threads', '/api/threads?cursor=page-1', '/api/projects', '/api/threads']);
  await f.catalog.loadProjectHistory();
  assert.equal(f.calls.at(-1), '/api/threads?cursor=last-1');
  assert.deepEqual(f.catalog.latestByProject.value.get('project').map(thread => thread.id), ['recent', 'older', 'oldest']);
  assert.equal(f.catalog.hasMoreProjectHistory.value, false);
});

test('多个项目共用历史游标，跳过空页并各自取按更新时间排列的三条', async t => {
  const f = fixture(t);
  const project = (id, updatedAt, projectId = 'old') => ({ id, title: id, projectId, updatedAt });
  const firstTime = 1_800_000_100;
  const pages = new Map([
    ['/api/projects', { projects: [{ projectId: 'old' }, { projectId: 'main' }] }],
    ['/api/threads', { threads: [project('newest', firstTime, 'main')], nextCursor: 'empty' }],
    ['/api/threads?cursor=empty', { threads: [], nextCursor: 'first-match' }],
    ['/api/threads?cursor=first-match', { threads: [project('third', firstTime - 80), project('first', firstTime - 50), project('main-old', firstTime - 20, 'main')], nextCursor: 'more' }],
    ['/api/threads?cursor=more', { threads: [project('second', (firstTime - 60) * 1000), project('fourth', firstTime - 120)], nextCursor: null }],
  ]);
  globalThis.fetch = async url => { f.calls.push(url); return Response.json(pages.get(url)); };
  await f.catalog.refresh();
  assert.equal(f.catalog.latestByProject.value.has('old'), false);
  await Promise.all([f.catalog.loadProjectHistory(), f.catalog.loadProjectHistory()]);
  assert.equal(f.calls.filter(url => url === '/api/threads?cursor=empty').length, 1);
  assert.equal(f.catalog.hasMoreProjectHistory.value, true);
  await f.catalog.loadProjectHistory();
  await f.catalog.loadProjectHistory();
  assert.deepEqual(f.catalog.latestByProject.value.get('old').map(thread => thread.id), ['first', 'second', 'third']);
  assert.deepEqual(f.catalog.latestByProject.value.get('main').map(thread => thread.id), ['newest', 'main-old']);
  assert.equal(f.catalog.knownThreads.value.some(thread => thread.id === 'fourth'), false);
  f.catalog.updateCatalogThread({ id: 'third', status: { type: 'active' } });
  assert.equal(threadActive(f.catalog.latestByProject.value.get('old')[2]), true);
  assert.equal(f.catalog.hasMoreProjectHistory.value, false);
});

test('前台持续打开时定期刷新，后台停止发请求，销毁后清理监听', async (t) => {
  t.mock.timers.enable({ apis: ['setInterval'] });
  const f = fixture(t);
  await f.catalog.refresh();
  f.publish();
  t.mock.timers.tick(30_000);
  await settle();
  assert.equal(f.catalog.threads.value[0].title, '最新 2');
  const count = f.calls.length;
  f.document.hidden = true;
  t.mock.timers.tick(60_000);
  await settle();
  assert.equal(f.calls.length, count);
  f.scope.stop();
  f.document.hidden = false;
  f.window.dispatchEvent(new Event('focus'));
  t.mock.timers.tick(30_000);
  await settle();
  assert.equal(f.calls.length, count);
});

test('多次恢复合并请求，刷新失败保留旧内容并允许重试', async (t) => {
  const f = fixture(t);
  await f.catalog.refresh();
  const previous = f.catalog.threads.value;
  const resolves = [];
  globalThis.fetch = async (url) => new Promise((resolve) => resolves.push([url, resolve]));
  const first = f.catalog.refresh();
  void f.catalog.refresh();
  f.window.dispatchEvent(new Event('online'));
  assert.equal(resolves.length, 2);
  for (const [, resolve] of resolves) resolve(Response.json({ message: '服务离线' }, { status: 503 }));
  await first;
  assert.equal(f.catalog.threads.value, previous);
  assert.equal(f.catalog.error.value, '服务离线');
  assert.equal(f.catalog.loadingThreads.value, false);
  globalThis.fetch = async () => Response.json({ threads: [{ id: 'recovered', title: '恢复' }], nextCursor: null, projects: [] });
  await f.catalog.refresh();
  assert.equal(f.catalog.error.value, '');
  assert.equal(f.catalog.threads.value[0].id, 'recovered');
});

test('登录失效清理目录，退出后的迟到响应不能恢复旧聊天', async (t) => {
  const f = fixture(t);
  await f.catalog.refresh();
  globalThis.fetch = async () => Response.json({ message: '连接过期' }, { status: 401 });
  await f.catalog.refresh();
  assert.equal(f.expired(), 1);
  assert.equal(f.catalog.threads.value.length, 0);
  f.authenticated.value = true;
  let resolve;
  globalThis.fetch = async () => new Promise((done) => { resolve = done; });
  const pending = f.catalog.loadThreads();
  f.authenticated.value = false;
  resolve(Response.json({ threads: [{ id: 'stale' }], nextCursor: 'old' }));
  await pending;
  assert.equal(f.catalog.threads.value.length, 0);
});

test('后台取消未完成列表请求，回前台立即请求最新内容而不等待旧响应', async (t) => {
  const f = fixture(t);
  await f.catalog.refresh();
  let resolve;
  let signal;
  globalThis.fetch = (_, options) => { signal = options.signal; return new Promise((done) => { resolve = done; }); };
  const previous = f.catalog.loadThreads();
  f.document.hidden = true;
  f.document.dispatchEvent(new Event('visibilitychange'));
  assert.ok(signal.aborted);
  globalThis.fetch = async () => Response.json({ threads: [{ id: 'latest' }], nextCursor: null, projects: [] });
  f.document.hidden = false;
  f.window.dispatchEvent(new Event('pageshow'));
  await settle();
  assert.equal(f.catalog.threads.value[0].id, 'latest');
  resolve(Response.json({ threads: [{ id: 'stale' }], nextCursor: 'old' }));
  await previous;
  assert.equal(f.catalog.threads.value[0].id, 'latest');
  assert.equal(f.catalog.loadingThreads.value, false);
});

test('项目预览回到后台时取消补页，迟到响应不会改写聊天或游标', async t => {
  const f = fixture(t);
  const baseFetch = globalThis.fetch;
  await f.catalog.refresh();
  let signal;
  let finish;
  globalThis.fetch = (_, options) => {
    signal = options.signal;
    return new Promise(resolve => { finish = resolve; });
  };
  const pending = f.catalog.loadProjectHistory();
  f.document.hidden = true;
  f.document.dispatchEvent(new Event('visibilitychange'));
  assert.equal(signal.aborted, true);
  assert.equal(f.catalog.loadingProjectHistory.value, false);
  finish(Response.json({ threads: [{ id: 'stale', projectId: 'project' }], nextCursor: null }));
  await pending;
  assert.equal(f.catalog.hasMoreProjectHistory.value, true);
  assert.deepEqual(f.catalog.latestByProject.value.get('project').map(thread => thread.id), ['recent']);
  globalThis.fetch = baseFetch;
  f.document.hidden = false;
  await f.catalog.loadProjectHistory();
  assert.equal(f.calls.at(-1), '/api/threads?cursor=page-1');
  assert.deepEqual(f.catalog.latestByProject.value.get('project').map(thread => thread.id), ['recent', 'older']);
});

test('最近工作按更新时间取五个，执行状态和置顶次序不改变最近顺序', () => {
  const seconds = 1_800_000_000;
  const rows = [
    { id: 'old-pin', pinnedIndex: 1, updatedAt: seconds - 200 },
    { id: 'old-active', status: 'active', updatedAt: seconds - 100 },
    { id: 'fifth', updatedAt: seconds - 5 },
    { id: 'first', updatedAt: seconds },
    { id: 'active', status: { type: 'active' }, updatedAt: seconds - 3 },
    { id: 'second', updatedAt: (seconds - 2) * 1000 },
    { id: 'fourth', updatedAt: seconds - 4 },
  ];
  const before = rows.map(row => row.id);
  assert.deepEqual(recentThreads(rows, 5).map(row => row.id), ['first', 'second', 'active', 'fourth', 'fifth']);
  assert.deepEqual(rows.map(row => row.id), before);
  rows[1].status = 'idle';
  rows[1].updatedAt = seconds + 1;
  assert.deepEqual(recentThreads(rows, 5).map(row => row.id), ['old-active', 'first', 'second', 'active', 'fourth']);
});

test('缺少更新时间的聊天排在已知时间之后，保持官方目录相对顺序', () => {
  const rows = [{ id: 'idle' }, { id: 'running', status: 'active' }, { id: 'newest', updatedAt: 1_800_000_000 }];
  assert.deepEqual(recentThreads(rows, 5).map(row => row.id), ['newest', 'idle', 'running']);
  assert.deepEqual(recentThreads([], 5), []);
});
