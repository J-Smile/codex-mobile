import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope, nextTick, ref } from 'vue';
import { useBrowserTheme } from '../../apps/mobile/src/browserChrome.ts';

test('网页主题跟随页面和系统，恢复前台后同步并保留手动选择', async t => {
  const originals = new Map(['document', 'window', 'localStorage', 'getComputedStyle']
    .map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const root = { dataset: {} };
  const meta = new Map();
  const media = Object.assign(new EventTarget(), { matches: true });
  const stored = new Map();
  globalThis.window = Object.assign(new EventTarget(), { matchMedia: () => media });
  globalThis.localStorage = { getItem: key => stored.get(key), setItem: (key, value) => stored.set(key, value) };
  globalThis.document = Object.assign(new EventTarget(), {
    hidden: false, documentElement: root,
    querySelector: selector => ({ setAttribute: (_, value) => meta.set(selector, value) }),
  });
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => root.dataset.theme === 'dark'
    ? root.dataset.surface === 'navigation' ? '#151517' : '#1c1c1e'
    : root.dataset.surface === 'navigation' ? '#f7f7f8' : '#fff' });
  const scope = effectScope();
  t.after(() => {
    scope.stop();
    for (const [key, original] of originals) {
      if (original) Object.defineProperty(globalThis, key, original);
      else delete globalThis[key];
    }
  });
  const surface = ref('navigation');
  const theme = scope.run(() => useBrowserTheme(() => surface.value));
  assert.equal(meta.get('meta[name="theme-color"]'), '#151517');
  assert.equal(meta.get('meta[name="color-scheme"]'), 'dark');
  assert.equal(stored.size, 0);
  surface.value = 'chat';
  await nextTick();
  assert.equal(meta.get('meta[name="theme-color"]'), '#1c1c1e');

  document.hidden = true;
  media.matches = false;
  media.dispatchEvent(new Event('change'));
  await nextTick();
  assert.equal(theme.value, 'dark');
  document.hidden = false;
  document.dispatchEvent(new Event('visibilitychange'));
  await nextTick();
  assert.equal(theme.value, 'light');
  assert.equal(meta.get('meta[name="theme-color"]'), '#fff');
  assert.equal(meta.get('meta[name="color-scheme"]'), 'light');

  theme.value = 'dark';
  await nextTick();
  window.dispatchEvent(new Event('pageshow'));
  await nextTick();
  assert.equal(stored.get('codex-mobile-theme'), 'dark');
  assert.equal(theme.value, 'dark');
  assert.equal(meta.get('meta[name="theme-color"]'), '#1c1c1e');
});
