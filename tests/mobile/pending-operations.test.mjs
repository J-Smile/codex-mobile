import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from '../../apps/mobile/src/api.ts';
import { submissionFixture, emptyOperations } from '../fixtures/submission.mjs';

const id = n => 'operation-' + String(n).padStart(16, '0');
const saved = (target = 'chat', number = 1) => [target, { requestId: id(number) }];
const stored = f => JSON.parse(f.data.get('codex-mobile-pending-v2'));
const message = (target = 'chat', input = { prompt: '原消息', images: [], model: 'chosen' }) =>
  ({ target, kind: 'message', url: '/api/threads/' + target + '/messages', input });

test('刷新保存的操作身份只含目标和 ID，恢复后仍等待核对', async t => {
  const f = submissionFixture(t, () => ({ operations: [] }), [saved()]);
  await f.loadOperations();
  assert.deepEqual(stored(f), [saved()]);
  assert.deepEqual(f.unresolvedOperations.value, [{ id: id(1), target: 'chat', kind: 'message', phase: 'unknown', localOnly: true }]);
  assert.equal(f.operationsReady.value, false);
  assert.match(f.operationsError.value, /版本不支持/);
});

test('同一页面升级后端后用原ID恢复accepted未投递及已结案终态且不自动发送', async t => {
  const entries = ['accepted', 'not-dispatched', 'unknown'].map((phase, index) => ({
    id: id(index + 1), target: 'chat-' + index, kind: 'message', phase, closed: index === 2,
  }));
  const initial = entries.map(entry => [entry.target, { requestId: entry.id }]);
  let upgraded = false;
  const f = submissionFixture(t, call => {
    assert.equal(call.method, 'GET');
    const ids = new URL(call.url, 'http://localhost').searchParams.get('ids').split(',');
    assert.deepEqual(ids, entries.map(entry => entry.id));
    return upgraded ? { operations: [], results: entries } : { operations: [] };
  }, initial);
  await f.loadOperations();
  assert.equal(f.operationsReady.value, false);
  assert.match(f.operationsError.value, /版本不支持/);
  assert.deepEqual(stored(f), initial);
  assert.equal(f.unresolvedOperations.value.length, 3);
  assert.ok(f.unresolvedOperations.value.every(operation => operation.localOnly));

  upgraded = true;
  await f.loadOperations();
  assert.equal(f.calls.length, 2);
  assert.ok(f.calls.every(call => call.method === 'GET'));
  assert.deepEqual(stored(f), []);
  assert.deepEqual(f.unresolvedOperations.value, []);
  assert.equal(f.operationsReady.value, true);
  assert.equal(f.operationsError.value, '');
  for (const entry of entries) assert.equal(f.readiness({ target: entry.target, capable: true,
    connected: true, hasContent: true, uploading: false, imageErrors: false, imageCount: 0,
    imageSupported: true, voiceBusy: false }), '');
});

test('服务端未决与本地身份合并，查询失败禁止投递并保留未决项', async t => {
  let fail = false;
  const operation = { id: id(1), target: 'chat', kind: 'message', phase: 'dispatching' };
  const f = submissionFixture(t, () => {
    if (fail) throw new Error('offline');
    return { operations: [operation], results: [operation] };
  }, [saved()]);
  await f.loadOperations();
  assert.equal(f.unresolvedOperations.value.length, 1);
  assert.equal(f.unresolvedOperations.value[0].localOnly, undefined);
  fail = true;
  await f.loadOperations();
  assert.equal(f.operationsReady.value, false);
  assert.deepEqual(f.unresolvedOperations.value, [operation]);
  assert.match(f.operationsError.value, /阅读和草稿/);
});

