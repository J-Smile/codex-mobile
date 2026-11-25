import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import { RelayConnection, createPhoneIdentity } from '../../apps/mobile/src/features/relay/connection.ts';
import { readSse } from '../../apps/mobile/src/sse.ts';
import { desktopId, parseInvitation } from '../../apps/mobile/src/features/relay/devices.ts';

const { origin, cookie, invitations, labels } = JSON.parse(readFileSync(0, 'utf8'));
const invites = invitations.map(link => parseInvitation(link, origin));
const fileName = '并发-附件.png';
const markers = ['/api/images', '/api/threads', '/api/projects', fileName, encodeURIComponent(fileName), ...labels, ...invites.map(invite => invite.ticket)]
  .map(value => Buffer.from(value));
const active = [];
let frames = 0;
let leaked = false;

const originalFetch = globalThis.fetch;
const recordings = [];
let posts = 0;
function observeBatch(batch) {
  for (let offset = 0; offset < batch.length;) {
    const length = batch.readUInt16BE(offset);
    offset += 2;
    assert.ok(length && offset + length <= batch.length);
    const frame = batch.subarray(offset, offset + length);
    frames++;
    leaked ||= markers.some(marker => frame.includes(marker));
    offset += length;
  }
}

globalThis.fetch = async (url, options = {}) => {
  const headers = new Headers(options.headers);
  headers.set('origin', origin);
  headers.set('cookie', cookie);
  const pathname = new URL(url).pathname;
  if (pathname.endsWith('/send')) {
    assert.equal(options.method, 'POST');
    assert.equal(headers.get('content-type'), 'application/octet-stream');
    assert.ok(options.body instanceof Uint8Array);
    posts++;
    observeBatch(Buffer.from(options.body));
  }
  const response = await originalFetch(url, { ...options, headers });
  if (pathname.endsWith('/session') && response.ok) {
    recordings.push((async () => {
      try {
        for await (const { type, data } of readSse(response.clone())) {
          if (type === 'frame') observeBatch(Buffer.from(data, 'base64url'));
        }
      } catch (error) {
        if (!options.signal?.aborted) throw error;
      }
    })());
  }
  return response;
};

function connect(desktop) {
  const connection = new RelayConnection(desktop, origin);
  active.push(connection);
  return connection;
}

async function readSnapshot(response, label) {
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-type'), /text\/event-stream/);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let stream = '';
  while (!stream.includes(label)) {
    const { value, done } = await reader.read();
    assert.equal(done, false, 'SSE 在收到当前桌面快照前已结束');
    stream += decoder.decode(value, { stream: true });
  }
  assert.match(stream, /event: snapshot/);
  return reader;
}

async function readMetadata(connection, label, timings) {
  await Promise.all(Array.from({ length: 8 }, async (_, request) => {
    const started = performance.now();
    const response = await connection.request(request % 2 ? '/api/session' : '/api/threads');
    assert.equal(response.status, 200);
    const data = await response.json();
    if (request % 2 === 0) assert.equal(data.threads[0].title, label);
    timings.push(performance.now() - started);
  }));
}

function p95(timings) {
  timings.sort((a, b) => a - b);
  return timings[Math.ceil(timings.length * 0.95) - 1].toFixed(0);
}

const metadataTimes = [];
const fixture = readFileSync(new URL('../../shared/public/icon-192.png', import.meta.url));
const desktops = await Promise.all(invites.map(async invitation => {
  return { id: await desktopId(invitation.publicKey), publicKey: invitation.publicKey, name: invitation.name, phone: createPhoneIdentity() };
}));
try {
  await Promise.all(desktops.map((desktop, index) => connect(desktop).pair(invites[index].ticket, '测试手机')));
  const mismatched = connect({ ...desktops[0], publicKey: desktops[1].publicKey });
  await assert.rejects(mismatched.request('/api/session'), { name: 'Error', status: 0 });

  const started = performance.now();
  const socketsPerDesktop = 8;
  const clients = desktops.flatMap((desktop, index) => Array.from({ length: socketsPerDesktop }, (_, slot) => {
    return { connection: connect(desktop), index, slot };
  }));
  const uploads = await Promise.all(clients.map(async ({ connection, index, slot }) => {
    const label = labels[index];
    const bytes = Buffer.concat([fixture, Buffer.alloc(256 * 1024, index * socketsPerDesktop + slot)]);
    const events = connection.request('/api/threads/chat/events?textAppends=1&compactTextAppends=1')
      .then(response => readSnapshot(response, label));
    const upload = connection.request('/api/images', {
      method: 'POST',
      headers: { 'content-type': 'image/png', 'x-file-name': encodeURIComponent(fileName) },
      body: new Blob([bytes]),
    });
    const reads = readMetadata(connection, label, metadataTimes);
    const [snapshot, uploaded] = await Promise.all([events, upload, reads]);
    assert.equal(uploaded.status, 201);
    const result = await uploaded.json();
    assert.equal(result.name, fileName);
    assert.equal(result.size, bytes.length);
    // 保持附件正文和实时流暂停消费，确认同一条连接还能完成小请求。
    const download = await connection.request(result.url);
    const status = await connection.request('/api/session');
    assert.equal(status.status, 200);
    await status.json();
    assert.deepEqual(Buffer.from(await download.arrayBuffer()), bytes);
    await snapshot.cancel();
    return { connection, url: result.url, bytes };
  }));
  const mixedDuration = performance.now() - started;
  const sessionCount = recordings.length;
  const warmTimes = [];
  await Promise.all(clients.map(({ connection, index }) => readMetadata(connection, labels[index], warmTimes)));
  assert.equal(recordings.length, sessionCount, '已有连接的并发小请求应复用同一条 SSE 流');

  const { connection, url, bytes } = uploads[0];
  const otherDesktop = await uploads[socketsPerDesktop].connection.request(url);
  assert.equal(otherDesktop.status, 404);
  await otherDesktop.text();
  const stalled = await Promise.all(Array.from({ length: 48 }, () => connection.request(url)));
  assert.ok(stalled.every(response => response.status === 200 && response.body));
  await Promise.all(Array.from({ length: 32 }, () => assert.rejects(connection.request('/api/session'), { status: 503 })));
  assert.deepEqual(Buffer.from(await stalled[0].arrayBuffer()), bytes);
  await Promise.all(stalled.slice(1).map(response => response.body.cancel()));
  const recovered = await connection.request('/api/session');
  assert.equal(recovered.status, 200);
  await recovered.json();

  for (const connection of active) connection.close();
  await Promise.all(recordings);
  assert.ok(posts < frames, '多条加密帧应合并到同一个 POST');
  assert.equal(leaked, false, '转发载荷中出现明文标记');
  console.log(`端到端并发: ${desktops.length} 台桌面、${clients.length} 条手机客户端连接、${metadataTimes.length} 个并发元数据请求、${uploads.length} 组 SSE 与附件上传/下载。混合用例 ${mixedDuration.toFixed(0)} ms，元数据 P95 ${p95(metadataTimes)} ms。`);
  console.log(`复用 ${clients.length} 条已握手的连接再发起 ${warmTimes.length} 个并发元数据请求，P95 ${p95(warmTimes)} ms。`);
  console.log(`单连接 48 个挂起下载和 32 个超额请求通过，取消后正常恢复。检查 ${frames} 个二进制帧，无明文标记。`);
} finally {
  for (const connection of active) connection.close();
  await delay(0);
}
