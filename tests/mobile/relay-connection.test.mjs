import assert from 'node:assert/strict';
import test from 'node:test';
import Noise from 'noise-handshake';
import Cipher from 'noise-handshake/cipher.js';
import { createPhoneIdentity, RelayConnection } from '../../apps/mobile/src/features/relay/connection.ts';
import { readSse, SseReadError } from '../../apps/mobile/src/sse.ts';
import { RelayError } from '../../apps/mobile/src/transport.ts';

const origin = 'http://localhost';
const transportError = { message: /无法通过云中转连接电脑/ };
const integrityError = { message: /加密连接校验失败/ };
const packetTypes = {
  auth: 1, authenticated: 2, request: 3, response: 6, responseData: 7, responseEnd: 8,
};
const networkReset = () => new TypeError('fixture network reset');

function relay(t, { startup, disconnect } = {}) {
  const desktop = new Noise('IK', false).s;
  const connection = new RelayConnection({
    id: 'a'.repeat(64), publicKey: desktop.publicKey.toString('hex'), name: '隔离电脑', phone: createPhoneIdentity(),
  }, origin);
  const requests = [];
  let attempts = 0;
  let session;
  t.after(() => connection.close());

  t.mock.method(globalThis, 'fetch', async (url, options) => {
    if (url.pathname.endsWith('/session')) {
      const attempt = ++attempts;
      if (attempt === 1 && startup === 'fetch-rejection') throw networkReset();
      if (attempt === 1 && startup === 'html-response') {
        return new Response('<p>Proxy response</p>', { headers: { 'content-type': 'text/html' } });
      }
      if (attempt === 1 && startup === 'ready-eof') {
        return new Response('', { headers: { 'content-type': 'text/event-stream' } });
      }
      const handshake = new Noise('IK', false, desktop);
      handshake.initialise(Buffer.from('codex-mobile-relay/v1'));
      const stream = new ReadableStream({
        start(controller) {
          session = { attempt, handshake, controller };
          const id = 'c'.repeat(32);
          controller.enqueue(Buffer.from('event: ready\ndata: ' + id + '\n\n'));
        },
      });
      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } });
    }

    const current = session;
    const { attempt, handshake, controller } = current;
    function sendFrame(encrypted) {
      const batch = Buffer.alloc(2 + encrypted.length);
      batch.writeUInt16BE(encrypted.length);
      Buffer.from(encrypted).copy(batch, 2);
      controller.enqueue(Buffer.from('event: frame\ndata: ' + batch.toString('base64url') + '\n\n'));
    }
    function send(kind, id, value = '') {
      const message = Buffer.alloc(5 + Buffer.byteLength(value));
      message[0] = kind;
      message.writeUInt32BE(id, 1);
      message.write(value, 5);
      sendFrame(current.outgoing.encrypt(message));
    }

    const batch = Buffer.from(options.body);
    for (let start = 0; start < batch.length;) {
      const end = start + 2 + batch.readUInt16BE(start);
      const encrypted = batch.subarray(start + 2, end);
      start = end;
      if (!current.incoming) {
        assert.equal(handshake.recv(encrypted).length, 0);
        if (attempt === 1 && startup === 'handshake-read-rejection') controller.error(networkReset());
        else if (attempt === 1 && startup === 'handshake-eof') controller.close();
        else {
          const response = Buffer.from(handshake.send());
          if (attempt === 1 && startup === 'handshake-tampering') response[response.length - 1] ^= 1;
          sendFrame(response);
          current.incoming = new Cipher(handshake.rx);
          current.outgoing = new Cipher(handshake.tx);
        }
        continue;
      }
      const plaintext = current.incoming.decrypt(encrypted);
      const kind = plaintext[0];
      const id = plaintext.readUInt32BE(1);
      if (kind === packetTypes.auth) {
        if (attempt === 1 && startup === 'auth-eof') controller.close();
        else send(packetTypes.authenticated, 0, attempt === 1 && startup === 'revoked' ? 'revoked' : 'ok');
        continue;
      }
      if (kind !== packetTypes.request) continue;
      const { method, path } = JSON.parse(plaintext.subarray(5));
      requests.push({ attempt, method, path });
      if (path.endsWith('/events')) {
        send(packetTypes.response, id, JSON.stringify({ status: 200, headers: { 'content-type': 'text/event-stream' } }));
        send(packetTypes.responseData, id, 'event: snapshot\ndata: {"ok":true}\n\n');
      } else if (method === 'POST' && attempt === 1) {
        if (disconnect === 'eof') controller.close();
        else if (disconnect === 'network-reset') controller.error(networkReset());
        else if (disconnect === 'tampering') sendFrame(Buffer.alloc(21));
        else if (disconnect === 'invalid-response') send(255, id);
      } else {
        send(packetTypes.response, id, JSON.stringify({ status: 200, headers: { 'content-type': 'application/json' } }));
        send(packetTypes.responseData, id, '{"connected":true}');
        send(packetTypes.responseEnd, id);
      }
    }
    return new Response(null, { status: 204 });
  });

  return { connection, requests, attempts: () => attempts };
}

test('云端请求或握手读取中断后重新握手，不误报加密校验失败', async t => {
  for (const startup of ['fetch-rejection', 'html-response', 'ready-eof', 'handshake-read-rejection', 'handshake-eof', 'auth-eof']) {
    await t.test(startup, async t => {
      const { connection, requests, attempts } = relay(t, { startup });
      await assert.rejects(connection.request('/api/session'), transportError);
      assert.deepEqual(requests, []);
      assert.deepEqual(await (await connection.request('/api/session')).json(), { connected: true });
      assert.equal(attempts(), 2);
    });
  }
});

test('已收到聊天快照的连接遇到网络重置或正常 EOF，失败的写入仅提交一次', async t => {
  for (const disconnect of ['network-reset', 'eof']) {
    await t.test(disconnect, async t => {
      const { connection, requests, attempts } = relay(t, { disconnect });
      const events = readSse(await connection.request('/api/threads/fixture/events'));
      assert.deepEqual((await events.next()).value, { type: 'snapshot', data: '{"ok":true}' });
      await Promise.all([
        assert.rejects(events.next(), failure => {
          assert.ok(failure instanceof SseReadError);
          assert.ok(failure.cause instanceof RelayError);
          assert.match(failure.cause.message, transportError.message);
          return true;
        }),
        assert.rejects(connection.request('/api/threads/fixture/turns', { method: 'POST' }), transportError),
      ]);
      assert.deepEqual(await (await connection.request('/api/session')).json(), { connected: true });
      assert.equal(attempts(), 2);
      assert.deepEqual(requests.filter(request => request.method === 'POST'), [
        { attempt: 1, method: 'POST', path: '/api/threads/fixture/turns' },
      ]);
    });
  }
});

test('握手或下行密文和协议校验失败仍关闭旧会话', async t => {
  for (const mode of ['handshake-tampering', 'tampering', 'invalid-response']) {
    await t.test(mode, async t => {
      const { connection, attempts } = relay(t, { startup: mode, disconnect: mode });
      await assert.rejects(connection.request('/api/session', { method: 'POST' }), integrityError);
      assert.deepEqual(await (await connection.request('/api/session')).json(), { connected: true });
      assert.equal(attempts(), 2);
    });
  }
});

test('加密认证明确拒绝后仍报告撤销配对', async t => {
  const { connection, requests } = relay(t, { startup: 'revoked' });
  await assert.rejects(connection.request('/api/session'), failure =>
    failure instanceof RelayError && failure.status === 401 && /已撤销配对/.test(failure.message));
  assert.deepEqual(requests, []);
});