test('人工结案只标记 unknown，不重发；进行中或失败结案保留身份', async t => {
  const previous = globalThis.window;
  globalThis.window = { confirm: () => true };
  t.after(() => { globalThis.window = previous; });
  const unknown = { id: id(1), target: 'chat', kind: 'message', phase: 'unknown' };
  let fail = true;
  const f = submissionFixture(t, call => {
    if (call.method === 'POST' && fail) throw new ApiError('结案失败', 503);
    return emptyOperations(call.url);
  }, [saved()]);
  await f.closeOperation({ ...unknown, phase: 'dispatching' });
  assert.deepEqual(f.calls, []);
  await f.closeOperation(unknown);
  assert.deepEqual(stored(f), [saved()]);
  assert.equal(f.operationsError.value, '结案失败');
  fail = false;
  await f.closeOperation(unknown);
  assert.deepEqual(stored(f), []);
  assert.deepEqual(f.calls.filter(call => call.method === 'POST'), Array(2).fill({ url: '/api/operations/' + id(1), method: 'POST', body: { confirm: 'close-unknown' } }));
  assert.deepEqual(f.unresolvedOperations.value, []);
});

test('终态解除旧等待；创建回执保留到查看原聊天，不额外保存正文', async t => {
  const entries = ['accepted', 'not-dispatched', 'unknown', 'accepted'].map((phase, n) => ({
    id: id(n + 1), target: n === 3 ? 'new:project' : 'chat-' + n, kind: n === 3 ? 'create' : 'message',
    phase, closed: n === 2, receipt: n === 3 ? { threadId: 'created', firstTurnAccepted: false } : {},
  }));
  const f = submissionFixture(t, () => ({ operations: [entries[0]], results: entries }), entries.map(entry => [entry.target, { requestId: entry.id }]));
  await f.loadOperations();
  assert.equal(f.operationsReady.value, true);
  assert.deepEqual(f.unresolvedOperations.value, []);
  assert.deepEqual(f.recoveredCreations.value, [entries[3]]);
  assert.deepEqual(stored(f), [['new:project', { requestId: id(4) }]]);
  f.acknowledgeCreation(f.recoveredCreations.value[0]);
  assert.deepEqual(stored(f), []);
  assert.deepEqual(f.recoveredCreations.value, []);
  assert.ok(f.calls.every(call => call.method === 'GET'));
});

test('missing 不能当作未发送；其他聊天仍可提交，原目标不能重复', async t => {
  const f = submissionFixture(t, call => call.method === 'GET' ? emptyOperations(call.url) : { accepted: true }, [saved()]);
  await f.loadOperations();
  assert.equal(f.operationsReady.value, true);
  assert.equal(f.unresolvedOperations.value[0].localOnly, true);
  await assert.rejects(f.submit(message()), /尚未确认/);
  await f.submit(message('other'));
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 1);
  assert.deepEqual(stored(f), [saved()]);
});

test('missing人工结案只移除本地身份且保留其他unknown，不发送结案或消息POST', async t => {
  const previous = globalThis.window;
  let confirmed = false;
  globalThis.window = { confirm: () => confirmed };
  t.after(() => { globalThis.window = previous; });
  const other = { id: id(2), target: 'other', kind: 'message', phase: 'unknown' };
  const initial = [saved('missing'), saved('other', 2)];
  const f = submissionFixture(t, call => {
    assert.equal(call.method, 'GET');
    const ids = new URL(call.url, 'http://localhost').searchParams.get('ids').split(',');
    return { operations: [other], results: ids.map(value => value === other.id ? other : { id: value, phase: 'missing' }) };
  }, initial);
  await f.loadOperations();
  const missing = f.unresolvedOperations.value.find(operation => operation.target === 'missing');
  assert.equal(missing.phase, 'unknown');
  assert.equal(missing.localOnly, true);
  await f.closeOperation(missing);
  assert.equal(f.calls.length, 1);
  assert.deepEqual(stored(f), initial);

  confirmed = true;
  await f.closeOperation(missing);
  assert.equal(f.calls.length, 2);
  assert.ok(f.calls.every(call => call.method === 'GET'));
  assert.equal(new URL(f.calls[1].url, 'http://localhost').searchParams.get('ids'), other.id);
  assert.deepEqual(stored(f), [saved('other', 2)]);
  assert.deepEqual(f.unresolvedOperations.value, [other]);
  assert.equal(f.operationsReady.value, true);
  assert.equal(f.operationsError.value, '');
  const readiness = { capable: true, connected: true, hasContent: true, uploading: false,
    imageErrors: false, imageCount: 0, imageSupported: true, voiceBusy: false };
  assert.equal(f.readiness({ ...readiness, target: 'missing' }), '');
  assert.match(f.readiness({ ...readiness, target: 'other' }), /尚未确认/);
});

