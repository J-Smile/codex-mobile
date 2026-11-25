import Noise from 'noise-handshake';
import Cipher from 'noise-handshake/cipher.js';
import { readSse, SseReadError } from '../../sse.ts';
import { desktopApiPath, RelayError } from '../../transport.ts';
import { keyBytes, type Desktop, type PhoneIdentity } from './devices.ts';

type Bytes = Uint8Array<ArrayBuffer>;
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });
const prologue = encoder.encode('codex-mobile-relay/v1');
const chunkSize = 48 * 1024;
const batchRecords = 8;
const receiveWindow = 8;
const sessionHeader = 'x-codex-relay-session';
const maxBody = 20 * 1024 * 1024;
const maxCounter = 0x1_0000_0000;

const frame = {
  auth: 1, authenticated: 2, request: 3, requestData: 4, cancel: 5, response: 6,
  responseData: 7, responseEnd: 8, responseError: 9, responseNext: 10,
} as const;

const unavailable = () => new RelayError('无法通过云中转连接电脑，请确认桌面服务在线后重试。');
const rejected = () => new RelayError('这台电脑已撤销配对，请重新扫码。', 401);
const tunnelFailure = (failure: unknown) => failure instanceof RelayError ? failure
  : failure instanceof SseReadError ? unavailable() : new RelayError('加密连接校验失败，请重新连接。');

function encodeBatch(frames: Bytes[]) {
  const bytes = new Uint8Array(frames.reduce((length, frame) => length + 2 + frame.byteLength, 0));
  const view = new DataView(bytes.buffer);
  let offset = 0;
  for (const frame of frames) {
    view.setUint16(offset, frame.byteLength);
    bytes.set(frame, offset + 2);
    offset += 2 + frame.byteLength;
  }
  return bytes;
}

function* decodeBatch(encoded: string) {
  if (encoded.length > Math.ceil(batchRecords * 65_537 * 4 / 3)) throw new Error('SSE batch too large');
  const binary = atob(encoded.replaceAll('-', '+').replaceAll('_', '/'));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  const view = new DataView(bytes.buffer);
  for (let offset = 0; offset < bytes.length;) {
    const size = view.getUint16(offset);
    offset += 2;
    if (size === 0 || offset + size > bytes.length) throw new Error('invalid ciphertext length');
    yield bytes.subarray(offset, offset + size);
    offset += size;
  }
}

async function* relayFrames(events: ReturnType<typeof readSse>, onActivity: () => void) {
  for await (const { type, data } of events) {
    onActivity();
    if (type === 'heartbeat') continue;
    if (type !== 'frame') throw new Error('invalid relay event');
    yield* decodeBatch(data);
  }
}

class Uplink {
  private readonly queue: { data: Bytes; resolve: () => void; reject: (error: unknown) => void }[] = [];
  private sending = false;
  private readonly endpoint: URL;
  private readonly session: string;
  private readonly controller: AbortController;

  constructor(endpoint: URL, session: string, controller: AbortController) {
    this.endpoint = endpoint;
    this.session = session;
    this.controller = controller;
  }

  send(data: Bytes) {
    return new Promise<void>((resolve, reject) => {
      this.controller.signal.throwIfAborted();
      this.queue.push({ data, resolve, reject });
      if (this.sending) return;
      this.sending = true;
      queueMicrotask(() => { void this.flush(); });
    });
  }

  private async flush() {
    const signal = this.controller.signal;
    while (this.queue.length) {
      const batch = this.queue.splice(0, batchRecords);
      const timeout = setTimeout(() => this.controller.abort(unavailable()), 12_000);
      try {
        const response = await fetch(this.endpoint, {
          method: 'POST',
          headers: { 'content-type': 'application/octet-stream', [sessionHeader]: this.session },
          body: encodeBatch(batch.map(frame => frame.data)),
          signal,
          redirect: 'error',
        });
        if (!response.ok) throw unavailable();
        for (const frame of batch) frame.resolve();
      } catch {
        const error = signal.aborted ? signal.reason : unavailable();
        this.controller.abort(error);
        for (const frame of [...batch, ...this.queue.splice(0)]) frame.reject(error);
      } finally {
        clearTimeout(timeout);
      }
    }
    this.sending = false;
  }
}

