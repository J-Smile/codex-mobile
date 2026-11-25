import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import { renderToString } from 'vue/server-renderer';

function compileSetup(descriptor, id) {
  const script = compileScript(descriptor, { id }).content;
  return ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
}
const source = readFileSync(new URL('../../mobile/src/components/NativeSheet.vue', import.meta.url), 'utf8');
const compiled = compileSetup(parse(source).descriptor, 'native-sheet-test');
const spring = Symbol('Motion.spring');

function fixture(t, { reduced = false, compact = true, keyboard = false, deferFrame = false } = {}) {
  const events = [], animations = [], cleanup = [];
  const viewport = Object.assign(new EventTarget(), { width: 390, height: keyboard ? 504 : 844 });
  const window = Object.assign(new EventTarget(), { innerWidth: 390, innerHeight: 844, visualViewport: viewport });
  const document = { documentElement: { clientHeight: 844 } };
  let frameId = 0;
  const frames = new Map();
  const requestFrame = callback => {
    const id = ++frameId;
    frames.set(id, callback);
    if (!deferFrame) queueMicrotask(() => { if (frames.delete(id)) callback(0); });
    return id;
  };
  function surface() {
    return vue.markRaw({
      style: { transform: 'none', opacity: '1' }, heightReads: 0, animation: null,
      get offsetHeight() { this.heightReads++; return 500; },
      querySelector() { return null; }, contains() { return false; },
    });
  }
  // 模拟 Motion 的公开边界：停止保留当前位置，也可以单独送达旧动画的完成通知。
  function animateMini(element, keyframes, options) {
    const { promise, resolve } = Promise.withResolvers();
    const current = Object.fromEntries(Object.entries(keyframes).map(([key, values]) => [key, values[0]]));
    const animation = {
      keyframes, options, current, stopped: false, finished: promise,
      stop() {
        this.stopped = true;
        if (element.animation !== this) return;
        for (const [key, value] of Object.entries(current)) element.style[key] = String(value);
        element.animation = null;
      },
      complete() {
        if (element.animation === this) {
          for (const [key, values] of Object.entries(keyframes)) element.style[key] = String(values.at(-1));
          element.animation = null;
        }
        resolve();
      },
    };
    element.animation = animation;
    animations.push(animation);
    return animation;
  }
  const panel = surface(), shade = surface();
  const dialog = { open: false, shown: 0, closed: 0, showModal() { this.open = true; this.shown++; }, close() { if (this.open) { this.open = false; this.closed++; } } };
  const handle = { captured: false, setPointerCapture() { this.captured = true; }, hasPointerCapture() { return this.captured; }, releasePointerCapture() { this.captured = false; } };
  const props = vue.reactive({ open: false, labelledby: 'sheet-title', panelClass: '', dismissible: true });
  const module = { exports: {} };
  new Function('require', 'module', 'exports', 'matchMedia', 'getComputedStyle', 'DOMMatrixReadOnly', 'window', 'document', 'requestAnimationFrame', 'cancelAnimationFrame', compiled)(
    name => name === 'motion-v' ? { animateMini, spring }
      : { ...vue, onMounted(callback) { callback(); }, onBeforeUnmount(callback) { cleanup.push(callback); } }, module, module.exports,
    query => ({ matches: query.includes('reduced-motion') ? reduced : compact }),
    element => ({ ...element.style, ...element.animation?.current }),
    class { constructor(transform) { this.m42 = Number(/translate3d\(0,\s*([\d.]+)px/.exec(transform)?.[1] || 0); } },
    window, document, requestFrame, id => frames.delete(id),
  );
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit(name) { events.push(name); if (name === 'close') props.open = false; } }));
  app.dialog.value = dialog;
  app.panel.value = panel;
  app.shade.value = shade;
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  const pointer = (clientY, timeStamp, clientX = 100) => ({ pointerId: 1, isPrimary: true, button: 0, currentTarget: handle, clientX, clientY, timeStamp, detail: 1 });
  return { app, props, panel, shade, dialog, handle, events, get animations() { return animations.filter(animation => animation === panel.animation || animation === shade.animation); }, pointer, viewport, window, document,
    async paint() { const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback(0)); await settled(); },
    async open() { props.open = true; await settled(); },
    async finish() { this.animations.forEach(animation => animation.complete()); await settled(); },
  };
}
async function settled() { await vue.nextTick(); await setImmediate(); await vue.nextTick(); }