test('保存超过100个ID时分批查询且不重复提示', async t => {
  const initial = Array.from({ length: 205 }, (_, n) => saved('chat-' + n, n));
  const f = submissionFixture(t, call => emptyOperations(call.url), initial);
  await f.loadOperations();
  assert.deepEqual(f.calls.map(call => new URL(call.url, 'http://local').searchParams.get('ids').split(',').length), [100, 100, 5]);
  assert.equal(f.unresolvedOperations.value.length, 205);
  assert.equal(new Set(f.unresolvedOperations.value.map(entry => entry.id)).size, 205);
});

test('同目标并发禁止重复，跨目标独立且冻结本次文字图片模型', async t => {
  const finish = [];
  const f = submissionFixture(t, call => call.method === 'GET' ? emptyOperations(call.url) : new Promise(resolve => finish.push(resolve)));
  f.operationsReady.value = true;
  const input = { prompt: '原文', images: ['image-a'], model: 'original' };
  const one = f.submit(message('one', input));
  input.prompt = '之后编辑'; input.images.push('image-b'); input.model = 'later';
  await assert.rejects(f.submit(message('one')), /发送/);
  const two = f.submit(message('two'));
  assert.deepEqual(f.calls[0].body.images, ['image-a']);
  assert.equal(f.calls[0].body.prompt, '原文');
  assert.equal(f.calls[0].body.model, 'original');
  assert.deepEqual(stored(f).map(entry => Object.keys(entry[1])), [['requestId'], ['requestId']]);
  finish[1]({ accepted: true }); await two;
  assert.equal(f.pendingSends.has('one'), true);
  finish[0]({ accepted: true }); await one;
  assert.equal(f.pendingSends.size, 0);
  assert.deepEqual(stored(f), []);
});

test('只接受明确未投递的失败恢复重试；HTTP错误码不推断结果', async t => {
  let outcome = 'unknown';
  const f = submissionFixture(t, call => {
    if (call.method === 'GET') return emptyOperations(call.url);
    throw new ApiError('请求被拒绝', 400, outcome);
  });
  f.operationsReady.value = true;
  await assert.rejects(f.submit(message()), /请求被拒绝/);
  assert.equal(stored(f).length, 1);
  await assert.rejects(f.submit(message()), /尚未确认/);
  outcome = 'not-dispatched';
  await assert.rejects(f.submit(message('other')), /请求被拒绝/);
  assert.equal(stored(f).length, 1);
  assert.equal(f.unresolvedOperations.value.some(value => value.target === 'other'), false);
});

test('旧查询迟到不能恢复已接受消息的等待', async t => {
  let resolveQuery;
  const f = submissionFixture(t, call => {
    if (call.method !== 'GET') return { accepted: true };
    if (!resolveQuery) return new Promise(resolve => { resolveQuery = resolve; });
    return { operations: [] };
  });
  f.operationsReady.value = true;
  const query = f.loadOperations();
  await f.submit(message());
  resolveQuery({ operations: [{ id: id(9), target: 'chat', kind: 'message', phase: 'dispatching' }] });
  await query;
  assert.deepEqual(f.unresolvedOperations.value, []);
  assert.deepEqual(stored(f), []);
});

