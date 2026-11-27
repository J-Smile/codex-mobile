import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { ref } from 'vue';
import { removeAccountDesktop } from '../../apps/mobile/src/features/relay/devices.ts';

const source = fs.readFileSync(new URL('../../apps/mobile/src/features/relay/RelayHome.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('RelayHome.ts', source, ts.ScriptTarget.Latest, true);
const method = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'removeFromAccount');
const compiled = ts.transpileModule(method.getText(ast), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
function fixture(removeAccountDesktop, forgetDesktop = async () => {}) {
  const state = { account: ref({id:'account',desktops:[{id:'pending'},{id:'active'}]}), desktops:ref([{id:'active'}]), active:ref({id:'active'}),
    removing:ref(''), remove:ref('pending'), pairing:ref(false), changingAccount:ref(false), error:ref('') };
  let released = 0, navigated = 0;
  const bindings = {...state, removeAccountDesktop, forgetDesktop, releaseConnection:()=>released++, choose:()=>navigated++};
  const run = new Function(...Object.keys(bindings),'let removalTrigger;'+compiled+';return removeFromAccount;')(...Object.values(bindings));
  return {...state,run,get released(){return released;},get navigated(){return navigated;}};
}
test('云端移除等待期间不清目录且重复点击不重发，失败后可重试', async () => {
  let resolve, reject, calls = 0;
  const f=fixture(()=>{calls++;return new Promise((yes,no)=>{resolve=yes;reject=no;});});
  const pending=f.run('pending'); await f.run('pending');
  assert.equal(calls,1);assert.equal(f.account.value.desktops.length,2);
  reject(Error('服务器暂不可用'));await pending;
  assert.equal(f.error.value,'服务器暂不可用');assert.equal(f.remove.value,'pending');
  const retry=f.run('pending');resolve();await retry;
  assert.deepEqual(f.account.value.desktops,[{id:'active'}]);assert.equal(f.released,0);
});
test('云端移除当前电脑后，即使本地缓存清理失败也断开并返回目录', async () => {
  const f=fixture(async()=>{},async()=>{throw Error('存储失败');});await f.run('active');
  assert.equal(f.released,1);assert.equal(f.navigated,1);assert.deepEqual(f.desktops.value,[]);
  assert.match(f.error.value,/云账号移除/);assert.equal(f.removing.value,'');
});
test('移除接口认证失败不伪报成功，404可视为已移除', async t => {
  let status=401;const calls=[];
  t.mock.method(globalThis,'fetch',async(url,init)=>{calls.push({url,init});return new Response(null,{status});});
  await assert.rejects(removeAccountDesktop('pending'),/登录已过期/);
  status=503;await assert.rejects(removeAccountDesktop('pending'),/无法从云账号移除/);
  status=404;await removeAccountDesktop('pending');
  assert.equal(calls.length,3);assert.equal(calls[0].init.method,'DELETE');
});