test('closing retains the native modal until its exit finishes, then releases it', async t => {
  const sheet = fixture(t);
  await sheet.open();
  assert.equal(sheet.dialog.open, true);
  assert.equal(sheet.app.present.value, true);
  sheet.props.open = false;
  await settled();
  assert.equal(sheet.dialog.open, true);
  assert.deepEqual(sheet.panel.animation.options, { duration: .2, ease: 'easeIn' });
  assert.equal(sheet.shade.animation.options.duration, .2);
  assert.equal(sheet.panel.heightReads, 0);
  sheet.shade.animation.complete();
  await settled();
  assert.equal(sheet.dialog.open, true);
  await sheet.finish();
  assert.equal(sheet.dialog.open, false);
  assert.equal(sheet.app.present.value, false);
  assert.deepEqual(sheet.events, ['after-close']);
});

test('normal and reduced-motion exits restore the opener only after native close, before after-close', async t => {
  for (const reduced of [false, true]) {
    const sheet = fixture(t, { reduced });
    const restored = [];
    const opener = {
      isConnected: true,
      getClientRects: () => [{}],
      focus(options) {
        restored.push({ options, dialogOpen: sheet.dialog.open, events: [...sheet.events] });
        sheet.document.activeElement = this;
      },
    };
    sheet.document.activeElement = opener;
    await sheet.open();
    await sheet.finish();
    sheet.document.activeElement = sheet.dialog;
    const nativeClose = sheet.dialog.close.bind(sheet.dialog);
    sheet.dialog.close = () => { nativeClose(); sheet.document.activeElement = {}; };
    sheet.props.open = false;
    await settled();
    assert.deepEqual(restored, []);
    await sheet.finish();
    assert.equal(sheet.document.activeElement, opener);
    assert.deepEqual(restored, [{ options: { preventScroll: true }, dialogOpen: false, events: [] }]);
    assert.deepEqual(sheet.events, ['after-close']);
  }
});

test('closing does not restore an opener removed or hidden while the sheet was open', async t => {
  for (const hidden of [false, true]) {
    const sheet = fixture(t);
    let restored = false;
    const opener = { isConnected: true, getClientRects: () => [{}], focus() { restored = true; } };
    sheet.document.activeElement = opener;
    await sheet.open();
    await sheet.finish();
    if (hidden) opener.getClientRects = () => [];
    else opener.isConnected = false;
    sheet.props.open = false;
    await settled();
    await sheet.finish();
    assert.equal(restored, false);
    assert.deepEqual(sheet.events, ['after-close']);
  }
});

test('a fresh sheet prepares its entry before becoming visible and focuses only after it arrives', async t => {
  const sheet = fixture(t);
  const stages = [], focus = [];
  const showModal = sheet.dialog.showModal.bind(sheet.dialog);
  sheet.dialog.showModal = () => { stages.push({ ready: sheet.app.ready.value, animations: sheet.animations.length }); showModal(); };
  sheet.panel.querySelector = selector => selector.startsWith('[autofocus]') ? { focus: options => focus.push({ options, transform: sheet.panel.style.transform }) } : null;
  await sheet.open();
  assert.deepEqual(stages, [{ ready: false, animations: 0 }]);
  assert.equal(sheet.app.ready.value, true);
  assert.deepEqual(sheet.panel.animation.keyframes.transform, ['translateY(calc(100% + 24px))', 'none']);
  assert.deepEqual(sheet.panel.animation.options, { type: spring, duration: .3, bounce: 0 });
  assert.equal(sheet.panel.heightReads, 0);
  assert.deepEqual(focus, []);
  await sheet.finish();
  assert.deepEqual(focus, [{ options: { preventScroll: true }, transform: 'none' }]);
  sheet.props.open = false;
  await settled();
  await sheet.finish();
  assert.equal(sheet.app.ready.value, false);
});

test('entry completion preserves a field the user already focused in the sheet', async t => {
  const sheet = fixture(t);
  const field = {}, handle = {};
  sheet.panel.contains = target => target === field;
  sheet.panel.querySelector = () => ({ focus() { sheet.document.activeElement = handle; } });
  await sheet.open();
  sheet.document.activeElement = field;
  await sheet.finish();
  assert.equal(sheet.document.activeElement, field);
});