for (const queryFails of [false, true]) {
  test(`跨聊天成功回执打断${queryFails ? '失败' : '成功'}核对后仍恢复原操作终态且不重发`, async t => {
    let rejectFirst, finishSecond, finishQuery, announceQuery;
    let reads = 0;
    const queryStarted = new Promise(resolve => { announceQuery = resolve; });
    const f = submissionFixture(t, call => {
      if (call.method === 'GET') {
        reads++;
        if (reads === 1) {
          announceQuery();
          return new Promise((resolve, reject) => { finishQuery = queryFails ? reject : resolve; });
        }
        const requestId = f.calls.find(value => value.url.includes('/first/')).body.requestId;
        return { operations: [], results: [{ id: requestId, target: 'first', kind: 'message', phase: 'accepted' }] };
      }
      if (call.url.includes('/first/')) return new Promise((_, reject) => { rejectFirst = reject; });
      return new Promise(resolve => { finishSecond = resolve; });
    });
    f.operationsReady.value = true;
    const first = assert.rejects(f.submit(message('first')), /回执丢失/);
    const second = f.submit(message('second'));
    rejectFirst(new ApiError('回执丢失', 503, 'unknown'));
    await queryStarted;
    finishSecond({ accepted: true });
    await second;
    const requestId = f.calls.find(call => call.url.includes('/first/')).body.requestId;
    finishQuery(queryFails ? new ApiError('旧核对失败', 503)
      : { operations: [], results: [{ id: requestId, target: 'first', kind: 'message', phase: 'accepted' }] });
    await first;
    assert.equal(reads, 2);
    assert.equal(f.calls.filter(call => call.method === 'POST').length, 2);
    assert.deepEqual(stored(f), []);
    assert.deepEqual(f.unresolvedOperations.value, []);
    assert.equal(f.operationsError.value, '');
    assert.equal(f.operationsReady.value, true);
    assert.equal(f.readiness({ target: 'first', capable: true, connected: true, hasContent: true,
      uploading: false, imageErrors: false, imageCount: 0, imageSupported: true, voiceBusy: false }), '');
  });
}

test('新核对接管后旧查询不能续接或覆盖当前未决结果', async t => {
  let finishOld;
  let reads = 0;
  const f = submissionFixture(t, call => {
    if (call.method !== 'GET') return { accepted: true };
    reads++;
    if (reads === 1) return new Promise(resolve => { finishOld = resolve; });
    return emptyOperations(call.url);
  }, [saved()]);
  f.operationsReady.value = true;
  const old = f.loadOperations();
  await f.submit(message('other'));
  await f.loadOperations();
  const current = structuredClone(f.unresolvedOperations.value.map(value => ({ ...value })));
  finishOld({ operations: [], results: [{ id: id(1), target: 'chat', kind: 'message', phase: 'accepted' }] });
  await old;
  assert.equal(reads, 2);
  assert.deepEqual(f.unresolvedOperations.value, current);
  assert.equal(current[0].localOnly, true);
  assert.deepEqual(stored(f), [saved()]);
  assert.equal(f.calls.filter(call => call.method === 'POST').length, 1);
});

for (const queryFails of [false, true]) {
  test(`注销后旧核对${queryFails ? '失败' : '成功'}不能续接或重新置就绪`, async t => {
    let finishQuery;
    const f = submissionFixture(t, () => new Promise((resolve, reject) => {
      finishQuery = queryFails ? reject : resolve;
    }), [saved()]);
    f.operationsReady.value = true;
    const query = f.loadOperations();
    f.invalidate();
    finishQuery(queryFails ? new ApiError('迟到的失败', 503)
      : { operations: [], results: [{ id: id(1), target: 'chat', kind: 'message', phase: 'accepted' }] });
    await query;
    assert.equal(f.calls.length, 1);
    assert.equal(f.operationsReady.value, false);
    assert.equal(f.operationsError.value, '');
    assert.deepEqual(f.unresolvedOperations.value, []);
    assert.deepEqual(stored(f), [saved()]);
  });
}

