import fs from 'node:fs';
import path from 'node:path';
import { parse } from 'smol-toml';

// This command only edits release metadata. Compilation runs in GitHub Actions.
const root = process.cwd();
const requested = process.argv[2];
const stableVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
if (process.argv.length > 3 || (requested !== undefined && !stableVersion.test(requested))) {
  throw new Error('用法：npm run release:version -- 0.3.1；仅接受正式版本号，不带 v。');
}
const sources = Object.fromEntries([
  'package.json', 'package-lock.json', 'Cargo.toml', 'Cargo.lock', 'src-tauri/tauri.conf.json',
].map(file => [file, fs.readFileSync(path.join(root, file), 'utf8')]));
const manifest = JSON.parse(sources['package.json']);
const lock = JSON.parse(sources['package-lock.json']);
const tauri = JSON.parse(sources['src-tauri/tauri.conf.json']);
const workspace = parse(sources['Cargo.toml']);
const cargoLock = parse(sources['Cargo.lock']);
const version = requested ?? manifest.version;
if (!stableVersion.test(version)) throw new Error('package.json 必须使用正式版本号。');
const versions = {
  'package-lock.json': lock.version,
  'package-lock.json packages[""]': lock.packages[''].version,
  'Cargo.toml': workspace.workspace.package.version,
  'src-tauri/tauri.conf.json': tauri.version,
};
for (const name of ['codex-mobile-bridge', 'codex-mobile-desktop', 'codex-mobile-relay']) {
  const entry = cargoLock.package.find(item => item.name === name);
  if (!entry) throw new Error(`Cargo.lock 缺少 ${name}。`);
  versions[`Cargo.lock ${name}`] = entry.version;
}
if (requested !== undefined) {
  manifest.version = lock.version = lock.packages[''].version = tauri.version = version;
  sources['package.json'] = JSON.stringify(manifest, null, 2) + '\n';
  sources['package-lock.json'] = JSON.stringify(lock, null, 2) + '\n';
  sources['src-tauri/tauri.conf.json'] = JSON.stringify(tauri, null, 2) + '\n';
  sources['Cargo.toml'] = sources['Cargo.toml'].replace(
    /(\[workspace\.package\][\s\S]*?\nversion\s*=\s*)"[^"]+"/,
    `$1"${version}"`,
  );
  sources['Cargo.lock'] = sources['Cargo.lock'].replace(
    /(\[\[package\]\]\s*\nname = "codex-mobile-(?:bridge|desktop|relay)"\s*\nversion = )"[^"]+"/g,
    `$1"${version}"`,
  );
  for (const [file, source] of Object.entries(sources)) fs.writeFileSync(path.join(root, file), source);
  console.log(`发布版本已同步为 ${version}，未执行本地编译。提交并推送后，在 GitHub Actions 发布。`);
} else {
  for (const [file, actual] of Object.entries(versions)) {
    if (actual !== version) throw new Error(`${file} 的版本 ${actual} 与 package.json 的 ${version} 不一致。运行 npm run release:version -- ${version} 同步。`);
  }
  const tag = `v${version}`;
  if (process.env.GITHUB_REF_TYPE === 'tag' && process.env.GITHUB_REF_NAME !== tag) {
    throw new Error(`发布标签必须为 ${tag}，当前为 ${process.env.GITHUB_REF_NAME}。`);
  }
  if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\ntag=${tag}\n`);
  console.log(`发布版本检查通过：${tag}`);
}
