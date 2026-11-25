import fs from 'node:fs';
import path from 'node:path';
import net from 'node:net';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const suffix = process.platform === 'win32' ? '.exe' : '';
const env = { ...process.env };
const pathKey = Object.keys(env).find(key => key.toUpperCase() === 'PATH') || 'PATH';
const searchPath = env[pathKey] || '';
const localCargo = path.join(root, '.local', 'cargo');
const cargoOnPath = searchPath.split(path.delimiter).some(directory => fs.existsSync(path.join(directory, `cargo${suffix}`)));
if (!env.CARGO_HOME && !cargoOnPath && fs.existsSync(path.join(localCargo, 'bin', `cargo${suffix}`))) {
  env.CARGO_HOME = localCargo;
  env.RUSTUP_HOME ||= path.join(root, '.local', 'rustup');
}
if (env.CARGO_HOME) env[pathKey] = path.join(env.CARGO_HOME, 'bin') + path.delimiter + searchPath;
const cargo = env.CARGO_HOME ? path.join(env.CARGO_HOME, 'bin', `cargo${suffix}`) : 'cargo';
const helpers = ['codex-mobile-bridge', 'codex-mobile-proxy', 'codex-mobile-updater'];

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: root, env, stdio: 'inherit', windowsHide: true, ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${path.basename(command)} 执行失败（${result.status ?? result.signal}）。`);
  return result;
}
function npm(script) {
  if (!process.env.npm_execpath) throw new Error('请通过 npm run 执行此构建命令。');
  run(process.execPath, [process.env.npm_execpath, 'run', script]);
}
function targetDirectory() {
  const result = run(cargo, ['metadata', '--format-version', '1', '--no-deps'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] });
  return JSON.parse(result.stdout).target_directory;
}
function prepare(release) {
  run(cargo, ['build', '--locked', '-p', 'codex-mobile-bridge', '--bins', ...(release ? ['--release'] : [])]);
  const binaries = path.join(root, 'src-tauri', 'binaries');
  fs.mkdirSync(binaries, { recursive: true });
  const obsolete = path.join(binaries, 'codex-mobile-runtime' + suffix);
  if (fs.existsSync(obsolete)) fs.unlinkSync(obsolete);
  const output = path.join(targetDirectory(), release ? 'release' : 'debug');
  for (const name of helpers) fs.copyFileSync(path.join(output, name + suffix), path.join(binaries, name + suffix));
  fs.writeFileSync(path.join(root, 'src-tauri', 'release.json'), JSON.stringify({
    backend: 'rust', runtimeBoundary: 2, version: manifest.version, platform: process.platform, arch: process.arch,
  }) + '\n');
}
function tauri(args) { run(process.execPath, [path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js'), ...args]); }

async function verifyPackage(resources) {
  const temporary = fs.mkdtempSync(path.join(root, '.local', 'native-smoke-'));
  const listener = net.createServer();
  await new Promise((resolve, reject) => { listener.once('error', reject); listener.listen(0, '127.0.0.1', resolve); });
  const { port } = listener.address(); await new Promise(resolve => listener.close(resolve));
  const child = spawn(path.join(resources, 'bin', 'codex-mobile-bridge' + suffix), ['--web-root', path.join(resources, 'mobile')], {
    cwd: resources, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
      ...env, CODEX_MOBILE_HOME: path.join(temporary, 'data'), CODEX_HOME: path.join(temporary, 'codex'),
      CODEX_MOBILE_RUNTIME_DIRECTORY: path.join(temporary, 'runtime'), CODEX_MOBILE_HOST: '127.0.0.1', CODEX_MOBILE_PORT: String(port),
    },
  });
  let log = ''; child.stdout.on('data', chunk => { log += chunk; }); child.stderr.on('data', chunk => { log += chunk; });
  const exited = new Promise((resolve, reject) => { child.once('exit', resolve); child.once('error', reject); });
  const address = `http://127.0.0.1:${port}`;
  try {
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      if (child.exitCode !== null) throw new Error(`运行包服务启动失败：${log}`);
      ready = await fetch(address + '/version.json', { signal: AbortSignal.timeout(500) }).then(r => r.ok, () => false);
      if (ready) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (!ready) throw new Error(`运行包服务未就绪：${log}`);
    const response = await fetch(address + '/?mobile=1');
    const html = await response.text();
    if (!response.ok || !html.includes('<title>Codex Mobile</title>')) throw new Error('运行包缺少可用手机入口。');
    const script = html.match(/src="(\/assets\/[^\"]+\.js)"/)?.[1];
    if (!script || !(await fetch(address + script)).ok) throw new Error('运行包网页脚本无法加载。');
    if ((await fetch(address + '/api/projects')).status !== 401) throw new Error('运行包认证检查未通过。');
    console.log('运行包验证通过：独立 Rust 服务、手机资源、版本和认证。');
  } finally { child.kill(); await exited; }
}

