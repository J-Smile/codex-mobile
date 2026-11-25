import assert from 'node:assert/strict';
import test from 'node:test';
import { setImmediate as settle } from 'node:timers/promises';
import { effectScope, ref } from 'vue';
import Noise from 'noise-handshake';
import Cipher from 'noise-handshake/cipher.js';
import { createPhoneIdentity, RelayConnection } from '../../mobile/src/features/relay/connection.ts';
import { useRelayLatency } from '../../mobile/src/features/relay/useRelayLatency.ts';

const origin = 'http://localhost';

test('电脑延迟排除握手，兼容新版 204 和旧版完整 JSON 响应', async t => {
  for (const status of [204, 200]) {
    await t.test(String(status), async t => {
      const identity = new Noise('IK', false).s;
      const handshake = new Noise('IK', false, identity);
      handshake.initialise(Buffer.from('codex-mobile-relay/v1'));
      const connection = new RelayConnection({
        id: 'a'.repeat(64), name: '隔离电脑', publicKey: identity.publicKey.toString('hex'), phone: createPhoneIdentity(),
      }, origin);
      const { promise: pingReceived, resolve: received } = Promise.withResolvers();
      let now = 0;
      let downstream;
      let incoming;
      let outgoing;
      let complete;
      t.mock.method(performance, 'now', () => now);
      t.after(() => connection.close());

      const send = (frame) => {
        const batch = Buffer.alloc(2 + frame.length);
        batch.writeUInt16BE(frame.length);
        Buffer.from(frame).copy(batch, 2);
        downstream.enqueue(Buffer.from('event: frame\ndata: ' + batch.toString('base64url') + '\n\n'));
      };
      function record(kind, request, data = '') {
        const bytes = Buffer.alloc(5 + Buffer.byteLength(data));
        bytes[0] = kind;
        bytes.writeUInt32BE(request, 1);
        bytes.write(data, 5);
        send(outgoing.encrypt(bytes));
      }
      t.mock.method(globalThis, 'fetch', async (url, options) => {
        if (url.pathname.endsWith('/session')) {
          const stream = new ReadableStream({
            start(controller) {
              downstream = controller;
              downstream.enqueue(Buffer.from('event: ready\ndata: ' + 'f'.repeat(32) + '\n\n'));
            },
          });
          return new Response(stream, { headers: { 'content-type': 'text/event-stream' } });
        }
        const payload = Buffer.from(options.body).subarray(2);
        if (!incoming) {
          now += 500;
          handshake.recv(payload);
          send(handshake.send());
          incoming = new Cipher(handshake.rx);
          outgoing = new Cipher(handshake.tx);
        } else {
          const message = incoming.decrypt(payload);
          const id = message.readUInt32BE(1);
          if (message[0] === 1) {
            now += 200;
            record(2, 0, 'ok');
          } else {
            assert.equal(message[0], 3);
            const { path, method, length } = JSON.parse(message.subarray(5));
            assert.deepEqual({ path, method, length }, { path: '/api/session?ping=1', method: 'GET', length: 0 });
            now += 47;
            record(6, id, JSON.stringify({ status, headers: {} }));
            if (status === 200) {
              complete = () => {
                now += 19;
                record(7, id, '{"authenticated":true}');
                record(8, id);
              };
            } else record(8, id);
            received();
          }
        }
        return new Response(null, { status: 204 });
      });

      let finished = false;
      const sample = connection.measureLatency(new AbortController().signal).then(value => { finished = true; return value; });
      await pingReceived;
      await settle();
      if (status === 200) {
        assert.equal(finished, false);
        complete();
      }
      assert.equal(await sample, status === 200 ? 66 : 47);
    });
  }
});