test('closing during entry cancels its pending autofocus and a later opening can focus normally', async t => {
  const sheet = fixture(t);
  const focus = [];
  sheet.panel.querySelector = selector => selector.startsWith('[autofocus]') ? { focus: options => focus.push(options) } : null;
  await sheet.open();
  sheet.panel.animation.current.transform = 'translate3d(0, 115px, 0)';
  sheet.shade.animation.current.opacity = .5;
  sheet.props.open = false;
  await settled();
  assert.equal(sheet.panel.animation.keyframes.transform[0], 'translate3d(0, 115px, 0)');
  assert.deepEqual(sheet.shade.animation.keyframes.opacity, [.5, 0]);
  await sheet.finish();
  assert.deepEqual(focus, []);
  assert.equal(sheet.dialog.open, false);
  await sheet.open();
  await sheet.finish();
  assert.deepEqual(focus, [{ preventScroll: true }]);
});

test('keyboard dismissal also waits when innerHeight and the layout viewport shrink with the keyboard', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const sheet = fixture(t, { deferFrame: true });
  const resize = height => {
    sheet.window.innerHeight = height;
    sheet.document.documentElement.clientHeight = height;
    sheet.viewport.height = height;
    sheet.viewport.dispatchEvent(new Event('resize'));
    sheet.window.dispatchEvent(new Event('resize'));
  };
  resize(504);
  await sheet.open();
  t.mock.timers.tick(700);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, false);
  for (const height of [550, 640, 760, 844]) {
    resize(height);
    t.mock.timers.tick(100);
    await sheet.paint();
    assert.equal(sheet.animations.length, 1);
  }
  t.mock.timers.tick(20);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, true);
  assert.equal(sheet.animations.length, 2);
});

test('rotating a closed sheet resets the viewport reference before a normal landscape entry', async t => {
  const sheet = fixture(t, { deferFrame: true });
  sheet.viewport.width = sheet.window.innerWidth = 844;
  sheet.viewport.height = sheet.window.innerHeight = sheet.document.documentElement.clientHeight = 390;
  sheet.viewport.dispatchEvent(new Event('resize'));
  await sheet.open();
  await sheet.paint();
  assert.equal(sheet.app.ready.value, true);
  assert.equal(sheet.animations.length, 2);
});

test('keyboard dismissal settles before entry, and a hidden preparation can close without leaving a modal', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const sheet = fixture(t, { keyboard: true, deferFrame: true });
  await sheet.open();
  assert.equal(sheet.dialog.open, true);
  assert.equal(sheet.app.ready.value, false);
  assert.equal(sheet.animations.length, 1);
  sheet.viewport.height = 700;
  sheet.viewport.dispatchEvent(new Event('resize'));
  t.mock.timers.tick(60);
  sheet.viewport.height = 844;
  sheet.viewport.dispatchEvent(new Event('resize'));
  t.mock.timers.tick(119);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, false);
  t.mock.timers.tick(1);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, true);
  assert.equal(sheet.animations.length, 2);
  await sheet.finish();
  sheet.props.open = false;
  await settled();
  await sheet.finish();

  sheet.viewport.height = 504;
  await sheet.open();
  sheet.props.open = false;
  await settled();
  assert.equal(sheet.dialog.open, false);
  assert.equal(sheet.app.ready.value, false);
  t.mock.timers.tick(600);
  await sheet.paint();
  assert.equal(sheet.dialog.open, false);
  assert.equal(sheet.app.present.value, false);
});

test('keyboard preparation acknowledges the click with a cancellable shade before the panel appears', async t => {
  const sheet = fixture(t, { keyboard: true, deferFrame: true });
  await sheet.open();
  assert.equal(sheet.app.ready.value, false);
  assert.equal(sheet.animations.length, 1);
  assert.equal(sheet.shade.animation.options.duration, .15);
  assert.deepEqual(sheet.shade.animation.keyframes, { opacity: [0, 1] });
  assert.equal(sheet.panel.animation, null);
  sheet.app.pressedBackdrop = true;
  sheet.app.backdropClick();
  await settled();
  assert.equal(sheet.dialog.open, false);
  assert.equal(sheet.animations.length, 0);
  assert.deepEqual(sheet.events, ['close', 'after-close']);
});

test('panel entry continues the visible preparation shade without resetting its opacity', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const sheet = fixture(t, { keyboard: true, deferFrame: true });
  await sheet.open();
  const feedback = sheet.shade.animation;
  feedback.current.opacity = .7;
  sheet.viewport.height = 844;
  sheet.viewport.dispatchEvent(new Event('resize'));
  t.mock.timers.tick(120);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, true);
  assert.ok(sheet.panel.animation);
  assert.equal(sheet.shade.animation, feedback);
  assert.equal(feedback.current.opacity, .7);
});

