import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const mode = process.argv[2];
if (mode !== 'build' && mode !== 'test') throw new Error('请使用 npm run build 或 npm test。');
const temporary = mode === 'build' ? fs.mkdtempSync(path.join(os.tmpdir(), 'codex-mobile-build-')) : null;
try {
  const result = spawnSync('docker', [
    'buildx', 'build', '--file', 'deploy/relay/Dockerfile',
    ...(mode === 'build'
      ? ['--target', 'frontend-export', '--output', `type=local,dest=${temporary}`]
      : ['--target', 'check', '--build-arg', `CHECK_RUN=${randomUUID()}`, '--output', 'type=cacheonly']),
    '.',
  ], { cwd: root, stdio: 'inherit', windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.exitCode = result.status ?? 1;
  } else if (temporary) {
    // 导出成功后再替换，保留 Vite 清理旧哈希资源的行为。
    for (const name of ['dist', 'desktop-dist']) {
      const destination = path.join(root, name);
      fs.rmSync(destination, { recursive: true, force: true });
      fs.cpSync(path.join(temporary, name), destination, { recursive: true });
    }
  }
} finally {
  if (temporary) fs.rmSync(temporary, { recursive: true, force: true });
}
