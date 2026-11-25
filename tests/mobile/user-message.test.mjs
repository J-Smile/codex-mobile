import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { computed, ref } from 'vue';
import ts from 'typescript';

const source = readFileSync(new URL('../../apps/mobile/src/features/conversation/ItemView.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('ItemView.ts', source, ts.ScriptTarget.Latest, true);
const declaration = ast.statements.find(node => ts.isVariableStatement(node)
  && node.declarationList.declarations.some(entry => entry.name.getText(ast) === 'userText'));
const script = ts.transpileModule(declaration.getText(ast), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
const display = content => new Function('computed', 'item', script + '\nreturn userText.value;')(computed, ref({ content }));
const text = value => ({ type: 'text', text: value });
const reply = entries => '<send_user_message_question_reply>\n' + JSON.stringify(entries) + '\n</send_user_message_question_reply>\n';

test('单题回复只显示实际回答，保留换行、符号及原有正文', () => {
  const answer = '已经重跑。\n仍显示 "连接失败"，请检查 `<root>`。';
  assert.equal(display([text(reply([{ questionItemId: '["request_user_input_async","call_fixture",0]', question: '是否已重跑？', answer }]))]), answer);
  assert.equal(display([text('补充说明'), text(reply([{ answer }]))]), '补充说明\n' + answer);
});

test('多题回复保留题目与答案的对应关系', () => {
  assert.equal(display([text(reply([
    { question: '在哪台设备？', answer: '手机' },
    { question: '何时发生？', answer: '关闭面板后' },
  ]))]), '在哪台设备？\n手机\n\n何时发生？\n关闭面板后');
});

test('普通消息、代码示例与无法识别的回复保持原文', () => {
  const values = [
    '普通消息\n{"answer":"这仍是原始 JSON"}',
    '```xml\n' + reply([{ answer: '示例回答' }]) + '```',
    '<send_user_message_question_reply>\n不是 JSON\n</send_user_message_question_reply>',
    reply([]), reply([{ question: '缺少答案' }]), reply([{ answer: { value: '未知结构' } }]),
  ];
  for (const value of values) assert.equal(display([text(value)]), value);
});

test('图片附带的文件说明仍仅在含图片时移除', () => {
  const value = '# Files mentioned by the user:\n图片.png\n\n## My request:\n检查这张图片';
  assert.equal(display([text(value), { type: 'localImage', path: '/fixture/image.png' }]), '检查这张图片');
  assert.equal(display([text(value)]), value);
});