test('reopening during exit cancels stale completion and queued native close events', async t => {
  const sheet = fixture(t);
  await sheet.open();
  await sheet.finish();
  sheet.props.open = false;
  await settled();
  const exiting = sheet.animations;
  sheet.panel.animation.current.transform = 'translate3d(0, 80px, 0)';
  sheet.shade.animation.current.opacity = .42;
  await sheet.open();
  assert.ok(exiting.every(animation => animation.stopped));
  assert.equal(sheet.panel.animation.keyframes.transform[0], 'translate3d(0, 80px, 0)');
  assert.equal(sheet.shade.animation.keyframes.opacity[0], .42);
  exiting.forEach(animation => animation.complete());
  sheet.app.nativeClosed();
  await sheet.finish();
  assert.equal(sheet.dialog.open, true);
  assert.equal(sheet.dialog.closed, 0);
  assert.equal(sheet.app.present.value, true);
  assert.deepEqual(sheet.events, []);
});

test('late and slow keyboard dismissal never starts a sheet against a moving bottom edge', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const sheet = fixture(t, { keyboard: true, deferFrame: true });
  await sheet.open();
  t.mock.timers.tick(700);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, false);
  for (const height of [550, 640, 760, 844]) {
    sheet.viewport.height = height;
    sheet.viewport.dispatchEvent(new Event('resize'));
    t.mock.timers.tick(100);
    await sheet.paint();
    assert.equal(sheet.animations.length, 1);
  }
  t.mock.timers.tick(20);
  await sheet.paint();
  assert.equal(sheet.app.ready.value, true);
  assert.equal(sheet.animations.length, 2);
});

test('open then close before rendering never leaves a modal in the top layer', async t => {
  const sheet = fixture(t);
  sheet.props.open = true;
  sheet.props.open = false;
  await settled();
  assert.equal(sheet.dialog.open, false);
  assert.equal(sheet.app.present.value, false);
});

test('a short drag returns home and its synthesized click does not dismiss', async t => {
  const sheet = fixture(t);
  await sheet.open();
  await sheet.finish();
  sheet.app.beginDrag(sheet.pointer(100, 0));
  sheet.app.moveDrag(sheet.pointer(140, 600));
  assert.equal(sheet.shade.style.opacity, '0.92');
  sheet.app.endDrag(sheet.pointer(140, 620));
  assert.deepEqual(sheet.panel.animation.keyframes.transform, ['translate3d(0, 40px, 0)', 'none']);
  assert.equal(sheet.panel.animation.options.type, spring);
  assert.deepEqual(sheet.shade.animation.keyframes.opacity, [.92, 1]);
  assert.equal(sheet.panel.heightReads, 1);
  sheet.app.handleClick({ detail: 1 });
  await sheet.finish();
  assert.equal(sheet.handle.captured, false);
  assert.equal(sheet.panel.style.transform, 'none');
  assert.equal(sheet.dialog.open, true);
  assert.deepEqual(sheet.events, []);
  sheet.app.handleClick({ detail: 0 });
  assert.deepEqual(sheet.events, ['close']);
});

test('接手正在入场的面板时，位移和遮罩都从手指按下的位置继续', async t => {
  const sheet = fixture(t);
  await sheet.open();
  sheet.panel.animation.current.transform = 'translate3d(0, 90px, 0)';
  sheet.shade.animation.current.opacity = .5;
  sheet.app.beginDrag(sheet.pointer(100, 0));
  sheet.app.moveDrag(sheet.pointer(130, 200));
  assert.equal(sheet.panel.style.transform, 'translate3d(0, 120px, 0)');
  assert.equal(sheet.shade.style.opacity, '0.44');
  sheet.app.endDrag(sheet.pointer(130, 220), true);
  assert.equal(sheet.panel.animation.keyframes.transform[0], 'translate3d(0, 120px, 0)');
  assert.equal(sheet.shade.animation.keyframes.opacity[0], .44);
  await sheet.finish();
  assert.equal(sheet.dialog.open, true);
});

