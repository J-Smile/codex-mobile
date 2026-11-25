import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const html = readFileSync(new URL('../../apps/mobile/index.html', import.meta.url), 'utf8');
const script = readFileSync(new URL('../../shared/public/startup.js', import.meta.url), 'utf8');
function fixture(saved, dark) {
  const listeners = new Map(), fetched = [];
  const status = { textContent: '正在打开…', setAttribute(name, value) { this[name] = value; } };
  const retry = { hidden: true };
  const meta = { color: {}, scheme: {} };
  const root = { dataset: {} };
  let reloads = 0;
  const context = {
    document: { documentElement: root, querySelector: selector => selector.includes('theme-color') ? meta.color : meta.scheme,
      getElementById: id => id === 'startup-status' ? status : retry,
      querySelectorAll: () => [{ src: '/assets/entry.js' }, { href: '/assets/page.js' }, { href: '/assets/page.css' }] },
    localStorage: { getItem: () => saved }, matchMedia: () => ({ matches: dark }),
    addEventListener: (name, callback) => listeners.set(name, callback),
    location: { reload: () => reloads++ }, HTMLScriptElement: class {},
    fetch: async (url, options) => { fetched.push({ url, ...options }); return { ok: true, arrayBuffer: async () => new ArrayBuffer(0) }; },
  };
  vm.runInNewContext(script, context);
  return { root, meta, status, retry, listeners, fetched, reloads: () => reloads, Script: context.HTMLScriptElement, context };
}

test('页面模块加载前使用已选主题，无选择时跟随系统', () => {
  const selected = fixture('light', true);
  assert.equal(selected.root.dataset.theme, 'light');
  assert.equal(selected.meta.color.content, '#ffffff');
  const system = fixture(null, true);
  assert.equal(system.root.dataset.theme, 'dark');
  assert.equal(system.meta.color.content, '#1c1c1e');
  assert.equal(system.meta.scheme.content, 'dark');
  assert.doesNotMatch(html, /name="apple-mobile-web-app-status-bar-style"/);
  assert.equal(system.retry.hidden, true);
});

test('入口或页面模块失败显示手动恢复，重新读取声明资源后才重开', async () => {
  for (const entryFailure of [true, false]) {
    const f = fixture('light', false);
    if (entryFailure) {
      const target = new f.Script();
      target.type = 'module';
      f.listeners.get('error')({ target });
    } else f.listeners.get('codex-startup-failed')();
    assert.match(f.status.textContent, /页面未能打开/);
    assert.equal(f.status.role, 'alert');
    assert.equal(f.retry.hidden, false);
    assert.equal(f.reloads(), 0);
    await f.retry.onclick();
    assert.equal(f.reloads(), 1);
    assert.deepEqual(f.fetched.map(entry => entry.cache), ['reload', 'reload', 'reload']);
  }
});

test('手动恢复仍离线时保留错误与重试，不刷新循环', async () => {
  const f = fixture('light', false);
  f.context.fetch = async () => { throw new Error('offline'); };
  f.listeners.get('codex-startup-failed')();
  await f.retry.onclick();
  assert.equal(f.retry.disabled, false);
  assert.match(f.status.textContent, /网络仍不可用/);
  assert.equal(f.reloads(), 0);
});

test('手动恢复等待资源正文下载完成，收到响应头时不提前重开', async () => {
  const f = fixture('light', false);
  let finish;
  const body = new Promise(resolve => { finish = resolve; });
  f.context.fetch = async () => ({ ok: true, arrayBuffer: () => body });
  f.listeners.get('codex-startup-failed')();
  const loading = f.retry.onclick();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.retry.disabled, true);
  assert.equal(f.reloads(), 0);
  finish(new ArrayBuffer(0));
  await loading;
  assert.equal(f.reloads(), 1);
});
