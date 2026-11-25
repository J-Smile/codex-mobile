import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { ref } from 'vue';

const source = readFileSync(new URL('../../mobile/src/features/conversation/ItemView.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('ItemView.ts', source, ts.ScriptTarget.Latest, true);
const selected = ast.statements.filter(node => ts.isFunctionDeclaration(node) ? node.name?.text === 'copy'
  : ts.isVariableStatement(node) && node.declarationList.declarations.some(entry => ['copied', 'copyError'].includes(entry.name.getText(ast))))
  .map(node => node.getText(ast)).join('\n');
const compiled = ts.transpileModule(selected, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;

function fixture(copyText) {
  const timers = [];
  const app = new Function('ref', 'copyText', 'setTimeout', compiled + '\nreturn { copied, copyError, copy };')(ref, copyText, callback => timers.push(callback));
  return { ...app, timers };
}

test('复制失败明确提示长按复制，重试前清除旧错误并保留完整输出', async () => {
  let attempt = 0, finish;
  const values = [];
  const app = fixture(value => {
    values.push(value);
    if (++attempt === 1) return Promise.reject(new DOMException('Clipboard denied', 'NotAllowedError'));
    return new Promise(resolve => { finish = resolve; });
  });
  const output = '第一行\n  保留缩进与换行\n最后一行';
  await app.copy(output);
  assert.equal(app.copied.value, false);
  assert.equal(app.copyError.value, '复制失败，请长按输出文字复制。');
  const retry = app.copy(output);
  assert.equal(app.copyError.value, '');
  assert.equal(app.copied.value, false);
  finish();
  await retry;
  assert.equal(app.copied.value, true);
  assert.equal(app.copyError.value, '');
  assert.deepEqual(values, [output, output]);
});

test('上次复制成功后的失败不会保留成功提示，原成功计时器不清除失败提示', async () => {
  let fail = false;
  const app = fixture(async () => { if (fail) throw new Error('Clipboard denied'); });
  await app.copy('输出');
  assert.equal(app.copied.value, true);
  fail = true;
  await app.copy('新的输出');
  assert.equal(app.copied.value, false);
  assert.match(app.copyError.value, /复制失败/);
  app.timers[0]();
  assert.match(app.copyError.value, /复制失败/);
});
