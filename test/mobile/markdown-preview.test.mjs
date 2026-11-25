import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import * as vue from 'vue';
import { previewFile } from '../../mobile/src/features/conversation/preview/filePreview.ts';

const source = readFileSync(new URL('../../mobile/src/features/conversation/preview/markdownPreview.ts', import.meta.url), 'utf8');

test('PDF 和 Office 的本机或网络链接不可操作，其他本机文件保留预览', () => {
  for (const path of ['/D:/项目/说明.PDF', 'relative%20note.%64ocx', 'https://example.test/表.XLSX?v=1', 'slides.pptx#page=1', 'sandbox:/mnt/data/report.pdf']) {
    assert.equal(previewFile(path)?.kind, 'disabled', path);
  }
  assert.deepEqual(previewFile('/D:/项目/页面%20A.html'), { name: '页面 A.html', kind: 'html' });
  assert.equal(previewFile('./app.ts#L2')?.kind, 'text');
  assert.equal(previewFile('https://example.test/page.html'), undefined);
});
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
function fixture(fail = false) {
  const calls = [];
  const renderer = { name: 'MdPreview' };
  const module = { exports: {} };
  new Function('require', 'module', 'exports', compiled)(name => {
    if (name === 'vue') return vue;
    calls.push(name);
    if (name === 'md-editor-v3') {
      if (fail) throw new Error('模块请求失败');
      return { MdPreview: renderer };
    }
    return {};
  }, module, module.exports);
  return { ...module.exports, calls, renderer, recover: () => { fail = false; } };
}

test('预加载与消息渲染共用一次 Markdown 和样式加载', async () => {
  const f = fixture();
  assert.deepEqual(f.calls, []);
  const first = f.preloadMarkdownPreview();
  assert.equal(f.preloadMarkdownPreview(), first);
  assert.equal(await first, f.renderer);
  assert.equal(f.markdownPreview.value, f.renderer);
  assert.deepEqual(f.calls.sort(), ['md-editor-v3', 'md-editor-v3/lib/preview.css']);
  await f.preloadMarkdownPreview();
  assert.equal(f.calls.length, 2);
});

test('Markdown 加载失败保留共享错误，不在同一页面重复导入失败模块', async () => {
  const f = fixture(true);
  const loading = f.preloadMarkdownPreview();
  await assert.rejects(loading, /模块请求失败/);
  assert.equal(f.markdownPreviewFailed.value, true);
  assert.equal(f.markdownPreview.value, undefined);
  assert.equal(f.calls.length, 2);
  f.recover();
  assert.equal(f.preloadMarkdownPreview(), loading);
  assert.equal(f.markdownPreviewFailed.value, true);
  assert.equal(f.calls.length, 2);
});
