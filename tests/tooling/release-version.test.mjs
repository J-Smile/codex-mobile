import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { parse } from 'smol-toml';

const script = fileURLToPath(new URL('../../scripts/release-version.mjs', import.meta.url));
function fixture(t) {
  const temporaryRoot = path.resolve(os.tmpdir());
  const root = fs.mkdtempSync(path.join(temporaryRoot, 'cm-release-version-'));
  t.after(() => {
    assert.equal(path.dirname(path.resolve(root)), temporaryRoot);
    fs.rmSync(root, { recursive: true, force: true });
  });
  fs.mkdirSync(path.join(root, 'apps/desktop/src-tauri'), { recursive: true });
  const files = {
    'package.json': JSON.stringify({ name: 'codex-mobile', version: '0.3.0', scripts: { test: 'unchanged' } }),
    'package-lock.json': JSON.stringify({ version: '0.3.0', lockfileVersion: 3, packages: { '': { version: '0.3.0' }, 'node_modules/example': { version: '1.2.3' } } }),
    'Cargo.toml': '[workspace.package]\nversion = "0.3.0"\nedition = "2021"\n',
    'Cargo.lock': 'version = 4\n\n[[package]]\nname = "codex-mobile-bridge"\nversion = "0.3.0"\n\n[[package]]\nname = "codex-mobile-desktop"\nversion = "0.3.0"\n\n[[package]]\nname = "codex-mobile-relay"\nversion = "0.3.0"\n\n[[package]]\nname = "serde"\nversion = "1.0.229"\n',
    'apps/desktop/src-tauri/tauri.conf.json': JSON.stringify({ version: '0.3.0', productName: 'Codex Mobile' }),
  };
  for (const [file, text] of Object.entries(files)) fs.writeFileSync(path.join(root, file), text);
  const run = (args = [], env = {}) => spawnSync(process.execPath, [script, ...args], {
    cwd: root, encoding: 'utf8', windowsHide: true,
    env: { ...process.env, GITHUB_REF_TYPE: '', GITHUB_REF_NAME: '', GITHUB_OUTPUT: '', ...env },
  });
  const read = file => fs.readFileSync(path.join(root, file), 'utf8');
  return { root, files, run, read };
}

test('发布版本同步 npm、Cargo 和 Tauri 元数据，不修改依赖版本', t => {
  const f = fixture(t);
  assert.equal(f.run(['0.3.1']).status, 0);
  assert.equal(JSON.parse(f.read('package.json')).scripts.test, 'unchanged');
  assert.equal(JSON.parse(f.read('package-lock.json')).packages['node_modules/example'].version, '1.2.3');
  assert.equal(parse(f.read('Cargo.lock')).package.find(item => item.name === 'serde').version, '1.0.229');
  assert.equal(parse(f.read('Cargo.lock')).package.find(item => item.name === 'codex-mobile-relay').version, '0.3.1');
  const output = path.join(f.root, 'github-output');
  const checked = f.run([], { GITHUB_REF_TYPE: 'tag', GITHUB_REF_NAME: 'v0.3.1', GITHUB_OUTPUT: output });
  assert.equal(checked.status, 0, checked.stderr);
  assert.equal(fs.readFileSync(output, 'utf8'), 'version=0.3.1\ntag=v0.3.1\n');
});

test('CI 拒绝不一致版本及错误发布标签，检查不改写文件', t => {
  const f = fixture(t);
  fs.writeFileSync(path.join(f.root, 'Cargo.toml'), '[workspace.package]\nversion = "0.2.0"\n');
  const mismatch = f.run();
  assert.notEqual(mismatch.status, 0);
  assert.match(mismatch.stderr, /Cargo.toml.*0.2.0.*0.3.0/);
  assert.equal(f.read('package.json'), f.files['package.json']);
  fs.writeFileSync(path.join(f.root, 'Cargo.toml'), f.files['Cargo.toml']);
  const wrongTag = f.run([], { GITHUB_REF_TYPE: 'tag', GITHUB_REF_NAME: 'v0.4.0' });
  assert.notEqual(wrongTag.status, 0);
  assert.match(wrongTag.stderr, /发布标签必须为 v0.3.0/);
});

test('不接受预发布或无效版本，失败时保留原元数据', t => {
  const f = fixture(t);
  for (const version of ['v0.3.1', '0.3.1-beta', '01.2.3', '0.3', '']) {
    assert.notEqual(f.run([version]).status, 0);
  }
  for (const [file, source] of Object.entries(f.files)) assert.equal(f.read(file), source);
});
