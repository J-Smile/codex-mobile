import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope, nextTick } from 'vue';
import { useDesktopUpgrade } from '../../apps/mobile/src/features/settings/useDesktopUpgrade.ts';

const settle = async () => { await new Promise(resolve => setImmediate(resolve)); await nextTick(); };

function fixture(t, respond) {
  const original = new Map(['document', 'fetch'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const calls = [], finished = [];
  globalThis.document = { hidden: false, querySelector: () => null };
  globalThis.fetch = async (url, options) => {
    const call = { url, method: options.method || 'GET', signal: options.signal };
    calls.push(call);
    return respond(call);
  };
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] });
  const scope = effectScope();
  const upgrade = scope.run(() => useDesktopUpgrade(result => finished.push(result)));
  t.after(() => {
    scope.stop();
    for (const [key, descriptor] of original) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  return { scope, upgrade, calls, finished, poll: async () => { t.mock.timers.tick(2000); await settle(); } };
}

test('UPDATE-002 下载校验解压共用checking，ready不误报中断，版本回读才确认升级', async t => {
  let update = { phase: 'checking', stage: 'checking', message: '正在检查正式版本' };
  let version = '0.3.28';
  const f = fixture(t, call => Response.json(call.method === 'POST' ? { accepted: true } : { version, update }));
  await f.upgrade.start(version);
  await settle();
  await f.upgrade.start(version);
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 1);
  for (const stage of ['downloading', 'verifying', 'extracting']) {
    update = { phase: 'checking', stage, message: stage, downloadedBytes: 1048576, totalBytes: null, elapsedMs: 2000, stageTimingsMs: { checking: 100 } };
    await f.poll();
    assert.equal(f.upgrade.upgrading.value, true);
    assert.deepEqual(f.upgrade.progress.value, update);
    assert.deepEqual(f.finished, []);
  }
  update = { phase: 'ready', message: '更新已通过校验' };
  await f.poll();
  assert.equal(f.upgrade.upgrading.value, true);
  assert.deepEqual(f.finished, []);
  update = { phase: 'applying', message: '正在切换版本' };
  await f.poll();
  assert.equal(f.upgrade.reconnecting.value, true);
  version = '0.3.29';
  await f.poll();
  assert.deepEqual(f.finished, [{ message: '已更新到 v0.3.29。', failed: false, updated: true }]);
  assert.equal(f.upgrade.upgrading.value, false);
  assert.equal(f.upgrade.progress.value, undefined);
});

test('UPDATE-003 旧桌面checking仍可等待，失败释放状态后允许用户重试', async t => {
  let update = { phase: 'checking', message: '旧桌面正在准备' };
  const f = fixture(t, call => Response.json(call.method === 'POST' ? { accepted: true } : { version: '0.3.28', update }));
  await f.upgrade.start('0.3.28');
  await settle();
  assert.equal(f.upgrade.progress.value.stage, undefined);
  assert.equal(f.upgrade.upgrading.value, true);
  update = { phase: 'error', message: '安装包校验失败' };
  await f.poll();
  assert.deepEqual(f.finished, [{ message: '安装包校验失败', failed: true, updated: false }]);
  update = { phase: 'current', message: '当前已是最新正式版本。' };
  await f.upgrade.start('0.3.28');
  await settle();
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 2);
  assert.equal(f.finished.at(-1).failed, false);
  assert.equal(f.upgrade.upgrading.value, false);
});

test('UPDATE-006 丢失升级提交回执只查状态，卸载后迟到查询不更新页面也不重发', async t => {
  let finish;
  const f = fixture(t, call => {
    if (call.method === 'POST') throw new TypeError('lost response');
    return new Promise(resolve => { finish = resolve; });
  });
  await f.upgrade.start('0.3.28');
  await settle();
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 1);
  const read = f.calls.find(call => call.method === 'GET');
  f.scope.stop();
  assert.equal(read.signal.aborted, true);
  finish(Response.json({ version: '0.3.29', update: { phase: 'current' } }));
  await settle();
  await f.poll();
  assert.deepEqual(f.finished, []);
  assert.equal(f.calls.length, 2);
  assert.equal(f.upgrade.progress.value, undefined);
});

test('UPDATE-006 重新配对要求结束升级等待，不把认证失败当作安装成功', async t => {
  const f = fixture(t, call => call.method === 'POST'
    ? Response.json({ accepted: true })
    : Response.json({ message: 'expired' }, { status: 401 }));
  await f.upgrade.start('0.3.28');
  await settle();
  assert.deepEqual(f.finished, [{ message: '连接已过期，请重新配对。', failed: true, updated: false }]);
  assert.equal(f.upgrade.upgrading.value, false);
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 1);
});
