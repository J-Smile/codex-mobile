import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { build } from 'vite';

test('手机入口预加载局域网与云页面各自的 CSS', { timeout: 30000 }, async () => {
  const bundle = await build({
    configFile: fileURLToPath(new URL('../../mobile/vite.config.ts', import.meta.url)),
    logLevel: 'silent',
    build: { write: false },
  });
  const entry = bundle.output.find(item => item.type === 'chunk' && item.isEntry);
  const ast = ts.createSourceFile(entry.fileName, entry.code, ts.ScriptTarget.Latest, true);
  assert.ok(bundle.output.every(item => !item.fileName.includes('PairingPage')));
  const styles = new Map(['App', 'RelayHome'].map(page => [page,
    bundle.output.find(item => item.type === 'asset' && item.fileName.includes('/' + page + '-') && item.fileName.endsWith('.css')).fileName,
  ]));
  let dependencies;
  const findDependencies = node => {
    if (ts.isArrayLiteralExpression(node) && node.elements.every(ts.isStringLiteral)) {
      const files = node.elements.map(item => item.text);
      if ([...styles.values()].every(file => files.includes(file))) dependencies = files;
    }
    ts.forEachChild(node, findDependencies);
  };
  findDependencies(ast);
  assert.ok(dependencies, '生产入口缺少页面资源映射');

  const found = new Set();
  const inspectPreload = node => {
    if (ts.isCallExpression(node) && node.arguments[0] && ts.isArrowFunction(node.arguments[0])) {
      const pages = [];
      const findImports = child => {
        if (ts.isCallExpression(child) && child.expression.kind === ts.SyntaxKind.ImportKeyword) {
          const page = child.arguments[0].text.match(/\/(App|RelayHome)-/)?.[1];
          if (page) pages.push(page);
        }
        ts.forEachChild(child, findImports);
      };
      findImports(node.arguments[0]);
      for (const page of pages) {
        const indices = node.arguments[1]?.arguments?.[0];
        assert.ok(indices && ts.isArrayLiteralExpression(indices), page + ' 缺少预加载依赖');
        const files = indices.elements.map(index => dependencies[Number(index.text)]);
        assert.ok(files.includes(styles.get(page)), page + ' 没有加载自己的 CSS');
        found.add(page);
      }
    }
    ts.forEachChild(node, inspectPreload);
  };
  inspectPreload(ast);
  assert.deepEqual(found, new Set(['App', 'RelayHome']));
});
