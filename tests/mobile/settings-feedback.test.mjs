import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { compileScript, parse } from '@vue/compiler-sfc';
import ts from 'typescript';
import * as vue from 'vue';
import { delegationPrompt } from '../../apps/mobile/src/features/composer/subagentDelegation.ts';

function fixture(t, file, initialProps, request = async () => ({})) {
  const source = readFileSync(new URL('../../apps/mobile/src/features/' + file, import.meta.url), 'utf8');
  const script = compileScript(parse(source).descriptor, { id: 'settings-feedback' }).content;
  const compiled = ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
  const module = { exports: {} }, cleanup = [], events = [], calls = [];
  new Function('require', 'module', 'exports', compiled)(name => name === 'vue'
    ? { ...vue, onBeforeUnmount(callback) { cleanup.push(callback); } }
    : name === './subagentDelegation' ? { delegationPrompt }
    : name === '../../api' ? { request: (url, options) => { calls.push({ url, ...options }); return request(url, options); } }
    : {}, module, module.exports);
  const props = vue.reactive(initialProps);
  const scope = vue.effectScope();
  const app = scope.run(() => module.exports.default.setup(props, { expose() {}, emit: (...args) => events.push(args) }));
  t.after(() => { cleanup.forEach(callback => callback()); scope.stop(); });
  return { props, app, events, calls };
}

test('unavailable delegation keeps the task and target editable, then submits after recovery', async t => {
  const target = { id: 'child', name: '检查', path: '/root/check', status: '已完成' };
  const f = fixture(t, 'composer/AgentDelegation.vue', { open: true, targets: [target], initialTask: '初始任务', busy: false, unavailable: '连接恢复后可委派任务' });
  f.app.task.value = '继续检查手机页面';
  f.app.targetId.value = target.id;
  f.app.mode.value = 'queue';
  f.app.submit();
  assert.deepEqual(f.events, []);
  assert.equal(f.app.canSubmit.value, false);

  f.props.unavailable = '图片上传完成后可委派任务';
  await vue.nextTick();
  f.app.submit();
  assert.deepEqual(f.events, []);
  f.props.unavailable = '';
  await vue.nextTick();
  assert.equal(f.app.canSubmit.value, true);
  f.app.submit();
  assert.equal(f.events[0][0], 'submit');
  assert.deepEqual(f.events[0][1], { prompt: delegationPrompt('继续检查手机页面', target), mode: 'queue', target });
});

test('update settings retain failed input and distinguish verification from removal', async t => {
  let resolveRequest, rejectRequest;
  const f = fixture(t, 'settings/UpdateSettings.vue', { open: false }, () => new Promise((resolve, reject) => { resolveRequest = resolve; rejectRequest = reject; }));
  f.app.token.value = 'fixture-token';
  f.app.reveal.value = true;
  const failed = f.app.save();
  assert.equal(f.app.busy.value, 'saving');
  rejectRequest(new Error('无法连接 GitHub'));
  await failed;
  assert.equal(f.app.busy.value, '');
  assert.equal(f.app.token.value, 'fixture-token');
  assert.equal(f.app.error.value, '无法连接 GitHub');

  const saved = f.app.save();
  resolveRequest({ configured: true, source: 'file', version: '0.2.1', mode: 'paused' });
  await saved;
  assert.equal(f.app.token.value, '');
  assert.equal(f.app.reveal.value, false);
  assert.equal(f.app.notice.value, 'Token 已验证并保存。');
  assert.equal(f.app.status.value.mode, 'paused');

  f.app.confirmRemove.value = true;
  const removed = f.app.remove();
  assert.equal(f.app.busy.value, 'removing');
  assert.equal(f.calls.at(-1).method, 'DELETE');
  resolveRequest({ configured: false, source: null, version: '0.2.1', mode: 'paused' });
  await removed;
  assert.equal(f.app.busy.value, '');
  assert.equal(f.app.confirmRemove.value, false);
  assert.equal(f.app.status.value.configured, false);
  assert.equal(f.app.notice.value, 'Token 已移除，继续使用免 token 更新。');
});

test('手机免 token 升级只提交一次，失败保留面板，接受请求后关闭面板', async t => {
  const versions = [];
  let resolveStart, rejectStart;
  const f = fixture(t, 'settings/UpdateSettings.vue', {
    open: false,
    upgrading: false,
    startUpgrade: version => new Promise((resolve, reject) => {
      versions.push(version);
      resolveStart = resolve;
      rejectStart = reject;
    }),
  });
  f.app.status.value = { configured: false, source: null, version: '0.3.8', mode: 'paused' };
  await f.app.upgrade();
  assert.deepEqual(versions, []);

  f.app.status.value.update = { phase: 'idle', message: '' };
  const failed = f.app.upgrade();
  await f.app.upgrade();
  assert.equal(f.app.busy.value, 'starting');
  assert.deepEqual(versions, ['0.3.8']);
  rejectStart(new Error('电脑暂时无法准备更新。'));
  await failed;
  assert.deepEqual(f.events, []);
  assert.equal(f.app.error.value, '电脑暂时无法准备更新。');

  const retry = f.app.upgrade();
  resolveStart();
  await retry;
  assert.deepEqual(f.events, [['close']]);
  assert.equal(f.app.error.value, '');
  f.app.status.value.mode = 'source';
  assert.equal(f.app.canUpgrade.value, false);
});