function record(kind: number, id: number, payload = new Uint8Array(0)): Bytes {
  const bytes = new Uint8Array(5 + payload.byteLength);
  bytes[0] = kind;
  new DataView(bytes.buffer).setUint32(1, id);
  bytes.set(payload, 5);
  return bytes;
}

export function createPhoneIdentity(): PhoneIdentity {
  return new Noise('IK', true).s;
}

interface Pending {
  head: (status: number, headers: Record<string, string>) => void;
  chunk: (bytes: Bytes) => void;
  finish: () => void;
  fail: (error: unknown) => void;
}

type Sender = (kind: number, id: number, bytes?: Bytes) => Promise<void>;

class Channel {
  private readonly pending = new Map<number, Pending>();
  private readonly send: Sender;
  private readonly closeTransport: () => void;
  private next = 0;

  constructor(closeTransport: () => void, send: Sender) {
    this.closeTransport = closeTransport;
    this.send = send;
  }

  close() { this.closeTransport(); }

  fail(error: RelayError) {
    for (const pending of this.pending.values()) pending.fail(error);
    this.pending.clear();
  }

  receive(kind: number, id: number, bytes: Bytes) {
    const pending = this.pending.get(id);
    if (!pending) return;
    switch (kind) {
      case frame.response: {
        const { status, headers } = JSON.parse(decoder.decode(bytes)) as {
          status: number;
          headers: Record<string, string>;
        };
        pending.head(status, headers);
        break;
      }
      case frame.responseData:
        pending.chunk(bytes);
        break;
      case frame.responseEnd:
        pending.finish();
        break;
      case frame.responseError:
        pending.fail(new RelayError('电脑未能完成此请求，请重试。', 503));
        break;
      default:
        throw new Error('invalid response');
    }
  }

  private async upload(id: number, body: Blob | Bytes, length: number) {
    const batchSize = chunkSize * batchRecords;
    for (let offset = 0; offset < length && this.pending.has(id); offset += batchSize) {
      const batch = body instanceof Blob
        ? new Uint8Array(await body.slice(offset, offset + batchSize).arrayBuffer())
        : body.subarray(offset, offset + batchSize);
      if (!this.pending.has(id)) return;
      const sends: Promise<void>[] = [];
      for (let part = 0; part < batch.length; part += chunkSize) {
        sends.push(this.send(frame.requestData, id, batch.subarray(part, part + chunkSize)));
      }
      await Promise.all(sends);
    }
  }