test('电脑列表按需测量，限制两台并发，关闭或切后台时保留聊天隧道', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const previousLocation = globalThis.location;
  globalThis.location = new URL(origin);
  const scope = effectScope();
  t.after(() => {
    scope.stop();
    if (previousLocation === undefined) delete globalThis.location;
    else globalThis.location = previousLocation;
  });
  const account = ref();
  const desktops = ['work', 'home', 'studio'].map(id => ({ id, name: id }));
  const active = new RelayConnection(desktops[0], origin);
  const probes = [];
  const closed = new Set();
  let healthRequests = 0;
  let maxRunning = 0;
  let running = 0;

  t.mock.getter(RelayConnection.prototype, 'hasTunnel', () => true);
  t.mock.method(RelayConnection.prototype, 'close', function () { closed.add(this); });
  t.mock.method(RelayConnection.prototype, 'measureLatency', function (signal) {
    maxRunning = Math.max(maxRunning, ++running);
    return new Promise((resolve, reject) => {
      signal.addEventListener('abort', () => reject(signal.reason), { once: true });
      probes.push({ connection: this, signal, resolve });
    }).finally(() => { running--; });
  });
  t.mock.method(globalThis, 'fetch', (url, options) => {
    assert.equal(url, '/relay/health');
    assert.equal(options.cache, 'no-store');
    assert.equal(options.credentials, 'omit');
    return Promise.resolve(new Response(null, { status: ++healthRequests === 1 ? 204 : 503 }));
  });

  const { serverLatency, desktopLatency } = scope.run(() => useRelayLatency({
    visibleAccount: () => account.value,
    desktops: () => desktops,
    activeConnection: id => id === 'work' ? active : undefined,
  }));
  assert.equal(healthRequests, 0);
  assert.equal(probes.length, 0);

  account.value = 'fixture-account';
  await settle();
  assert.equal(healthRequests, 1);
  assert.ok(serverLatency.value >= 0);
  assert.equal(probes.length, 2);
  assert.equal(probes[0].connection, active);
  probes[0].resolve(47);
  probes[1].resolve(82);
  await settle();
  assert.equal(probes.length, 3);
  assert.equal(desktopLatency.get('work'), 47);
  assert.equal(desktopLatency.get('home'), 82);
  probes[2].resolve(61);
  await settle();
  assert.equal(desktopLatency.get('studio'), 61);

  t.mock.timers.tick(10_000);
  await settle();
  assert.equal(healthRequests, 2);
  assert.equal(serverLatency.value, null);
  assert.equal(probes.length, 5);
  account.value = undefined;
  await settle();
  assert.ok(probes[3].signal.aborted && probes[4].signal.aborted);
  assert.equal(maxRunning, 2);
  assert.ok(closed.has(probes[4].connection));
  assert.ok(!closed.has(active));
  assert.equal(desktopLatency.get('home'), 82);

  t.mock.timers.tick(30_000);
  await settle();
  assert.equal(healthRequests, 2);
  assert.equal(probes.length, 5);
});

test('电脑探测超时释放临时隧道并显示不可测', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const previousLocation = globalThis.location;
  globalThis.location = new URL(origin);
  const scope = effectScope();
  t.after(() => {
    scope.stop();
    if (previousLocation === undefined) delete globalThis.location;
    else globalThis.location = previousLocation;
  });
  let closeCalls = 0;
  t.mock.method(RelayConnection.prototype, 'close', () => { closeCalls++; });
  t.mock.method(RelayConnection.prototype, 'measureLatency', signal =>
    new Promise((_, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true })));
  t.mock.method(globalThis, 'fetch', () => Promise.resolve(new Response(null, { status: 204 })));
  const { desktopLatency } = scope.run(() => useRelayLatency({
    visibleAccount: () => 'fixture-account',
    desktops: () => [{ id: 'slow', name: '慢速电脑' }],
    activeConnection: () => undefined,
  }));
  assert.equal(desktopLatency.get('slow'), undefined);
  t.mock.timers.tick(5_000);
  await settle();
  assert.equal(desktopLatency.get('slow'), null);
  assert.equal(closeCalls, 1);
});
