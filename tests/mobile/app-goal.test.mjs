import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { ref } from 'vue';
import { submissionFixture } from '../fixtures/submission.mjs';

const source = readFileSync(new URL('../../apps/mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const methods = new Set(['createGoal', 'changeMode', 'stopTurn', 'delegate', 'openDelegation', 'openGoal', 'openContext', 'openDesktop', 'onComposerInput', 'submissionReason']);
const compiled = ts.transpileModule(ast.statements.filter(node => ts.isFunctionDeclaration(node) && methods.has(node.name?.text))
  .map(node => node.getText(ast)).join('\n'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(t) {
  const state = {
    capabilities: ref({}), operationsReady: ref(true), error: ref(''), notice: ref(''),
    draft: ref('保留目标和草稿'), selectedId: ref('chat'), streamStatus: ref('connected'),
    selectedThread: ref({ id: 'chat', goal: { objective: '进行中的目标', status: 'active' }, model: 'fixture-model', mode: 'plan' }),
    turns: ref([{ id: 'turn', status: 'inProgress' }]), stopping: ref(false),
    goalOpen: ref(false), contextOpen: ref(false), delegationOpen: ref(false),
    sending: ref(false), resuming: ref(false), imagesUploading: ref(false), imageErrors: ref(false), composerMenu: ref(false),
    draftText: ref('@'), composer: ref(null), voice: { clear() {}, busy: ref(false) },
    newChat: ref(false), draftKey: ref('chat'), hasMessage: ref(true), resumableTurn: ref(),
    draftImages: ref([]), firstTurnPending: ref(false), delegationError: ref(''),
  };
  const submissions = submissionFixture(t, () => assert.fail('关闭的控制入口不能发出请求'));
  submissions.operationsReady.value = true;
  const bindings = { ...state, submissions, request: () => assert.fail('关闭的控制入口不能发出请求'),
    sendMessage: () => assert.fail('关闭的委派入口不能发送消息'),
    attachments: { clear: () => assert.fail('不能清空附件') } };
  return { ...state, ...new Function(...Object.keys(bindings), compiled + '\nreturn { ' + [...methods].join(', ') + ' };')(...Object.values(bindings)) };
}

test('新目标直接拒绝，既不创建聊天也不绑定目标，保留原草稿', async t => {
  const f = fixture(t);
  await assert.rejects(f.createGoal({ objective: '新的目标', tokenBudget: 6400 }), /请在官方 Codex/);
  assert.equal(f.draft.value, '保留目标和草稿');
  assert.equal(f.selectedId.value, 'chat');
});

test('执行和规划切换不再清除已有目标或修改设置', async t => {
  for (const mode of ['default', 'plan']) {
    const f = fixture(t);
    const before = JSON.parse(JSON.stringify(f.selectedThread.value));
    await f.changeMode(mode);
    assert.deepEqual(f.selectedThread.value, before);
    assert.match(f.error.value, /官方 Codex/);
  }
});

test('即使残留实时连接状态，停止、委派、目标和工具入口也无副作用', async t => {
  const f = fixture(t);
  await f.stopTurn();
  await f.delegate({ prompt: '不要发送', mode: 'now' });
  f.openDelegation(); f.openGoal(); f.openContext();
  await f.openDesktop();
  f.onComposerInput({ data: '@', isComposing: false });
  assert.equal(f.goalOpen.value, false);
  assert.equal(f.contextOpen.value, false);
  assert.equal(f.delegationOpen.value, false);
  assert.equal(f.draft.value, '保留目标和草稿');
  assert.equal(f.draftText.value, '@');
  assert.equal(f.turns.value[0].status, 'inProgress');
});