  async request(path: string, options: RequestInit): Promise<Response> {
    const method = (options.method || 'GET').toUpperCase();
    const headers = new Headers(options.headers);
    const input = options.body;
    if (input && typeof input !== 'string' && !(input instanceof Blob)) {
      throw new RelayError('不支持此上传格式。', 400);
    }
    const body = typeof input === 'string' ? encoder.encode(input) : input || new Uint8Array(0);
    const length = body instanceof Blob ? body.size : body.byteLength;
    if (length > (path === '/api/images' ? maxBody : 1024 * 1024)) {
      throw new RelayError('文件或请求过大。', 413);
    }
    const { signal } = options;
    signal?.throwIfAborted();
    const id = ++this.next;
    if (id >= maxCounter) {
      this.close();
      throw unavailable();
    }
    return new Promise((resolve, reject) => {
      let stream: ReadableStreamDefaultController<Bytes> | undefined;
      let credit = receiveWindow;
      let finished = false;
      const cleanup = () => {
        finished = true;
        this.pending.delete(id);
        signal?.removeEventListener('abort', abort);
      };
      const cancelRemote = () => {
        void this.send(frame.cancel, id).catch(() => this.close());
      };
      const cancel = () => {
        cleanup();
        cancelRemote();
      };
      const abort = () => {
        pending.fail(signal?.reason);
        cancelRemote();
      };
      const pending: Pending = {
        head: (status, responseHeaders) => {
          const readable = method === 'HEAD' || [204, 205, 304].includes(status) ? null : new ReadableStream<Bytes>({
            start(controller) { stream = controller; },
            pull: controller => {
              const available = (controller.desiredSize ?? 0) - credit;
              if (finished || available <= 0 || (credit > 0 && available < receiveWindow / 2)) return;
              credit += available;
              return this.send(frame.responseNext, id, Uint8Array.of(available)).catch(pending.fail);
            },
            cancel,
          }, { highWaterMark: receiveWindow });
          resolve(new Response(readable, { status, headers: responseHeaders }));
        },
        chunk: bytes => {
          if (!stream || credit <= 0) throw new Error('response exceeded its credit');
          credit--;
          stream.enqueue(bytes);
        },
        finish: () => {
          if (finished) return;
          stream?.close();
          cleanup();
        },
        fail: error => {
          if (finished) return;
          stream?.error(error);
          cleanup();
          reject(error);
        },
      };
      this.pending.set(id, pending);
      signal?.addEventListener('abort', abort, { once: true });
      void this.send(frame.request, id, encoder.encode(JSON.stringify({
        method,
        path,
        length,
        contentType: headers.get('content-type'),
        contract: headers.get('x-codex-mobile-contract'),
        fileName: headers.get('x-file-name'),
        range: headers.get('range'),
        responseWindow: receiveWindow,
      }))).then(() => {
        if (length) return this.upload(id, body, length);
      }).catch(error => { if (!finished) { pending.fail(error); cancelRemote(); } });
    });
  }
}

export class RelayConnection {
  private readonly desktop: Desktop;
  private readonly origin: string;
  private readonly accountExpired?: () => void;
  private live?: { controller: AbortController; connecting: Promise<Channel> };

  constructor(desktop: Desktop, origin = location.origin, accountExpired?: () => void) {
    this.desktop = desktop;
    this.origin = origin;
    this.accountExpired = accountExpired;
  }

  private connect(ticket: string, name: string) {
    if (this.live) return this.live.connecting;
    const controller = new AbortController();
    controller.signal.addEventListener('abort', () => {
      if (this.live?.controller === controller) this.live = undefined;
    }, { once: true });
    const connecting = this.open(controller, ticket, name);
    this.live = { controller, connecting };
    return connecting;
  }

