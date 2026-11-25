import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { OutputBundle } from 'rollup';

const root = fileURLToPath(new URL('.', import.meta.url));
const publicDir = path.resolve(root, '../public');
const outDir = path.resolve(root, '../dist');
const version = new Date().toISOString();
const relayWeb = process.env.CODEX_MOBILE_RELAY_WEB === '1';

export function shellAssets(bundle: OutputBundle) {
  const assets = new Set<string>();
  function visit(file: string) {
    const chunk = bundle[file];
    if (chunk?.type !== 'chunk' || assets.has(file)) return;
    assets.add(file);
    for (const css of chunk.viteMetadata?.importedCss || []) assets.add(css);
    chunk.imports.forEach(visit);
    // main.ts 同时加载 App 和 Vue；App 里的聊天、扫码等异步入口按使用缓存。
    if (chunk.isEntry) chunk.dynamicImports.forEach(visit);
  }
  Object.values(bundle).forEach(chunk => { if (chunk.type === 'chunk' && chunk.isEntry) visit(chunk.fileName); });
  return Array.from(assets, file => '/' + file);
}

export default defineConfig({
  root,
  publicDir,
  plugins: [vue(), {
    name: 'codex-mobile-update',
    apply: 'build',
    transformIndexHtml: () => [
      { tag: 'meta', attrs: { name: 'app-version', content: version }, injectTo: 'head' },
      ...(relayWeb ? [{ tag: 'meta', attrs: { name: 'codex-mobile-relay' }, injectTo: 'head' as const }] : []),
    ],
    writeBundle(options, bundle) {
      const directory = options.dir || outDir;
      const assets = shellAssets(bundle);
      const worker = fs.readFileSync(path.join(publicDir, 'sw.js'), 'utf8')
        .replace('__BUILD_VERSION__', version).replace('__BUILD_ASSETS__', JSON.stringify(assets))
        .replace('__RELAY_WEB__', String(relayWeb));
      fs.writeFileSync(path.join(directory, 'version.json'), JSON.stringify({ version }));
      fs.writeFileSync(path.join(directory, 'sw.js'), worker);
    },
  }],
  server: { proxy: { '/api': 'http://127.0.0.1:4319' } },
  build: { outDir, emptyOutDir: true },
});
