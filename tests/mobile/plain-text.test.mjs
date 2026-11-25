import test from 'node:test';
import assert from 'node:assert/strict';
import { editorText } from '../../apps/mobile/src/features/composer/plainText.ts';

const text = value => ({ nodeType: 3, nodeName: '#text', textContent: value });
const element = (nodeName, ...children) => ({ nodeType: 1, nodeName, childNodes: children.map(child => typeof child === 'string' ? text(child) : child) });
const br = () => element('BR');

test('纯文本编辑保留中文、空格、换行与原始符号', () => {
  assert.equal(editorText(element('DIV', '第一行  \n第二行\n<原始文字>')), '第一行  \n第二行\n<原始文字>');
});

test('浏览器生成的空行占位不多出换行，连续空行仍保留', () => {
  assert.equal(editorText(element('DIV', br())), '');
  assert.equal(editorText(element('DIV', '第一行', element('DIV', br()))), '第一行\n');
  assert.equal(editorText(element('DIV', '第一行', element('DIV', br()), element('DIV', br()))), '第一行\n\n');
});

test('原生块与软换行得到相同正文，光标前片段保留行分隔', () => {
  assert.equal(editorText(element('DIV', element('DIV', '第一行'), element('DIV', '第二行'))), '第一行\n第二行');
  assert.equal(editorText(element('DIV', '第一行', br(), '第二行', br())), '第一行\n第二行');
  const prefix = editorText(element('#document-fragment', '第一行', element('DIV', '@')));
  assert.equal(prefix, '第一行\n@');
  assert.equal(prefix.length, '第一行\n@'.length);
});