test('distance or a recent downward flick dismisses; a paused flick does not', async t => {
  for (const [distance, moveTime, releaseTime, dismissed] of [[140, 500, 510, true], [40, 40, 55, true], [40, 40, 200, false]]) {
    const sheet = fixture(t);
    await sheet.open();
    await sheet.finish();
    sheet.app.beginDrag(sheet.pointer(100, 0));
    sheet.app.moveDrag(sheet.pointer(100 + distance, moveTime));
    sheet.app.endDrag(sheet.pointer(100 + distance, releaseTime));
    assert.equal(sheet.events.includes('close'), dismissed);
    assert.equal(sheet.handle.captured, false);
  }
});

test('horizontal or mostly horizontal handle drags do not become close clicks', async t => {
  for (const distance of [2, 120]) {
    const sheet = fixture(t);
    await sheet.open();
    await sheet.finish();
    sheet.app.beginDrag(sheet.pointer(100, 0, 100));
    sheet.app.moveDrag(sheet.pointer(100 + distance, 100, 300));
    sheet.app.endDrag(sheet.pointer(100 + distance, 120, 300));
    sheet.app.handleClick({ detail: 1 });
    await sheet.finish();
    assert.equal(sheet.panel.style.transform, 'none');
    assert.deepEqual(sheet.events, []);
  }
});

test('cancelled or reversed drags return home without dismissing the sheet', async t => {
  for (const cancelled of [true, false]) {
    const sheet = fixture(t);
    await sheet.open();
    await sheet.finish();
    sheet.app.beginDrag(sheet.pointer(100, 0));
    sheet.app.moveDrag(sheet.pointer(300, 100));
    if (!cancelled) sheet.app.moveDrag(sheet.pointer(120, 150));
    sheet.app.endDrag(sheet.pointer(cancelled ? 300 : 120, 160), cancelled);
    sheet.app.handleClick({ detail: 1 });
    await sheet.finish();
    assert.equal(sheet.panel.style.transform, 'none');
    assert.deepEqual(sheet.events, []);
  }
});

test('busy sheets reject gesture and Escape dismissal but still close when controlled by the parent', async t => {
  const sheet = fixture(t);
  await sheet.open();
  await sheet.finish();
  sheet.props.dismissible = false;
  sheet.app.beginDrag(sheet.pointer(100, 0));
  sheet.app.moveDrag(sheet.pointer(300, 100));
  sheet.app.endDrag(sheet.pointer(300, 120));
  sheet.app.requestClose();
  assert.deepEqual(sheet.events, []);
  assert.equal(sheet.handle.captured, false);
  sheet.props.open = false;
  await settled();
  await sheet.finish();
  assert.equal(sheet.dialog.open, false);
});

test('reduced motion keeps opacity feedback and removes entry and exit movement', async t => {
  const sheet = fixture(t, { reduced: true });
  await sheet.open();
  for (const open of [true, false]) {
    sheet.props.open = open;
    await settled();
    for (const animation of [sheet.panel.animation, sheet.shade.animation]) {
      assert.deepEqual(animation.keyframes, { opacity: open ? [0, 1] : [1, 0] });
      assert.equal(animation.options.duration, .1);
    }
    assert.equal(sheet.panel.style.transform, 'none');
    assert.equal(sheet.panel.heightReads, 0);
    await sheet.finish();
  }
  assert.equal(sheet.dialog.open, false);
});

const permissionTemplate = parse(readFileSync(new URL('../../mobile/src/features/settings/PermissionSettings.vue', import.meta.url), 'utf8')).descriptor;
const permissionSetup = compileSetup(permissionTemplate, 'sheet-permissions');
function permissionFixture(t) {
  const requests = [], cleanup = [];
  const api = { threadUrl: (id, action) => '/threads/' + id + '/' + action, request(url, options) {
    const request = { url, ...options, ...Promise.withResolvers() };
    requests.push(request);
    return request.promise;
  } };
  const module = { exports: {} };
  new Function('require', 'module', 'exports', permissionSetup)(name => name === 'vue'
    ? { ...vue, useId: () => 'permission-test', onBeforeUnmount: fn => cleanup.push(fn) }
    : name === '../../api' ? api : {}, module, module.exports);
  const props = vue.reactive({ thread: { id: 'sample', permissionProfile: ':danger-full-access' }, connected: true, active: false, newThread: false });
  const scope = vue.effectScope();
  const state = scope.run(() => module.exports.default.setup(props, { expose() {} }));
  t.after(() => { cleanup.forEach(fn => fn()); scope.stop(); });
  const icon = () => vue.h('svg');
  const view = { ...module.exports.default, template: permissionTemplate.template.content, setup: () => ({ ...state }),
    components: { NativeSheet: (_, { slots }) => slots.default(), Check: icon, Close: icon, Loading: icon, Lock: icon, Warning: icon } };
  return { props, state, requests, async open() { state.open.value = true; await settled(); }, async render() {
    const html = await renderToString(vue.createSSRApp(view, props));
    return { html, radios: html.match(/<input type="radio"[^>]*>/g) ?? [], footer: html.match(/<footer[^>]*>(.*?)<\/footer>/s)?.[1] };
  } };
}

