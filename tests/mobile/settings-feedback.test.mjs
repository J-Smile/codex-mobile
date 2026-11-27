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

test('官方委派未支持队列时只提交即时请求，队列能力消失保留任务', async t => {
  const f = fixture(t, 'composer/AgentDelegation.vue', { open: true, targets: [], initialTask: '检查页面', busy: false, unavailable: '', queueSupported: true });
  f.app.mode.value = 'queue';
  f.props.queueSupported = false;
  await vue.nextTick();
  assert.equal(f.app.mode.value, 'now');
  assert.equal(f.app.task.value, '检查页面');
  f.app.submit();
  assert.equal(f.events[0][1].mode, 'now');
  assert.match(f.events[0][1].prompt, /请创建一个子智能体/);
});

// UPDATE-001 原凭据保存/删除场景随入口移除迁移为只读失败恢复；不再保留写凭据能力。
test('公开更新状态读取失败可重试，保留暂停模式且不提交凭据', async t => {
  let resolveRequest, rejectRequest;
  const f = fixture(t, 'settings/UpdateSettings.vue', { open: false }, () => new Promise((resolve, reject) => { resolveRequest = resolve; rejectRequest = reject; }));
  const failed = f.app.load();
  assert.equal(f.app.loading.value, true);
  rejectRequest(new Error('无法连接电脑'));
  await failed;
  assert.equal(f.app.loading.value, false);
  assert.equal(f.app.status.value, undefined);
  assert.equal(f.app.error.value, '无法连接电脑');

  const retry = f.app.load();
  resolveRequest({ version: '0.3.29', mode: 'paused', update: { phase: 'idle' } });
  await retry;
  assert.equal(f.app.loading.value, false);
  assert.equal(f.app.error.value, '');
  assert.equal(f.app.status.value.mode, 'paused');
  assert.equal(f.app.canUpgrade.value, true);
  assert.ok(f.calls.every(call => call.url === '/api/update-settings' && !call.method && !call.body));
});

test('关闭或重新打开升级面板不接收旧版本读取的迟到结果', async t => {
  const pending = [];
  const f = fixture(t, 'settings/UpdateSettings.vue', { open: false }, () => new Promise(resolve => pending.push(resolve)));
  const first = f.app.load();
  const second = f.app.load();
  assert.equal(f.calls[0].signal.aborted, true);
  pending[1]({ version: '0.3.29', mode: 'automatic' });
  await second;
  pending[0]({ version: '0.3.28', mode: 'source' });
  await first;
  assert.equal(f.app.status.value.version, '0.3.29');
  f.props.open = true;
  await vue.nextTick();
  f.props.open = false;
  await vue.nextTick();
  assert.equal(f.calls.at(-1).signal.aborted, true);
  pending[2]({ version: '0.3.27', mode: 'automatic' });
  await vue.nextTick();
  assert.equal(f.app.status.value, undefined);
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
  f.app.status.value = { version: '0.3.8', mode: 'paused' };
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
