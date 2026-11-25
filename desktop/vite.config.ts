import { defineConfig } from 'vite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = path.dirname(fileURLToPath(import.meta.url));
export default defineConfig({
  root,
  publicDir: path.resolve(root, '../public'),
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: '127.0.0.1' },
  build: { outDir: path.resolve(root, '../desktop-dist'), emptyOutDir: true, target: 'es2022' },
});