test('卸载撤销等待，迟到回执不清持久身份或写入其他电脑', async t => {
  let finish;
  const f = submissionFixture(t, () => new Promise(resolve => { finish = resolve; }));
  f.operationsReady.value = true;
  const pending = f.submit(message());
  f.scope.stop();
  finish({ accepted: true });
  await assert.rejects(pending);
  assert.equal(stored(f).length, 1);
  assert.deepEqual([...f.data.keys()], ['codex-mobile-pending-v2']);
});

test('保存身份失败时不发网络请求且不遗留发送中状态', async t => {
  const f = submissionFixture(t, () => assert.fail('不能投递'));
  f.operationsReady.value = true;
  globalThis.localStorage.setItem = () => { throw new Error('QuotaExceeded'); };
  await assert.rejects(f.submit(message()), /尚未发送/);
  assert.equal(f.pendingSends.size, 0);
  assert.deepEqual(f.calls, []);
});

test('统一就绪结果区分附件、语音、首轮和连接，纯图片可发送', t => {
  const f = submissionFixture(t, () => ({}));
  f.operationsReady.value = true;
  const ready = { target: 'chat', capable: true, connected: true, hasContent: true, uploading: false,
    imageErrors: false, imageCount: 1, imageSupported: true, voiceBusy: false };
  assert.equal(f.readiness(ready), '');
  assert.match(f.readiness({ ...ready, uploading: true }), /正在上传/);
  assert.match(f.readiness({ ...ready, imageErrors: true }), /重试或移除/);
  assert.match(f.readiness({ ...ready, imageSupported: false }), /不支持图片/);
  assert.match(f.readiness({ ...ready, connected: false }), /连接恢复/);
  assert.match(f.readiness({ ...ready, firstTurnPending: true }), /首轮/);
  assert.match(f.readiness({ ...ready, voiceBusy: true }), /语音/);
});

test('认证失效后旧发送回执不覆盖重登后的核对和新请求', async t => {
  const finish = [];
  const f = submissionFixture(t, call => {
    if (call.method === 'GET') return { operations: [], results: new URL(call.url, 'http://localhost').searchParams.get('ids').split(',').map(id => ({ id, target: 'chat', kind: 'message', phase: 'accepted' })) };
    return new Promise(resolve => finish.push(resolve));
  });
  f.operationsReady.value = true;
  const old = f.submit(message());
  const oldRejected = assert.rejects(old, { name: 'AbortError' });
  f.invalidate();
  assert.equal(f.operationsReady.value, false);
  assert.equal(stored(f).length, 1);
  await f.loadOperations();
  const current = f.submit(message());
  const currentIdentity = stored(f);
  finish[0]({ accepted: true });
  await oldRejected;
  assert.deepEqual(stored(f), currentIdentity);
  assert.equal(f.pendingSends.has('chat'), true);
  finish[1]({ accepted: true });
  await current;
  assert.deepEqual(stored(f), []);
});

test('认证失效后的旧核对结果不能置就绪或覆盖新会话', async t => {
  const finish = [];
  const f = submissionFixture(t, () => new Promise(resolve => finish.push(resolve)), [['chat', { requestId: id(1) }]]);
  const old = f.loadOperations();
  f.invalidate();
  const current = f.loadOperations();
  finish[0]({ operations: [], results: [{ id: id(1), target: 'chat', kind: 'message', phase: 'accepted' }] });
  await old;
  assert.equal(f.operationsReady.value, false);
  assert.equal(stored(f).length, 1);
  finish[1]({ operations: [], results: [{ id: id(1), phase: 'missing' }] });
  await current;
  assert.equal(f.unresolvedOperations.value[0].localOnly, true);
  assert.equal(stored(f).length, 1);
});
