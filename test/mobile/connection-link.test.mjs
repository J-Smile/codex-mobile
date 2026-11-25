import test from 'node:test';
import assert from 'node:assert/strict';
import { parseConnectionKey } from '../../mobile/src/features/pairing/connectionLink.ts';
import { desktopId, readPairingInvitation } from '../../mobile/src/features/relay/devices.ts';
import fs from 'node:fs';
import ts from 'typescript';
import { ref } from 'vue';

const script = fs.readFileSync(new URL('../../mobile/src/App.vue', import.meta.url), 'utf8').match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1];
const ast = ts.createSourceFile('App.ts', script, ts.ScriptTarget.Latest, true);
const methods = new Set(['connectScanned', 'takeConnectionKey', 'onConnectionLink']);
const compiled = ts.transpileModule(ast.statements.filter(node => ts.isFunctionDeclaration(node) && methods.has(node.name?.text)).map(node => node.getText(ast)).join('\n'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;

function fixture() {
  const loginKey = ref(''), scanOpen = ref(true), assignments = [], logins = [];
  const location = { origin: 'https://mobile.example', pathname: '/', search: '', hash: '', assign: url => assignments.push(url) };
  const history = { state: {}, replaceState(_, __, address) { const next = new URL(address, location.origin); Object.assign(location, { pathname: next.pathname, search: next.search, hash: next.hash }); } };
  const bindings = { loginKey, scanOpen, location, history, props: {}, initializeConnection: async () => { logins.push(loginKey.value); } };
  return { ...bindings, assignments, logins, ...new Function(...Object.keys(bindings), compiled + '\nreturn { connectScanned, takeConnectionKey, onConnectionLink };')(...Object.values(bindings)) };
}

test('扫码只将密钥填入当前网页并登录，不跳转或覆盖当前对话地址', async () => {
  const f = fixture(), key = 'k'.repeat(43);
  f.location.search = '?thread=current-chat';
  await f.connectScanned(key);
  assert.deepEqual(f.logins, [key]);
  assert.deepEqual(f.assignments, []);
  assert.equal(f.location.search, '?thread=current-chat');
  assert.equal(f.location.hash, '');
  assert.equal(f.scanOpen.value, false);
});

test('旧的跨地址二维码只读取密钥，主动打开的带密钥链接仍能登录', async () => {
  const f = fixture(), key = 'k'.repeat(43);
  await f.connectScanned(parseConnectionKey('https://retired.example/?thread=old-chat#key=' + key));
  assert.deepEqual(f.assignments, []);
  assert.deepEqual(f.logins, [key]);
  assert.equal(f.location.origin, 'https://mobile.example');
  assert.equal(f.location.search, '');
  f.location.hash = '#key=' + key;
  f.onConnectionLink();
  assert.deepEqual(f.logins, [key, key]);
  assert.equal(f.location.hash, '');
  f.onConnectionLink();
  assert.equal(f.logins.length, 2);
});

test('连接二维码支持纯密钥和旧链接，拒绝缺失或无效的密钥', () => {
  const key = 'a'.repeat(43);
  assert.equal(parseConnectionKey(key), key);
  assert.equal(parseConnectionKey('  ' + key + '\n'), key);
  assert.equal(parseConnectionKey('https://mobile.example/?thread=chat-1#key=' + key), key);
  for (const value of ['hello', 'javascript:alert(1)', 'https://mobile.example/', 'https://user:pass@mobile.example/#key=' + key]) {
    assert.throws(() => parseConnectionKey(value), /连接二维码/);
  }
});

test('云配对码匹配所选电脑公钥，拒绝别的电脑、域名和已过期的配对码', async () => {
  const origin = 'https://mobile.example';
  const publicKey = 'ab'.repeat(32);
  const id = await desktopId(publicKey);
  const target = { id, name: '工作电脑' };
  const payload = { version: 1, publicKey, ticket: 'a'.repeat(43), name: '云配对名称', expiresAt: Date.now() + 300_000 };
  const url = (options = {}, base = origin) => base + '/#pair=' + Buffer.from(JSON.stringify({ ...payload, ...options })).toString('base64url');
  assert.equal((await readPairingInvitation(url(), target, origin)).id, id);
  await assert.rejects(readPairingInvitation(url({ publicKey: 'cd'.repeat(32), name: target.name }), target, origin), /请扫描「工作电脑」/);
  await assert.rejects(readPairingInvitation(url({}, 'https://retired.example'), target, origin), /当前|此云端地址/);
  await assert.rejects(readPairingInvitation(url({ expiresAt: 0 }), target, origin), /二维码已过期/);
});
