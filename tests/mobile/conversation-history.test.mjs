import test from 'node:test';
import assert from 'node:assert/strict';
import { createRenderer, effectScope, h, isReactive, nextTick, ref } from 'vue';
import { useConversationDisclosure, useConversationHistory, useConversationItemDetail } from '../../apps/mobile/src/features/conversation/useConversationHistory.ts';

function fixture(t) {
  const originalWindow = globalThis.window;
  const window = new EventTarget();
  let index = 0;
  const entries = [{ url: 'https://example.test/?thread=parent&project=work', state: { retained: true } }];
  const move = (position) => {
    if (position < 0 || position >= entries.length) return;
    index = position;
    window.location = new URL(entries[index].url);
    window.dispatchEvent(new Event('popstate'));
  };
  window.location = new URL(entries[0].url);
  window.history = {
    get state() { return entries[index].state; },
    replaceState(state, _, url) { entries[index] = { state, url }; window.location = new URL(url); },
    pushState(state, _, url) { entries.splice(index + 1); entries.push({ state, url }); index++; window.location = new URL(url); },
    back() { move(index - 1); },
    forward() { move(index + 1); },
  };
  globalThis.window = window;
  const parentTurns = [{ id: 'older-page', items: [] }, { id: 'current', items: [] }];
  let current = { threadId: 'parent', projectId: 'work', thread: { id: 'parent', title: '父对话' }, turns: parentTurns,
    historyCursor: 'older-cursor', scrollTop: 582, stickToBottom: false, newOutput: true };
  const activations = [];
  const scope = effectScope();
  const navigation = scope.run(() => useConversationHistory({
    capture: () => current,
    activate: (threadId, projectId, saved) => {
      activations.push({ threadId, projectId, saved });
      current = saved || { threadId, projectId, thread: { id: threadId, title: threadId }, turns: [], historyCursor: null,
        scrollTop: 0, stickToBottom: true, newOutput: false };
    },
  }));
  t.after(() => { scope.stop(); if (originalWindow === undefined) delete globalThis.window; else globalThis.window = originalWindow; });
  return { navigation, window, entries, activations, parentTurns, scope, current: () => current, setCurrent: (value) => { current = value; } };
}

test('子对话页内打开并返回父级，保留已读历史页、游标和滚动位置', (t) => {
  const f = fixture(t);
  f.navigation.openChild('child');
  assert.equal(f.current().threadId, 'child');
  assert.equal(f.window.location.searchParams.get('thread'), 'child');
  assert.equal(f.window.history.state.retained, true);
  assert.equal(f.navigation.parentTitle.value, '父对话');
  assert.equal(f.navigation.canGoBack.value, true);
  assert.equal(f.navigation.back(), true);
  assert.equal(f.current().threadId, 'parent');
  assert.equal(f.current().turns, f.parentTurns);
  assert.equal(f.current().historyCursor, 'older-cursor');
  assert.equal(f.current().scrollTop, 582);
  assert.equal(f.current().stickToBottom, false);
  assert.equal(f.current().newOutput, true);
  assert.equal(f.navigation.canGoBack.value, false);
  assert.equal(f.navigation.back(), false);
});

test('多层子对话与浏览器前进后退恢复各自阅读状态', (t) => {
  const f = fixture(t);
  f.navigation.openChild('child');
  f.setCurrent({ ...f.current(), scrollTop: 231, stickToBottom: false });
  f.navigation.openChild('grandchild');
  f.window.history.back();
  assert.equal(f.current().threadId, 'child');
  assert.equal(f.current().scrollTop, 231);
  assert.equal(f.navigation.canGoBack.value, true);
  f.window.history.back();
  assert.equal(f.current().threadId, 'parent');
  f.window.history.forward();
  assert.equal(f.current().threadId, 'child');
  assert.equal(f.current().scrollTop, 231);
  f.window.history.forward();
  assert.equal(f.current().threadId, 'grandchild');
  assert.equal(f.navigation.canGoBack.value, true);
});

test('列表重新选择根聊天会结束父级栈，同一聊天不重复压栈', (t) => {
  const f = fixture(t);
  f.navigation.openChild('parent');
  assert.equal(f.entries.length, 1);
  f.navigation.openChild('child');
  f.navigation.reset();
  assert.equal(f.navigation.canGoBack.value, false);
  assert.equal(f.navigation.back(), false);
  assert.equal(f.entries.length, 2);
});

test('未缓存的浏览器历史按 URL 恢复，已销毁页面不再处理返回', (t) => {
  const f = fixture(t);
  f.navigation.openChild('child');
  f.window.history.pushState(null, '', 'https://example.test/?thread=external&project=elsewhere');
  f.window.dispatchEvent(new Event('popstate'));
  assert.equal(f.current().threadId, 'external');
  assert.equal(f.current().projectId, 'elsewhere');
  assert.equal(f.activations.at(-1).saved, undefined);
  f.scope.stop();
  const count = f.activations.length;
  f.window.history.back();
  assert.equal(f.activations.length, count);
});

test('退出连接清理临时阅读状态，浏览器返回不会取回旧会话缓存', (t) => {
  const f = fixture(t);
  f.navigation.openChild('child');
  f.navigation.clear();
  assert.equal(f.navigation.canGoBack.value, false);
  f.window.history.back();
  assert.equal(f.activations.at(-1).saved, undefined);
});

test('子对话返回保留执行分组展开与已读详情，输出缓存保持浅引用', async (t) => {
  let app;
  t.after(() => app?.unmount());
  const f = fixture(t);
  f.scope.stop();
  const createNode = (type) => ({ type, children: [], parent: null });
  const renderer = createRenderer({
    createElement: createNode, createText: createNode, createComment: createNode,
    insert: (node, parent) => { node.parent = parent; parent.children.push(node); },
    remove: (node) => { node.parent.children = node.parent.children.filter((entry) => entry !== node); },
    parentNode: (node) => node.parent, nextSibling: () => null,
    setElementText() {}, setText() {}, patchProp() {},
  });
  const active = ref('parent');
  const views = new Map();
  let navigation;
  const View = {
    props: ['threadId'],
    setup(props) {
      const expanded = useConversationDisclosure(() => props.threadId + ':activity');
      const detail = useConversationItemDetail(() => props.threadId + ':tool');
      views.set(props.threadId, { expanded, detail });
      return () => h('div');
    },
  };
  app = renderer.createApp({
    setup() {
      navigation = useConversationHistory({
        capture: f.current,
        activate: (threadId, projectId, saved) => {
          f.setCurrent(saved || { ...f.current(), threadId, projectId, turns: [] });
          active.value = threadId;
        },
      });
      return () => h(View, { key: active.value, threadId: active.value });
    },
  });
  app.mount(createNode('root'));
  const output = { id: 'tool', type: 'commandExecution', aggregatedOutput: '完整输出'.repeat(100) };
  views.get('parent').expanded.value = true;
  views.get('parent').detail.save(output, 'v1');
  navigation.openChild('child');
  await nextTick();
  assert.equal(views.get('child').expanded.value, undefined);
  assert.equal(views.get('child').detail.detail.value, undefined);
  navigation.back();
  await nextTick();
  assert.equal(views.get('parent').expanded.value, true);
  assert.equal(views.get('parent').detail.detail.value, output);
  assert.equal(views.get('parent').detail.version.value, 'v1');
  assert.equal(isReactive(views.get('parent').detail.detail.value), false);
});