test('权限读取时三行和按钮已就位，真实配置到达后才能选择或保存', async t => {
  const f = permissionFixture(t);
  await f.open();
  const waiting = await f.render();
  assert.deepEqual(waiting.radios.map(tag => /value="([^"]+)"/.exec(tag)[1]), ['read-only', 'workspace-write', 'full-access']);
  assert.ok(waiting.radios.every(tag => tag.includes('disabled') && !tag.includes('checked')));
  assert.match(waiting.footer, /type="submit"[^>]*disabled.*正在读取权限/s);
  f.state.selected.value = 'full-access';
  await f.state.apply();
  assert.equal(f.requests.length, 1);
  f.requests[0].resolve({ permissions: { mode: 'read-only', profile: ':read-only' } });
  await settled();
  const loaded = await f.render();
  assert.equal(loaded.radios.length, 3);
  assert.ok(loaded.radios.every(tag => !tag.includes('disabled')));
  assert.deepEqual(loaded.radios.filter(tag => tag.includes('checked')), [loaded.radios[0]]);
  assert.match(loaded.footer, /disabled.*应用权限/s);
  f.state.selected.value = 'workspace-write';
  assert.doesNotMatch((await f.render()).footer, /disabled/);
  const saving = f.state.apply();
  assert.equal(f.requests[1].method, 'POST');
  assert.deepEqual(JSON.parse(f.requests[1].body), { permissions: 'workspace-write' });
  f.requests[1].resolve({ applied: true });
  await saving;
  const exiting = await f.render();
  assert.equal(f.state.open.value, false);
  assert.match(exiting.radios[1], /checked/);
});

test('无有效权限配置时保留禁用的选项并可重试，自定义权限不被预选为标准选项', async t => {
  const f = permissionFixture(t);
  await f.open();
  f.requests[0].resolve({ permissions: {} });
  await settled();
  const failed = await f.render();
  assert.match(failed.html, /role="alert"[^>]*>未能读取当前权限/);
  assert.equal(failed.radios.length, 3);
  assert.ok(failed.radios.every(tag => tag.includes('disabled') && !tag.includes('checked')));
  assert.match(failed.footer, /type="button"[^>]*>重新读取权限/);
  const retry = f.state.readSettings();
  assert.match((await f.render()).footer, /disabled.*正在读取权限/s);
  assert.equal(f.requests[1].url, f.requests[0].url);
  f.requests[1].resolve({ permissions: { mode: 'custom', profile: 'team-policy' } });
  await retry;
  const custom = await f.render();
  assert.match(custom.html, /当前使用自定义权限：team-policy/);
  assert.ok(custom.radios.every(tag => !tag.includes('checked') && !tag.includes('disabled')));
  assert.match(custom.footer, /disabled.*应用权限/s);
});

test('切换对话会取消读取并保持退场内容，旧响应不能选中重开后的权限', async t => {
  const f = permissionFixture(t);
  await f.open();
  const first = f.requests[0];
  const waiting = await f.render();
  f.props.newThread = true;
  await settled();
  assert.ok(first.signal.aborted);
  assert.equal(f.state.open.value, false);
  const exiting = await f.render();
  assert.deepEqual(exiting.radios, waiting.radios);
  assert.equal(exiting.footer, waiting.footer);
  await f.open();
  const defaults = await f.render();
  assert.deepEqual(defaults.radios, []);
  assert.match(defaults.footer, /知道了/);
  assert.equal(f.requests.length, 1);
  f.props.newThread = false;
  await settled();
  await f.open();
  first.resolve({ permissions: { mode: 'full-access' } });
  await settled();
  assert.deepEqual((await f.render()).radios, waiting.radios);
  f.requests[1].reject(new Error('连接中断'));
  await settled();
  assert.match((await f.render()).footer, /重新读取权限/);
});
