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
    for (const name of ['mobile', 'desktop']) {
      const destination = path.join(root, 'dist', name);
      fs.rmSync(destination, { recursive: true, force: true });
      fs.cpSync(path.join(temporary, 'dist', name), destination, { recursive: true });
    }
  } else {
    const output = path.join(os.homedir(), '.codex', 'artifacts', 'codex-mobile', 'docker-regression', new Date().toISOString().replaceAll(':', '-'));
    fs.mkdirSync(output, { recursive: true });
    const image = 'codex-mobile-browser-test:' + randomUUID();
    const built = spawnSync('docker', ['buildx', 'build', '--file', 'deploy/relay/Dockerfile',
      '--target', 'browser-check', '--tag', image, '--load', '.'], { cwd: root, stdio: 'inherit', windowsHide: true });
    if (built.error) throw built.error;
    if (built.status !== 0) process.exitCode = built.status ?? 1;
    else {
      try {
        const tested = spawnSync('docker', ['run', '--rm', '--init', '--network', 'none',
          '--mount', `type=bind,source=${output},target=/results`, image], { cwd: root, stdio: 'inherit', windowsHide: true });
        if (tested.error) throw tested.error;
        if (tested.status !== 0) process.exitCode = tested.status ?? 1;
      } finally {
        // 仅删除本次生成的临时测试镜像标签；截图和报告始终保留。
        const removed = spawnSync('docker', ['image', 'rm', image], { cwd: root, stdio: 'pipe', windowsHide: true });
        if (removed.status !== 0) console.warn(`临时测试镜像未清理：${image}`);
        console.log(`浏览器回归报告：${output}`);
      }
    }
  }
} finally {
  if (temporary) fs.rmSync(temporary, { recursive: true, force: true });
}