async function packageNative() {
  const platform = { win32: 'windows', darwin: 'macos' }[process.platform];
  if (!platform) throw new Error('请在 Windows 或 macOS 本机生成对应平台的安装包。');
  if (process.env.GITHUB_REF_TYPE === 'tag' && process.env.GITHUB_REF_NAME !== `v${manifest.version}`) throw new Error('发布标签必须与应用版本一致。');
  tauri(['build', '--bundles', process.platform === 'win32' ? 'nsis' : 'app,dmg']);
  const output = path.join(targetDirectory(), 'release');
  const name = `codex-mobile-${manifest.version}-${platform}-${process.arch}`;
  fs.mkdirSync(path.join(root, '.local'), { recursive: true });
  const temporary = fs.mkdtempSync(path.join(root, '.local', 'native-package-'));
  const staging = path.join(temporary, name); fs.mkdirSync(staging);
  let resources = staging;
  if (process.platform === 'win32') {
    fs.copyFileSync(path.join(output, 'codex-mobile-desktop.exe'), path.join(staging, 'Codex Mobile.exe'));
    fs.cpSync(path.join(root, 'dist'), path.join(staging, 'mobile'), { recursive: true });
    fs.cpSync(path.join(root, 'src-tauri', 'binaries'), path.join(staging, 'bin'), { recursive: true });
  } else {
    fs.cpSync(path.join(output, 'bundle', 'macos', 'Codex Mobile.app'), path.join(staging, 'Codex Mobile.app'), { recursive: true });
    resources = path.join(staging, 'Codex Mobile.app', 'Contents', 'Resources');
  }
  fs.copyFileSync(path.join(root, 'src-tauri', 'release.json'), path.join(staging, 'release.json'));
  fs.copyFileSync(path.join(root, 'README.md'), path.join(staging, 'README.md'));
  await verifyPackage(resources);
  const release = path.join(root, 'release'); fs.mkdirSync(release, { recursive: true });
  const archive = path.join(release, name + '.zip');
  if (fs.existsSync(archive)) fs.unlinkSync(archive);
  if (process.platform === 'win32') {
    run('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
      'Add-Type -AssemblyName System.IO.Compression.FileSystem; [System.IO.Compression.ZipFile]::CreateFromDirectory($env:CM_PACKAGE_SOURCE, $env:CM_PACKAGE_ARCHIVE, [System.IO.Compression.CompressionLevel]::Optimal, $true)'],
    { env: { ...env, CM_PACKAGE_SOURCE: staging, CM_PACKAGE_ARCHIVE: archive } });
  } else { run('ditto', ['-c', '-k', '--norsrc', '--noextattr', '--noqtn', '--keepParent', staging, archive]); }
  const bundled = path.join(output, 'bundle', process.platform === 'win32' ? 'nsis' : 'dmg');
  const installerExtension = process.platform === 'win32' ? '.exe' : '.dmg';
  const installer = fs.readdirSync(bundled).find(name =>
    name.includes(`_${manifest.version}_`) && name.endsWith(installerExtension),
  );
  if (!installer) throw new Error(`Tauri 没有生成当前版本 ${manifest.version} 的安装程序。`);
  const installerPath = path.join(release, `codex-mobile-${manifest.version}-${platform}-${process.arch}-setup${installerExtension}`);
  fs.copyFileSync(path.join(bundled, installer), installerPath);
  for (const file of [archive, installerPath]) {
    const hash = createHash('sha256').update(fs.readFileSync(file)).digest('hex');
    fs.writeFileSync(file + '.sha256', `${hash}  ${path.basename(file)}\n`);
    console.log(`已生成：${file}`);
  }
}

const mode = process.argv[2];
if (mode === 'dev-assets' || mode === 'build-assets') {
  npm(mode === 'dev-assets' ? 'build:mobile' : 'build:web');
  prepare(mode === 'build-assets' && process.env.TAURI_ENV_DEBUG !== 'true');
  if (mode === 'dev-assets') npm('dev:desktop');
} else if (mode === 'dev') {
  tauri(['dev']);
} else if (mode === 'build') {
  tauri(['build', '--bundles', process.platform === 'win32' ? 'nsis' : 'app,dmg']);
} else if (mode === 'package') {
  await packageNative();
} else if (mode === 'bridge') {
  if (!fs.existsSync(path.join(root, 'dist', 'index.html'))) npm('build:mobile');
  run(cargo, ['run', '--locked', '-p', 'codex-mobile-bridge', '--bin', 'codex-mobile-bridge', '--', '--web-root', path.join(root, 'dist')]);
} else if (mode === 'test') {
  run(cargo, ['test', '--locked', '-p', 'codex-mobile-bridge', '-p', 'codex-mobile-relay', '--tests']);
} else if (mode === 'test-relay') {
  run(cargo, ['test', '--locked', '-p', 'codex-mobile-bridge', '--test', 'relay', '--', '--include-ignored', '--nocapture'], {
    env: { ...env, CODEX_MOBILE_TEST_NODE: process.execPath },
  });
} else if (mode === 'check') {
  npm('build:web');
  prepare(false);
  run(cargo, ['check', '--locked', '--workspace', '--all-targets']);
} else {
  throw new Error('未知构建操作。使用 npm run dev、build:native、package、start:bridge 或 test:rust。');
}