  private async open(controller: AbortController, ticket: string, name: string) {
    const signal = controller.signal;
    let timeout = setTimeout(() => controller.abort(unavailable()), 15_000);
    signal.addEventListener('abort', () => clearTimeout(timeout), { once: true });
    const watchHeartbeat = () => {
      clearTimeout(timeout);
      if (!signal.aborted) timeout = setTimeout(() => controller.abort(unavailable()), 45_000);
    };
    try {
      const endpoint = '/relay/mobile/' + this.desktop.id;
      const response = await fetch(new URL(endpoint + '/session', this.origin), {
        method: 'POST',
        headers: { accept: 'text/event-stream' },
        signal,
        redirect: 'error',
        cache: 'no-store',
      }).catch(() => { throw unavailable(); });
      if (response.status === 404) throw new RelayError('这台电脑不在当前账号中，或暂时离线。', 404);
      if (response.status === 401) {
        this.accountExpired?.();
        throw new RelayError('云端账号登录已过期，请重新登录。', 401);
      }
      if (!response.ok) throw unavailable();
      const events = readSse(response);
      const ready = await events.next();
      if (ready.done) throw unavailable();
      if (ready.value.type !== 'ready' || !/^[a-f0-9]{32}$/.test(ready.value.data)) throw new Error('invalid session');
      const uplink = new Uplink(new URL(endpoint + '/send', this.origin), ready.value.data, controller);
      let authenticated = false;
      const packets = relayFrames(events, () => { if (authenticated) watchHeartbeat(); });

      const handshake = new Noise('IK', true, this.desktop.phone);
      handshake.initialise(prologue, keyBytes(this.desktop.publicKey));
      await uplink.send(handshake.send());
      const serverHello = await packets.next();
      if (serverHello.done) throw unavailable();
      if (handshake.recv(serverHello.value).byteLength) throw new Error('invalid handshake');
      const outgoing = new Cipher(handshake.tx);
      const incoming = new Cipher(handshake.rx);
      const send: Sender = (kind, id, data) => {
        if (signal.aborted) return Promise.reject(signal.reason);
        if (outgoing.nonce >= maxCounter) {
          controller.abort(unavailable());
          return Promise.reject(signal.reason);
        }
        return uplink.send(outgoing.encrypt(record(kind, id, data)));
      };
      await send(frame.auth, 0, encoder.encode(JSON.stringify({ name, ticket })));
      const authentication = await packets.next();
      if (authentication.done) throw unavailable();
      const result = incoming.decrypt(authentication.value);
      if (result.length < 5 || result[0] !== frame.authenticated
        || new DataView(result.buffer, result.byteOffset).getUint32(1) !== 0) {
        throw new Error('invalid authentication');
      }
      if (decoder.decode(result.subarray(5)) !== 'ok') throw rejected();

      const channel = new Channel(() => controller.abort(unavailable()), send);
      const disconnected = () => channel.fail(signal.reason);
      signal.throwIfAborted();
      signal.addEventListener('abort', disconnected, { once: true });
      authenticated = true;
      watchHeartbeat();
      void (async () => {
        try {
          for await (const encrypted of packets) {
            if (incoming.nonce >= maxCounter) throw new Error('session limit');
            const message = incoming.decrypt(encrypted);
            if (message.length < 5) throw new Error('short frame');
            const id = new DataView(message.buffer, message.byteOffset).getUint32(1);
            channel.receive(message[0], id, message.subarray(5));
          }
          controller.abort(unavailable());
        } catch (failure) {
          controller.abort(tunnelFailure(failure));
        }
      })();
      return channel;
    } catch (failure) {
      const error = signal.aborted ? signal.reason : tunnelFailure(failure);
      controller.abort(error);
      throw error;
    }
  }

  async pair(ticket: string, name: string) { await this.connect(ticket, name); }

  get hasTunnel() { return this.live !== undefined; }

  close() { this.live?.controller.abort(unavailable()); }

  private channel(signal?: AbortSignal | null) {
    signal?.throwIfAborted();
    const connecting = this.connect('', '');
    let abort: (() => void) | undefined;
    const cancelled = signal && new Promise<never>((_, reject) => {
      abort = () => reject(signal.reason);
      signal.addEventListener('abort', abort, { once: true });
    });
    return (cancelled ? Promise.race([connecting, cancelled]) : connecting)
      .finally(() => { if (abort) signal?.removeEventListener('abort', abort); });
  }

  async measureLatency(signal: AbortSignal) {
    const channel = await this.channel(signal);
    const start = performance.now();
    const response = await channel.request('/api/session?ping=1', { signal });
    // 旧桌面仍返回会话 JSON，读完整个响应后再采样。
    await response.arrayBuffer();
    return response.ok ? Math.round(performance.now() - start) : null;
  }

  async request(url: string, options: RequestInit = {}) {
    const path = desktopApiPath(url, this.origin);
    if (!path) throw new RelayError('请求地址无效。', 400);
    const channel = await this.channel(options.signal);
    return channel.request(path, options);
  }
}
