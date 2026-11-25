import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { ref } from 'vue';
import { ApiError } from '../../mobile/src/api.ts';
import { conversationParams } from '../../mobile/src/features/conversation/links.ts';

const source = readFileSync(new URL('../../mobile/src/App.vue', import.meta.url), 'utf8').split('<script setup lang="ts">')[1].split('</script>')[0];
const ast = ts.createSourceFile('App.ts', source, ts.ScriptTarget.Latest, true);
const methods = new Set(['initializeConnection', 'enterApp', 'login', 'takeConnectionKey', 'connectScanned']);
const compiled = ts.transpileModule(ast.statements.filter(node => ts.isFunctionDeclaration(node) && methods.has(node.name?.text))
  .map(node => node.getText(ast)).join('\n'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
const mainAst = ts.createSourceFile('main.ts', readFileSync(new URL('../../mobile/src/main.ts', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true);
const mainCompiled = ts.transpileModule(mainAst.statements.filter(node => !ts.isImportDeclaration(node)).map(node => node.getText(mainAst)).join('\n'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  transformers: { before: [context => root => {
    const visit = node => ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword
      ? ts.factory.updateCallExpression(node, ts.factory.createIdentifier('loadModule'), node.typeArguments, node.arguments)
      : ts.visitEachChild(node, visit, context);
    return ts.visitNode(root, visit);
  }] },
}).outputText;

function bootstrap(url, session, cloud = false) {
  const page = Promise.withResolvers(), mounted = Promise.withResolvers();
  const calls = [], imports = [], failures = [];
  const bindings = {
    document: { documentElement: { dataset: { theme: 'light' } } }, location: new URL(url),
    relayMode: () => cloud,
    request(path) { calls.push(path); return session; },
    loadModule(name) {
      imports.push(name);
      return name === 'vue' ? Promise.resolve({ createApp: (Page, props) => ({ mount: target => mounted.resolve({ Page, props, target }) }) }) : page.promise;
    },
    window: { dispatchEvent: event => failures.push(event.type) }, Event,
  };
  const initialSession = new Function(...Object.keys(bindings), mainCompiled + '\nreturn initialSession;')(...Object.values(bindings));
  return { initialSession, calls, imports, failures, showPage: () => page.resolve({ default: { name: 'FixturePage' } }), mounted: mounted.promise };
}

function fixture(url = 'https://fixture.invalid/', respond = async () => ({ ok: true }), operations = Promise.resolve()) {
  const location = new URL(url), calls = [], addresses = [];
  let refreshes = 0;
  const state = {
    capabilities: ref({}), operationsReady: ref(false),
    authenticated: ref(false), booting: ref(true), loggingIn: ref(false), loginKey: ref(''),
    loginError: ref(''), loginFailureStatus: ref(null), scanOpen: ref(true),
    selectedId: ref(''), selectedProjectId: ref(''), selectedThread: ref(null), projects: ref([]), navigationOpen: ref(true),
  };
  const bindings = {
    loadOperations: async () => { await operations; state.operationsReady.value = true; },
    ...state, location, ApiError, props: {}, conversationParams,
    history: { state: {}, replaceState(_state, _title, next) { addresses.push(next); location.href = new URL(next, location).href; } },
    request(url, init = {}) {
      const call = { url, method: init.method || 'GET', ...(init.body ? { body: JSON.parse(init.body) } : {}) };
      calls.push(call); return respond(call);
    },
    refreshCatalog: async () => { refreshes++; }, threadProjectId: thread => thread.projectId,
    updateAddress() {}, startNewChat() {}, openProject() {},
  };
  const actions = new Function(...Object.keys(bindings), compiled + '\nreturn { initializeConnection, enterApp, login, takeConnectionKey, connectScanned };')(...Object.values(bindings));
  return { ...state, ...actions, calls, addresses, location, get refreshes() { return refreshes; } };
}

test('云网页启动只加载云入口，配对前不会请求桌面 HTTP API', async () => {
  const boot = bootstrap('https://fixture.invalid/', undefined, true);
  assert.deepEqual(boot.calls, []);
  assert.deepEqual(boot.imports, ['./features/relay/RelayHome.vue', 'vue']);
  boot.showPage();
  assert.equal((await boot.mounted).target, '#app');
});

test('首次连接复用预取会话，聊天和目录无需等待未决操作', async () => {
  const session = Promise.withResolvers();
  const operations = Promise.withResolvers();
  const boot = bootstrap('https://fixture.invalid/?thread=chat&project=project', session.promise);
  assert.deepEqual(boot.calls, ['/api/session']);
  assert.deepEqual(boot.imports, ['./App.vue', 'vue']);
  boot.showPage();
  const mounted = await boot.mounted;
  assert.equal(mounted.props.initialSession, session.promise);
  assert.equal(mounted.target, '#app');
  const f = fixture('https://fixture.invalid/?thread=chat&project=project', undefined, operations.promise);
  f.operationsReady.value = true;
  const entering = f.initializeConnection(mounted.props.initialSession);
  assert.deepEqual(f.calls, []);
  assert.equal(f.authenticated.value, false);
  session.resolve({ ok: true });
  await Promise.resolve();
  assert.equal(f.authenticated.value, true);
  assert.equal(f.selectedId.value, 'chat');
  assert.equal(f.selectedProjectId.value, 'project');
  assert.equal(f.refreshes, 1);
  assert.equal(f.operationsReady.value, false);
  operations.resolve();
  await entering;
  assert.equal(f.operationsReady.value, true);
  assert.equal(f.booting.value, false);
  assert.deepEqual(boot.failures, []);
});

test('预取的 401 和网络失败进入原登录分支，重试发起新会话校验', async () => {
  for (const failure of [new ApiError('尚未配对', 401), new ApiError('电脑连接服务不可用', 0)]) {
    const boot = bootstrap('https://fixture.invalid/', Promise.reject(failure));
    boot.showPage();
    const mounted = await boot.mounted;
    const f = fixture();
    await f.initializeConnection(mounted.props.initialSession);
    assert.equal(f.authenticated.value, false);
    assert.equal(f.booting.value, false);
    assert.equal(f.refreshes, 0);
    assert.equal(f.loginError.value, failure.status === 401 ? '' : failure.message);
    assert.equal(f.loginFailureStatus.value, failure.status === 401 ? null : 0);
    assert.deepEqual(f.calls, []);
    assert.deepEqual(boot.failures, []);
    await f.initializeConnection();
    assert.deepEqual(f.calls, [{ url: '/api/session', method: 'GET' }]);
    assert.equal(f.authenticated.value, true);
    assert.equal(f.loginError.value, '');
    assert.equal(f.loginFailureStatus.value, null);
    assert.equal(f.refreshes, 1);
  }
});

test('带连接密钥的启动不预取旧会话，登录成功后使用新会话校验', async () => {
  const key = 'fixture-key-'.padEnd(32, 'x');
  const url = 'https://fixture.invalid/?thread=chat#key=' + key;
  const boot = bootstrap(url, Promise.resolve({ stale: true }));
  assert.deepEqual(boot.calls, []);
  assert.equal(boot.initialSession, undefined);
  boot.showPage();
  const mounted = await boot.mounted;
  const login = Promise.withResolvers();
  const f = fixture(url, call => call.url === '/api/login' ? login.promise : Promise.resolve({ ok: true }));
  assert.equal(f.takeConnectionKey(), true);
  assert.equal(f.location.hash, '');
  const entering = f.initializeConnection(mounted.props.initialSession);
  assert.deepEqual(f.calls, [{ url: '/api/login', method: 'POST', body: { accessKey: key } }]);
  assert.equal(f.authenticated.value, false);
  login.resolve({ ok: true });
  await entering;
  assert.deepEqual(f.calls[1], { url: '/api/session', method: 'GET' });
  assert.equal(f.calls.length, 2);
  assert.equal(f.authenticated.value, true);
  assert.equal(f.selectedId.value, 'chat');
  assert.equal(f.loginKey.value, '');
});

test('首次预取失败后，手动登录和扫码都重新校验登录后的会话', async () => {
  for (const method of ['login', 'scan']) {
    const f = fixture();
    await f.initializeConnection(Promise.reject(new ApiError('尚未配对', 401)));
    const key = 'fixture-key-'.padEnd(32, 'x');
    if (method === 'login') { f.loginKey.value = key; await f.login(); }
    else await f.connectScanned(key);
    assert.deepEqual(f.calls, [
      { url: '/api/login', method: 'POST', body: { accessKey: key } },
      { url: '/api/session', method: 'GET' },
    ]);
    assert.equal(f.authenticated.value, true);
    assert.equal(f.refreshes, 1);
    if (method === 'scan') {
      assert.equal(f.selectedId.value, '');
      assert.equal(f.scanOpen.value, false);
      assert.equal(f.location.hash, '');
    }
  }
});
