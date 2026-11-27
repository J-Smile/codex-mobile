import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';

// 本地模拟复用真实构建页面；HTTP、SSE、存储和附件仅使用合成资料。
const webRoot = path.resolve('dist/mobile');
const output = path.resolve(process.argv[2] || 'test-results/browser');
fs.mkdirSync(output, { recursive: true });
const syntheticImage = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jf8kAAAAASUVORK5CYII=', 'base64');
const imageId = 'b'.repeat(32) + '.png';
const savedId = 'saved-operation-0001';
const source = { id: 'fixture-chat', title: '隔离来源聊天', status: 'idle' };
const project = { projectId: 'fixture-project', label: '隔离项目', path: '/fixture/project' };
const referencedChat = { id: 'fixture-reference-chat', title: '参考 [聊天]', cwd: project.path, status: 'idle' };
const referencedFile = { id: '/fixture/project/UI [review].vue', kind: 'file', label: 'UI [review].vue',
  description: 'UI [review].vue', mention: '[UI \\[review\\].vue](</fixture/project/UI [review].vue>)' };
const capabilities = {
  native: true, readLive: true, send: true, create: true, images: true,
  imageReferences: true, modelSelection: true, interactions: false,
  interrupt: false, queue: false, catalog: false, settings: false, goals: false,
};
const cases = [
  { name: 'normal-text', ids: ['SEND-003'], enabled: true },
  { name: 'normal-image-only', ids: ['SEND-004'], image: true, enabled: true },
  { name: 'accepted-receipt-lost', ids: ['OPS-002'], local: true, terminal: 'accepted', image: true, enabled: true },
  { name: 'not-dispatched-receipt-lost', ids: ['OPS-003'], local: true, terminal: 'not-dispatched', enabled: true },
  { name: 'missing-record', ids: ['OPS-005'], local: true, enabled: false },
  { name: 'other-chat-pending', ids: ['OPS-009'], local: true, other: true, enabled: true },
  { name: 'operations-read-failed', ids: ['OPS-005'], operationFailure: true, enabled: false },
  { name: 'question-disabled', ids: ['INTERACT-005'], question: true, enabled: true },
  { name: 'unknown-not-replayed', ids: ['OPS-004'], local: true, terminal: 'unknown', enabled: false },
  { name: 'closed-unknown-allows-new-intent', ids: ['OPS-006'], local: true, terminal: 'unknown', closed: true, enabled: true },
  { name: 'old-server-keeps-draft', ids: ['OPS-005'], local: true, oldServer: true, enabled: false },
  { name: 'expired-image-draft', ids: ['SEND-006'], restoredImage: true, enabled: false },
  { name: 'unknown-close-in-chat', ids: ['OPS-004', 'OPS-006'], local: true, terminal: 'unknown', closeUnknown: true, enabled: false },
  { name: 'native-question-read-only', ids: ['INTERACT-001', 'INTERACT-005'], question: true, readonlyQuestion: true, enabled: true },
  { name: 'native-question-open-desktop', ids: ['CONTROL-007', 'INTERACT-005'], question: true, readonlyQuestion: true, desktopNavigation: true, enabled: true },
];
let scenario;
const calls = [];
const results = [];
const pageErrors = [];

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://127.0.0.1');
  const json = (value, status = 200) => {
    response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    response.end(JSON.stringify(value));
  };
  if (url.pathname.startsWith('/api/')) {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const bytes = Buffer.concat(chunks);
    const input = request.headers['content-type']?.includes('application/json') && bytes.length ? JSON.parse(bytes) : null;
    calls.push({ scenario: scenario.name, viewport: scenario.width, method: request.method,
      path: url.pathname, query: url.search, input, contract: request.headers['x-codex-mobile-contract'] });
    const currentCapabilities = { ...capabilities, desktopNavigation: !!scenario.desktopNavigation, contextReferences: !!scenario.references,
      ...(scenario.historyOnly ? { native: false, readLive: false, send: false, create: false, images: false, modelSelection: false } : {}) };
    if (url.pathname === '/api/session') return json({ capabilities: currentCapabilities });
    if (url.pathname === '/api/update-settings') return json({
      version: scenario.version || '0.3.28', mode: 'automatic',
      update: scenario.update || { phase: 'idle', message: '' },
    });
    if (url.pathname === '/api/update') return json({ accepted: true });
    if (url.pathname === '/api/operations') {
      if (scenario.operationFailure) return json({ message: '隔离网络失败' }, 503);
      const value = { operations: [] };
      if (url.searchParams.has('ids') && !scenario.oldServer) {
        value.results = url.searchParams.get('ids').split(',').map(id => scenario.terminal
          ? { id, kind: scenario.restoreCreation ? 'create' : 'message', target: scenario.restoreCreation ? 'new:' + project.projectId : scenario.other ? 'other-chat' : source.id,
            phase: scenario.terminal, closed: !!scenario.closed,
            receipt: scenario.terminal === 'accepted' ? scenario.restoreCreation
              ? { threadId: 'fixture-created', firstTurnAccepted: false } : { status: 'accepted' } : null }
          : { id, phase: 'missing' });
      }
      return json(value);
    }
    if (url.pathname === '/api/operations/' + savedId && request.method === 'POST') {
      scenario.closed = true;
      return json({ closed: true });
    }
    if (url.pathname === '/api/projects') return json({ projects: url.searchParams.has('threadId') ? [project] : [], capabilities: currentCapabilities });
    if (url.pathname === '/api/context') {
      if (scenario.contextFailure) return json({ message: '隔离文件查询暂时失败' }, 503);
      return json({ mode: 'references', plugins: [], browserTabs: [],
        files: url.searchParams.get('query')?.toLowerCase().includes('review') ? [referencedFile] : [],
        pluginMessage: '当前连接未开放插件上下文。', browserMessage: '当前连接未开放浏览器上下文。' });
    }
    if (url.pathname === '/api/threads') {
      if (request.method === 'GET') return json({ threads: scenario.references ? [source, referencedChat] : [source], nextCursor: null });
      scenario.createdInputs.push(input);
      return scenario.rejectCreation
        ? json({ message: '连接协议已更新，请保留草稿后刷新页面。', outcome: 'not-dispatched' }, 426)
        : json({ threadId: 'fixture-created', firstTurnAccepted: !scenario.failedCreation });
    }
    if (url.pathname === '/api/model-options') return json({ recentModels: ['gpt-6.1-sol'], efforts: ['low', 'high'] });
    if (url.pathname === '/api/images' && request.method === 'POST') return json({
      id: imageId, name: 'synthetic.png', size: syntheticImage.length, type: 'image/png', url: '/api/images/' + imageId,
    });
    if (url.pathname.startsWith('/api/images/')) {
      if (scenario.restoredImage) return json({ message: '合成图片引用已失效' }, 404);
      response.setHeader('content-type', 'image/png');
      return response.end(syntheticImage);
    }
    if (url.pathname.endsWith('/events')) {
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
      const thread = url.pathname.includes('fixture-created')
        ? { id: 'fixture-created', title: '已创建隔离聊天', projectId: project.projectId }
        : scenario.currentModel ? { ...source, model: 'gpt-6.2-sol', reasoningEffort: 'xhigh' } : source;
      const interactions = scenario.question ? [{
        id: 'question', kind: 'questions', source: 'app-server', title: '隔离测试问题', replySupported: !scenario.readonlyQuestion,
        skipSupported: !scenario.readonlyQuestion, questions: [{ id: 'choice', question: '选择测试项', header: '测试', options: [{ label: '继续', description: '合成选项说明' }] }],
      }] : [];
      const turns = scenario.composerMenu ? [{
        id: 'fixture-code-turn', status: 'completed', items: [{
          id: 'fixture-code', type: 'agentMessage',
          text: [...Array.from({ length: 36 }, (_, index) => '代码块之前的合成正文 ' + (index + 1)),
            '```bash\nprintf "composer menu"\n```',
            ...Array.from({ length: 36 }, (_, index) => '代码块之后的合成正文 ' + (index + 1))].join('\n\n'),
        }],
      }] : [];
      const snapshot = { thread, turns, nextCursor: null, connection: scenario.historyOnly ? 'history' : 'live',
        capabilities: { ...currentCapabilities, ...(scenario.failedCreation && thread.id === 'fixture-created' ? { send: false } : {}) },
        syncMode: 'desktop-poll', firstTurnPending: !!(scenario.restoreCreation || scenario.failedCreation) && thread.id === 'fixture-created', interactions };
      response.write('event: snapshot\ndata: ' + JSON.stringify(snapshot) + '\n\n');
      if ((scenario.restoreCreation || scenario.failedCreation) && thread.id === 'fixture-created') {
        scenario.createdStream = response;
        scenario.createdSnapshot = snapshot;
      }
      return;
    }
    if (url.pathname.endsWith('/open')) return scenario.desktopNavigation ? json({ ok: true }) : json({ message: '未开放桌面导航' }, 503);
    if (url.pathname.endsWith('/hooks')) return json({ mode: 'read-only', pendingReview: null,
      sources: [{ label: '合成项目配置', path: '/fixture/project/.codex/hooks.json', config: { hooks: { SessionStart: [] } } }], issues: [] });
    if (url.pathname.endsWith('/history')) return json({ thread: source, turns: [], nextCursor: null });
    if (url.pathname.endsWith('/messages')) {
      if (scenario.failedCreation) {
        scenario.finishMessage = () => json({ accepted: true });
        return;
      }
      return json({ accepted: true });
    }
    return json({});
  }
  if (scenario.webVersion && url.pathname === '/version.json') return json({ version: scenario.webVersion });
  if (scenario.webVersion && url.pathname === `/assets/upgrade-probe-${scenario.webVersion}.js`) {
    scenario.releaseUpgradeAsset = (status = 200) => {
      response.writeHead(status, { 'content-type': 'text/javascript', 'cache-control': 'no-store' });
      response.end('export const prepared = true;');
    };
    if (!scenario.holdUpgradeAsset) scenario.releaseUpgradeAsset();
    return;
  }
  const file = path.resolve(webRoot, '.' + (url.pathname === '/' ? '/index.html' : url.pathname));
  if (!file.startsWith(webRoot + path.sep)) { response.writeHead(403); return response.end(); }
  try {
    response.setHeader('content-type', ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css',
      '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png' })[path.extname(file)] || 'application/octet-stream');
    if (url.pathname === '/' && scenario.webUpgrade) {
      const html = fs.readFileSync(file, 'utf8');
      return response.end(scenario.webVersion ? html.replace(/(<meta name="app-version" content=")[^"]+/, '$1' + scenario.webVersion) : html);
    }
    if (url.pathname === '/sw.js' && scenario.webVersion) {
      response.setHeader('cache-control', 'no-store');
      return response.end(fs.readFileSync(file, 'utf8')
        .replace(/const buildVersion = [^;]+;/, `const buildVersion = '${scenario.webVersion}';`)
        .replace(/const buildAssets = (\[[^;]+\]);/, (_, assets) =>
          `const buildAssets = ${JSON.stringify([...JSON.parse(assets), `/assets/upgrade-probe-${scenario.webVersion}.js`])};`));
    }
    response.end(fs.readFileSync(file));
  } catch { response.writeHead(404); response.end(); }
});
const browser = await chromium.launch({ headless: true });
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = 'http://127.0.0.1:' + server.address().port;

async function pageFor(viewport) {
  const context = await browser.newContext({ viewport, hasTouch: true, isMobile: viewport.width < 900,
    serviceWorkers: scenario.webUpgrade ? 'allow' : 'block', reducedMotion: 'reduce' });
  await context.route('**/*', route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
  if (scenario.local) await context.addInitScript(({ target, requestId, current, once }) => {
    if (once && sessionStorage.getItem('fixture-initialized')) return;
    if (once) sessionStorage.setItem('fixture-initialized', 'true');
    localStorage.setItem('codex-mobile-pending-v2', JSON.stringify([[target, { requestId }]]));
    sessionStorage.setItem('codex-mobile-drafts', JSON.stringify({ [current]: '恢复时保留的新草稿' }));
  }, { target: scenario.restoreCreation ? 'new:' + project.projectId : scenario.other ? 'other-chat' : source.id,
    requestId: savedId, current: scenario.restoreCreation ? 'new:' + project.projectId : source.id, once: !!scenario.webUpgrade });
  if (scenario.restoredImage) await context.addInitScript(({ target, id, size }) => {
    localStorage.setItem('codex-mobile-image-drafts', JSON.stringify({ [target]: [{
      id, name: 'expired.png', url: '/api/images/' + id, size, type: 'image/png',
    }] }));
  }, { target: source.id, id: imageId, size: syntheticImage.length });
  const page = await context.newPage();
  page.setDefaultTimeout(12_000);
  page.on('pageerror', error => pageErrors.push({ scenario: scenario.name, width: viewport.width, message: error.message }));
  return { context, page };
}

async function verifyComposerMenu(page) {
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto(origin + '/?thread=' + source.id);
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.fill('打开菜单时保留草稿');
  await page.getByRole('button', { name: '更多操作', exact: true }).click();
  const addPhoto = page.getByRole('menuitem', { name: '添加照片' });
  const codeHeader = page.locator('.md-editor-code-head');
  await codeHeader.waitFor({ state: 'attached' });
  const photoBounds = await addPhoto.boundingBox();
  const point = { x: photoBounds.x + photoBounds.width / 2, y: photoBounds.y + photoBounds.height / 2 };
  await codeHeader.evaluate((header, point) => {
    const scroller = header.closest('.conversation-scroll');
    const codeBlock = header.closest('.md-editor-code');
    scroller.scrollTop += codeBlock.getBoundingClientRect().top + header.offsetHeight / 2 - point.y;
  }, point);
  const layers = await page.evaluate(point => {
    const elements = document.elementsFromPoint(point.x, point.y);
    return { includesHeader: elements.some(element => element.closest('.md-editor-code-head')),
      menuOnTop: !!elements[0]?.closest('#composer-menu') };
  }, point);
  assert.ok(layers.includesHeader, '测试点必须实际与代码块标题重叠');
  assert.ok(layers.menuOnTop, '重叠处的点击必须由添加内容菜单接收');
  await page.screenshot({ path: path.join(output, 'composer-menu-open-' + scenario.width + '.png') });
  const [picker] = await Promise.all([page.waitForEvent('filechooser'), addPhoto.click()]);
  await picker.setFiles([]);
  assert.equal(await input.innerText(), '打开菜单时保留草稿');
}

async function verifyCurrentModel(page) {
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto(origin + '/?thread=' + source.id);
  const settings = page.getByRole('button', { name: '模型与思考强度', exact: true });
  await settings.click();
  const model = page.getByLabel('模型', { exact: true });
  const effort = page.getByLabel('思考强度', { exact: true });
  const modelChoice = await model.locator('option:checked').textContent();
  const effortChoice = await effort.locator('option:checked').textContent();
  assert.deepEqual([modelChoice, effortChoice], ['6.2 Sol（当前）', 'Xhigh（当前）']);
  assert.deepEqual([await model.inputValue(), await effort.inputValue()], ['', '']);
  const dialog = page.locator('dialog[open].sheet-ready');
  await dialog.screenshot({ path: path.join(output, 'current-model-open-' + scenario.width + '.png'), animations: 'disabled' });
  await page.getByRole('button', { name: '用于下次发送', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  await page.getByRole('textbox', { name: '消息内容' }).fill('沿用当前模型和思考强度');
  const [response] = await Promise.all([
    page.waitForResponse(response => new URL(response.url()).pathname === '/api/threads/' + source.id + '/messages' && response.request().method() === 'POST'),
    page.getByRole('button', { name: '发送消息', exact: true }).click(),
  ]);
  assert.ok(response.ok());
  const message = response.request().postDataJSON();
  assert.equal(message.model, undefined);
  assert.equal(message.thinking, undefined);
}

async function verifySubmission(page) {
  await page.goto(origin + '/?thread=' + source.id);
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.waitFor();
  if (scenario.local) assert.equal(await input.innerText(), '恢复时保留的新草稿', '恢复旧回执不能清除当前草稿');
  if (scenario.question) {
    if (scenario.readonlyQuestion) {
      await page.getByText('选择测试项', { exact: true }).waitFor();
      await page.getByRole('listitem').filter({ hasText: '继续：合成选项说明' }).waitFor();
      assert.equal(await page.locator('.interaction-submit').count(), 0);
      const openDesktop = page.getByRole('button', { name: '在电脑上打开', exact: true });
      assert.equal(await openDesktop.count(), scenario.desktopNavigation ? 1 : 0);
      if (scenario.desktopNavigation) {
        await input.fill('打开桌面不清草稿');
        await openDesktop.click();
        await page.getByText('已在电脑上打开此对话', { exact: true }).waitFor();
        assert.equal(await input.innerText(), '打开桌面不清草稿');
        const opens = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.path.endsWith('/open'));
        assert.equal(opens.length, 1);
        assert.equal(opens[0].path, '/api/threads/' + source.id + '/open');
        assert.equal(opens[0].contract, '2');
      }
    } else {
      await page.locator('.interaction-submit').waitFor();
      assert.equal(await page.locator('.interaction-submit').isDisabled(), true);
      assert.doesNotMatch(await page.locator('.interaction-status').innerText(), /重新连接后即可回复/);
    }
    assert.equal(await page.locator('.composer-dock').evaluate(element => element.inert), false, '不可回复卡片不能锁住普通输入');
  }
  if (scenario.image) {
    await input.fill('');
    await page.getByLabel('选择发送的图片').setInputFiles({ name: 'synthetic.png', mimeType: 'image/png', buffer: syntheticImage });
    await page.locator('.composer-attachments img').waitFor();
    await page.waitForFunction(() => !document.querySelector('.composer-attachments .image-state'));
  } else await input.fill('固定回归测试文字');
  if (scenario.restoredImage) {
    const expired = page.getByText('已失效', { exact: true });
    await expired.waitFor();
    assert.match(await expired.getAttribute('title'), /移除后重新选择/);
  }
  const send = page.getByRole('button', { name: '发送消息', exact: true });
  await page.waitForFunction(expected => document.querySelector('[aria-label="发送消息"]')?.disabled === !expected, scenario.enabled);
  if (scenario.enabled) {
    await send.click();
    await page.waitForFunction(() => document.querySelector('[aria-label="消息内容"]').textContent === '');
    if (scenario.image) await page.locator('.composer-attachments').waitFor({ state: 'hidden' });
  } else {
    await input.press('Control+Enter');
    assert.equal(await input.innerText(), '固定回归测试文字');
  }
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.path.endsWith('/messages'));
  assert.equal(writes.length, scenario.enabled ? 1 : 0);
  if (writes.length) {
    assert.equal(writes[0].contract, '2');
    assert.deepEqual(writes[0].input.images, scenario.image ? [imageId] : []);
    assert.equal(writes[0].input.prompt, scenario.image ? '' : '固定回归测试文字');
  }
  if (scenario.local) assert.ok(calls.some(call => call.scenario === scenario.name && call.viewport === scenario.width
    && call.path === '/api/operations' && new URLSearchParams(call.query).get('ids') === savedId));
  if (scenario.restoredImage) {
    assert.equal(calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'HEAD').length, 1);
    await page.getByRole('button', { name: '移除图片：expired.png' }).click();
    assert.equal(await input.innerText(), '固定回归测试文字');
    await send.click();
    await page.waitForFunction(() => document.querySelector('[aria-label="消息内容"]').textContent === '');
    const recovered = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.path.endsWith('/messages'));
    assert.equal(recovered.length, 1);
    assert.deepEqual(recovered[0].input.images, []);
  }
  if (scenario.closeUnknown) {
    const composer = page.locator('.composer-region');
    const retry = composer.getByRole('button', { name: '重新核对', exact: true });
    const close = composer.getByRole('button', { name: '已核对，结束等待', exact: true });
    for (const button of [retry, close]) {
      await button.scrollIntoViewIfNeeded();
      await button.click({ trial: true });
    }
    const bounds = [];
    for (const button of [retry, close]) {
      const geometry = await button.evaluate(element => {
        const box = element.getBoundingClientRect();
        const clip = { top: 0, right: innerWidth, bottom: innerHeight, left: 0 };
        for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
          const style = getComputedStyle(ancestor);
          const rect = ancestor.getBoundingClientRect();
          if (/auto|scroll|hidden|clip/.test(style.overflowX)) {
            clip.left = Math.max(clip.left, rect.left); clip.right = Math.min(clip.right, rect.right);
          }
          if (/auto|scroll|hidden|clip/.test(style.overflowY)) {
            clip.top = Math.max(clip.top, rect.top); clip.bottom = Math.min(clip.bottom, rect.bottom);
          }
        }
        return { box: box.toJSON(), clip, label: element.textContent };
      });
      const { box, clip, label } = geometry;
      assert.ok(box.width >= 44 && box.height >= 44, `${label}触控范围至少44px`);
      assert.ok(box.left >= clip.left - 0.5 && box.right <= clip.right + 0.5
        && box.top >= clip.top - 0.5 && box.bottom <= clip.bottom + 0.5, `${label}完整位于视口和祖先可见区域`);
      bounds.push(box);
    }
    if (Math.abs(bounds[0].top - bounds[1].top) < 1) {
      assert.ok(bounds[1].left - bounds[0].right >= 8, '同行核对操作间距至少8px');
    } else assert.ok(bounds[1].top - bounds[0].bottom >= 4, '换行核对操作间距至少4px');
    await page.screenshot({ path: path.join(output, `${scenario.name}-controls-${scenario.width}.png`), animations: 'disabled' });
    await Promise.all([
      page.waitForResponse(response => new URL(response.url()).pathname === '/api/operations'),
      composer.getByRole('button', { name: '重新核对', exact: true }).click(),
    ]);
    assert.equal(await send.isDisabled(), true, '再次核对unknown不能自动释放');
    page.once('dialog', dialog => dialog.dismiss());
    await composer.getByRole('button', { name: '已核对，结束等待', exact: true }).click();
    assert.equal(calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST').length, 0);
    page.once('dialog', dialog => dialog.accept());
    await composer.getByRole('button', { name: '已核对，结束等待', exact: true }).click();
    await page.waitForFunction(() => !document.querySelector('[aria-label="发送消息"]').disabled);
    assert.equal(await input.innerText(), '固定回归测试文字');
    const beforeSend = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
    assert.equal(beforeSend.length, 1);
    assert.deepEqual(beforeSend[0].input, { confirm: 'close-unknown' });
    await send.click();
    await page.waitForFunction(() => document.querySelector('[aria-label="消息内容"]').textContent === '');
    const afterSend = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.path.endsWith('/messages'));
    assert.equal(afterSend.length, 1);
    assert.notEqual(afterSend[0].input.requestId, savedId);
  }
}

async function verifyCreation(page) {
  await page.goto(origin);
  await page.getByRole('button', { name: '新聊天', exact: true }).click();
  await page.locator('dialog[open]').getByRole('button', { name: source.title }).click();
  await page.locator('dialog[open]').getByRole('button', { name: project.label }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  const input = page.getByRole('textbox', { name: '消息内容' });
  const options = page.getByRole('button', { name: '模型与思考强度', exact: true });
  assert.match(await options.innerText(), /桌面默认/);
  await options.click();
  await page.locator('#chat-model').selectOption('gpt-6.1-sol');
  await page.locator('#chat-effort').selectOption('high');
  await page.getByRole('button', { name: '用于下次发送', exact: true }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  await page.getByLabel('选择发送的图片').setInputFiles({ name: 'synthetic.png', mimeType: 'image/png', buffer: syntheticImage });
  await input.fill('新建发送隔离验证');
  const send = page.getByRole('button', { name: '创建对话并发送' });
  await send.click();
  await page.getByRole('alert').filter({ hasText: '连接协议已更新' }).waitFor();
  assert.equal(await input.innerText(), '新建发送隔离验证');
  assert.equal(await page.locator('.composer-attachments img').count(), 1);
  scenario.rejectCreation = false;
  await send.click();
  await page.getByRole('heading', { name: '已创建隔离聊天', exact: true }).waitFor();
  assert.equal(scenario.createdInputs.length, 2);
  for (const submitted of scenario.createdInputs) {
    assert.equal(submitted.sourceThreadId, source.id);
    assert.equal(submitted.projectId, project.projectId);
    assert.equal(submitted.model, 'gpt-6.1-sol');
    assert.equal(submitted.thinking, 'high');
    assert.deepEqual(submitted.images, [imageId]);
  }
  assert.notEqual(scenario.createdInputs[0].requestId, scenario.createdInputs[1].requestId);
  await input.fill('已结束聊天追加验证');
  await page.getByRole('button', { name: '发送消息', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('[aria-label="消息内容"]').textContent === '');
  await page.locator('.composer-attachments').waitFor({ state: 'hidden' });
  await page.getByRole('button', { name: /访问权限：/ }).click();
  await page.getByText('手机沿用当前对话的桌面权限。当前连接不支持修改权限，请在桌面调整。').waitFor();
  assert.equal(await page.locator('dialog[open] input[type=radio]').count(), 0);
  await page.getByRole('button', { name: '知道了', exact: true }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  assert.ok((await options.boundingBox()).height >= 44);
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
  assert.equal(writes.filter(call => call.path === '/api/threads').length, 2);
  assert.equal(writes.filter(call => call.path.endsWith('/messages')).length, 1);
  assert.ok(writes.every(call => call.contract === '2'));
}

async function verifyUpgrade(page) {
  await page.goto(origin + '/?thread=' + source.id);
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.fill('升级期间保留的合成草稿');
  async function openUpgrade() {
    await page.getByRole('button', { name: '对话更多操作' }).click();
    await page.getByRole('menuitem', { name: '对话设置', exact: true }).click();
    await page.getByRole('button', { name: /在线升级/ }).click();
    await page.getByRole('heading', { name: '在线升级', exact: true }).waitFor();
    await page.getByRole('dialog', { name: '设置', exact: true }).waitFor({ state: 'hidden' });
  }
  await openUpgrade();
  const initialSheet = page.getByRole('dialog', { name: '在线升级', exact: true });
  assert.equal(await initialSheet.locator('input, form, #update-source-content').count(), 0);
  assert.doesNotMatch(await initialSheet.innerText(), /token|更新来源|Repository/);
  await page.screenshot({ path: path.join(output, 'mobile-public-update-' + scenario.width + '.png') });
  scenario.update = { phase: 'checking', stage: 'downloading', message: '正在下载安装包：1.0 MiB / 2.0 MiB（50%）',
    downloadedBytes: 1048576, totalBytes: 2097152 };
  await page.getByRole('button', { name: '立即升级', exact: true }).click();
  await page.getByRole('dialog', { name: '在线升级', exact: true }).waitFor({ state: 'hidden' });
  await page.getByText(scenario.update.message, { exact: true }).waitFor();
  await openUpgrade();
  const background = page.getByRole('button', { name: '正在后台升级…', exact: true });
  assert.equal(await background.isDisabled(), true, '升级期间不能重复提交');
  const sheet = page.getByRole('dialog', { name: '在线升级', exact: true });
  for (const update of [
    { phase: 'checking', stage: 'downloading', message: '正在下载安装包：已下载 1.5 MiB', downloadedBytes: 1572864, totalBytes: null },
    { phase: 'checking', stage: 'verifying', message: '正在校验安装包…' },
    { phase: 'checking', stage: 'extracting', message: '正在解压安装包…' },
    { phase: 'ready', message: '更新已通过校验，正在准备切换…' },
    { phase: 'applying', message: '正在切换到新版本…' },
  ]) {
    scenario.update = update;
    await sheet.getByText(update.message, { exact: true }).waitFor();
    assert.equal(await background.isDisabled(), true);
    if (update.totalBytes === null) assert.doesNotMatch(await sheet.getByRole('status').innerText(), /%/);
  }
  scenario.version = '0.3.29';
  scenario.update = { phase: 'current', message: '已更新' };
  await page.getByRole('button', { name: '立即升级', exact: true }).waitFor();
  await page.getByRole('button', { name: '关闭在线升级' }).click();
  await sheet.waitFor({ state: 'hidden' });
  await page.getByText('已更新到 v0.3.29。', { exact: true }).waitFor();
  assert.equal(await input.innerText(), '升级期间保留的合成草稿');
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
  assert.equal(writes.length, 1);
  assert.equal(writes[0].path, '/api/update');
  assert.equal(writes[0].contract, '2');
}

async function verifyRecoveredCreation(page) {
  await page.goto(origin);
  await page.locator('[aria-label="已创建的聊天"]').getByRole('button', { name: '查看原聊天' }).waitFor();
  assert.equal(await page.getByRole('heading', { name: '已创建隔离聊天', exact: true }).count(), 0, '恢复回执不能抢导航');
  await page.getByRole('button', { name: '新聊天', exact: true }).click();
  await page.locator('dialog[open]').getByRole('button', { name: source.title }).click();
  await page.locator('dialog[open]').getByRole('button', { name: project.label }).click();
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.waitFor();
  assert.equal(await input.innerText(), '恢复时保留的新草稿');
  assert.equal(await page.getByRole('button', { name: '创建对话并发送' }).isDisabled(), true);
  await page.locator('.composer-region').getByRole('button', { name: '查看原聊天', exact: true }).click();
  await page.getByRole('heading', { name: '已创建隔离聊天', exact: true }).waitFor();
  await input.fill('首轮确认后的新意图');
  const send = page.getByRole('button', { name: '发送消息', exact: true });
  assert.equal(await send.isDisabled(), true, '首轮未接受仍禁止追加');
  assert.equal(calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST').length, 0);
  await page.getByRole('button', { name: '新建聊天', exact: true }).click();
  await page.locator('dialog[open]').getByRole('button', { name: project.label }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  assert.equal(await input.innerText(), '恢复时保留的新草稿');
  assert.equal(await page.getByRole('button', { name: '创建对话并发送' }).isDisabled(), true, '查看过原聊天但首轮未确认，返回同项目仍不能再次创建');
  const beforeConfirm = await page.evaluate(() => JSON.parse(localStorage.getItem('codex-mobile-pending-v2')));
  assert.deepEqual(beforeConfirm, [['new:' + project.projectId, { requestId: savedId }]]);
  await Promise.all([
    page.waitForResponse(response => new URL(response.url()).pathname === '/api/threads/fixture-created/events'),
    page.locator('.composer-region').getByRole('button', { name: '查看原聊天', exact: true }).click(),
  ]);
  await page.getByRole('heading', { name: '已创建隔离聊天', exact: true }).waitFor();
  assert.equal(await input.innerText(), '首轮确认后的新意图');
  assert.equal(await send.isDisabled(), true);
  scenario.createdStream.write('event: snapshot\ndata: ' + JSON.stringify({ ...scenario.createdSnapshot, firstTurnPending: false }) + '\n\n');
  await page.waitForFunction(() => !document.querySelector('[aria-label="发送消息"]').disabled);
  await send.click();
  await page.waitForFunction(() => document.querySelector('[aria-label="消息内容"]').textContent === '');
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
  assert.equal(writes.length, 1);
  assert.equal(writes[0].path, '/api/threads/fixture-created/messages');
  const saved = await page.evaluate(() => JSON.parse(sessionStorage.getItem('codex-mobile-drafts')));
  assert.equal(saved['new:' + project.projectId], '恢复时保留的新草稿');
  const pending = await page.evaluate(() => JSON.parse(localStorage.getItem('codex-mobile-pending-v2')));
  assert.deepEqual(pending, []);
}

async function verifyHooks(page) {
  await page.goto(origin + '/?thread=' + source.id);
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.fill('查看钩子保留原草稿');
  await page.getByRole('button', { name: '对话更多操作' }).click();
  await page.getByRole('menuitem', { name: '审核钩子', exact: true }).click();
  const sheet = page.getByRole('dialog', { name: '审核钩子', exact: true });
  await sheet.getByText('合成项目配置', { exact: true }).waitFor();
  await sheet.locator('summary').click();
  await sheet.getByText('/fixture/project/.codex/hooks.json', { exact: true }).waitFor();
  const navigate = sheet.getByRole('button', { name: '在电脑审阅', exact: true });
  assert.equal(await navigate.count(), scenario.desktopNavigation ? 1 : 0);
  if (scenario.desktopNavigation) {
    await navigate.scrollIntoViewIfNeeded();
    await navigate.click({ trial: true });
    const before = await navigate.boundingBox();
    const viewport = page.viewportSize();
    assert.ok(before.y >= 0 && before.y + before.height <= viewport.height, '滚动后桌面审阅按钮完整位于可视区');
    const scroll = await sheet.locator('.native-sheet-content').evaluate(element => ({
      top: element.scrollTop, height: element.clientHeight, content: element.scrollHeight,
    }));
    if (viewport.height < 450) {
      assert.ok(scroll.content > scroll.height && scroll.top > 0, '横屏长配置使用面板滚动到达桌面审阅按钮');
      await page.screenshot({ path: path.join(output, `${scenario.name}-navigation-reachable-${scenario.width}.png`), animations: 'disabled' });
    }
    await navigate.click();
    await sheet.getByRole('status').filter({ hasText: '已打开电脑上的对话' }).waitFor();
    await navigate.scrollIntoViewIfNeeded();
    await navigate.click({ trial: true });
    const after = await navigate.boundingBox();
    assert.ok(after.y >= 0 && after.y + after.height <= viewport.height, '导航成功提示出现后按钮仍可滚动到达');
  }
  await page.screenshot({ path: path.join(output, `${scenario.name}-panel-${scenario.width}.png`), animations: 'disabled' });
  await sheet.getByRole('button', { name: '关闭审核钩子' }).click();
  await sheet.waitFor({ state: 'hidden' });
  assert.equal(await input.innerText(), '查看钩子保留原草稿');
  const actual = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width);
  assert.equal(actual.filter(call => call.path.endsWith('/hooks')).length, 1);
  const writes = actual.filter(call => call.method === 'POST');
  assert.equal(writes.length, scenario.desktopNavigation ? 1 : 0);
  if (scenario.desktopNavigation) {
    assert.equal(writes[0].path, '/api/threads/' + source.id + '/open');
    assert.equal(writes[0].contract, '2');
  }
}

async function verifyFailedFirstTurn(page) {
  await page.goto(origin);
  await page.getByRole('button', { name: '新聊天', exact: true }).click();
  await page.locator('dialog[open]').getByRole('button', { name: source.title }).click();
  await page.locator('dialog[open]').getByRole('button', { name: project.label }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.fill('桌面默认模型的首次请求');
  await page.getByRole('button', { name: '创建对话并发送', exact: true }).click();
  await page.getByRole('heading', { name: '已创建隔离聊天', exact: true }).waitFor();
  assert.equal(scenario.createdInputs.length, 1);
  assert.equal(scenario.createdInputs[0].model, undefined, '未选择模型时不替换桌面默认');
  assert.equal(scenario.createdInputs[0].thinking, undefined);
  await input.fill('核对首轮期间保留的新草稿');
  const options = page.getByRole('button', { name: '模型与思考强度', exact: true });
  await options.click();
  await page.locator('#chat-model').selectOption('gpt-6.1-sol');
  await page.locator('#chat-effort').selectOption('high');
  await page.getByRole('button', { name: '用于下次发送', exact: true }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  assert.equal(await page.getByRole('button', { name: '发送消息', exact: true }).isDisabled(), true, '未收到首轮状态前仍等待');
  const failure = '模型代理返回 404：合成默认模型不可用';
  scenario.createdStream.write('event: snapshot\ndata: ' + JSON.stringify({
    ...scenario.createdSnapshot, firstTurnPending: false,
    capabilities: { ...scenario.createdSnapshot.capabilities, send: true },
    thread: { ...scenario.createdSnapshot.thread, status: 'idle', model: 'fixture-unavailable-model' },
    turns: [{ id: 'fixture-first-turn', status: 'failed', startedAt: 1_795_622_400,
      error: { message: failure }, items: [] }],
  }) + '\n\n');
  await page.locator('.turn-failure').getByText(failure, { exact: true }).waitFor();
  await page.waitForFunction(() => document.querySelector('[aria-label="发送消息"]')?.disabled === false);
  assert.equal(await input.innerText(), '核对首轮期间保留的新草稿', 'failed确认只释放首轮等待，不清当前聊天的新草稿');
  assert.equal(await options.getAttribute('title'), 'gpt-6.1-sol');
  assert.match(await options.innerText(), /High/, '首轮失败不清下一次的显式模型选择');
  assert.equal(await page.getByText('正在核对新聊天首轮，请稍候。', { exact: true }).count(), 0);
  assert.equal(await page.getByText('已找到创建的聊天，正在核对官方首轮状态；草稿已保留。', { exact: true }).count(), 0, '首轮failed已确认后不能残留核对中的提示');
  await page.screenshot({ path: path.join(output, `${scenario.name}-failure-confirmed-${scenario.width}.png`), animations: 'disabled' });
  await input.fill('');
  assert.equal(await page.getByRole('button', { name: '发送消息', exact: true }).isDisabled(), true, '空输入不能伪装继续失败轮次');
  assert.equal(await page.getByRole('button', { name: '继续任务', exact: true }).count(), 0);
  await input.press('Control+Enter');
  assert.equal(calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.path.endsWith('/messages')).length, 0);
  await input.fill('使用显式模型恢复当前聊天');
  const received = page.waitForRequest(request => request.method() === 'POST'
    && new URL(request.url()).pathname === '/api/threads/fixture-created/messages');
  await page.getByRole('button', { name: '发送消息', exact: true }).click();
  const submitted = (await received).postDataJSON();
  assert.equal(submitted.prompt, '使用显式模型恢复当前聊天');
  assert.equal(submitted.model, 'gpt-6.1-sol');
  assert.equal(submitted.thinking, 'high');
  assert.equal(submitted.mode, 'now');
  await input.fill('发送期间继续编辑的新草稿');
  assert.equal(await page.getByRole('button', { name: '发送中', exact: true }).isDisabled(), true);
  scenario.finishMessage();
  await page.waitForFunction(() => document.querySelector('[aria-label="发送消息"]')?.disabled === false);
  assert.equal(await input.innerText(), '发送期间继续编辑的新草稿', '迟到接受回执不清后续编辑');
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
  assert.equal(writes.filter(call => call.path === '/api/threads').length, 1, '首轮failed不得再次create');
  const messages = writes.filter(call => call.path.endsWith('/messages'));
  assert.equal(messages.length, 1);
  assert.equal(messages[0].path, '/api/threads/fixture-created/messages');
  assert.equal(messages[0].contract, '2');
  assert.ok(!writes.some(call => /\/(resume|settings)$/.test(call.path)), '新消息不触发异常续接或写全局模型配置');
}

async function verifyContextReferences(page) {
  if (scenario.referenceCreation) {
    await page.goto(origin);
    await page.getByRole('button', { name: '新聊天', exact: true }).click();
    await page.locator('dialog[open]').getByRole('button', { name: source.title }).click();
    await page.locator('dialog[open]').getByRole('button', { name: project.label }).click();
    await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  } else await page.goto(origin + '/?thread=' + source.id);
  const input = page.getByRole('textbox', { name: '消息内容' });
  const body = '核对引用与原草稿。\n\n[普通文档](https://example.test/manual)\n保留换行。';
  await input.fill(body);
  const target = scenario.referenceCreation ? 'new:' + project.projectId : source.id;
  const expectedMentions = [];
  const editorBefore = await input.evaluate(element => ({
    html: element.innerHTML,
    lines: [...element.childNodes].map(node => node.textContent || ''),
    blankLines: [...element.children].filter(node => !node.textContent).map(node => ({
      height: node.getBoundingClientRect().height, lineHeight: Number.parseFloat(getComputedStyle(node).lineHeight),
    })),
  }));
  assert.deepEqual(editorBefore.lines, body.split('\n'), '真实编辑器保留正文行及其中的空行');
  assert.equal(editorBefore.blankLines.length, 1);
  assert.ok(editorBefore.blankLines.every(line => Math.abs(line.height - line.lineHeight) < 1), '空行只占一个可见行高');
  async function verifyDraft() {
    const expected = expectedMentions.length ? expectedMentions.join('\n') + '\n\n' + body : body;
    assert.equal(await page.evaluate(key => JSON.parse(sessionStorage.getItem('codex-mobile-drafts'))[key], target), expected,
      '添加或移除引用不改真实草稿正文、换行和普通Markdown链接');
    assert.equal(await input.innerHTML(), editorBefore.html, '引用操作不修改正文的可见DOM');
  }
  await verifyDraft();
  await page.getByLabel('选择发送的图片').setInputFiles({ name: 'synthetic.png', mimeType: 'image/png', buffer: syntheticImage });
  await page.locator('.composer-attachments img').waitFor();
  await page.waitForFunction(() => !document.querySelector('.composer-attachments .image-state'));
  const model = page.getByRole('button', { name: '模型与思考强度', exact: true });
  await model.click();
  await page.locator('#chat-model').selectOption('gpt-6.1-sol');
  await page.locator('#chat-effort').selectOption('high');
  await page.getByRole('button', { name: '用于下次发送', exact: true }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });

  async function selectReference(query, label) {
    await page.getByRole('button', { name: '更多操作', exact: true }).click();
    const entry = page.getByRole('menuitem', { name: '添加上下文', exact: true });
    assert.equal(await entry.isDisabled(), false, 'catalog=false时普通文件聊天引用仍可单独开放');
    await entry.click();
    const sheet = page.getByRole('dialog', { name: '添加上下文', exact: true });
    await sheet.getByRole('textbox', { name: /搜索.*文件.*聊天/ }).fill(query);
    if (scenario.contextFailure) await sheet.getByRole('alert').filter({ hasText: '隔离文件查询暂时失败' }).waitFor();
    else await sheet.getByText('文件和聊天会作为文字引用加入消息，由当前聊天读取。', { exact: true }).waitFor();
    assert.equal(await sheet.getByRole('heading', { name: '插件', exact: true }).count(), 0);
    assert.equal(await sheet.getByRole('heading', { name: '标签页', exact: true }).count(), 0);
    await sheet.locator('.context-result').filter({ hasText: label }).click();
    await sheet.waitFor({ state: 'hidden' });
    const mention = label === referencedFile.label ? referencedFile.mention : '[参考 \\[聊天\\]](codex://threads/fixture-reference-chat)';
    if (!expectedMentions.includes(mention)) expectedMentions.push(mention);
    await verifyDraft();
  }

  await selectReference('参考', referencedChat.title);
  scenario.contextFailure = false;
  await selectReference('review', referencedFile.label);
  await selectReference('review', referencedFile.label);
  assert.equal(await page.locator('.composer-context').count(), 2, '重复选择同一引用不重复插入');
  await page.getByRole('button', { name: '移除 ' + referencedFile.label, exact: true }).click();
  expectedMentions.splice(expectedMentions.indexOf(referencedFile.mention), 1);
  assert.equal(await page.locator('.composer-context').count(), 1);
  await verifyDraft();
  assert.equal(await page.locator('.composer-attachments img').count(), 1);
  await selectReference('review', referencedFile.label);
  assert.equal(await model.getAttribute('title'), 'gpt-6.1-sol');
  assert.match(await model.innerText(), /High/);
  const expectedPrompt = '[参考 \\[聊天\\]](codex://threads/fixture-reference-chat)\n' + referencedFile.mention + '\n\n' + body;
  assert.equal(await page.evaluate(key => JSON.parse(sessionStorage.getItem('codex-mobile-drafts'))[key], target), expectedPrompt,
    '文字引用沿用原草稿格式与持久化入口');
  const contextCalls = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.path === '/api/context');
  assert.ok(contextCalls.length >= 4);
  for (const call of contextCalls) {
    const params = new URLSearchParams(call.query);
    if (scenario.referenceCreation) {
      assert.equal(params.get('projectId'), project.projectId);
      assert.equal(params.get('sourceThreadId'), source.id);
      assert.equal(params.has('threadId'), false);
    } else {
      assert.equal(params.get('threadId'), source.id);
      assert.equal(params.has('projectId'), false);
      assert.equal(params.has('sourceThreadId'), false);
    }
  }
  assert.equal(calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST'
    && (call.path === '/api/threads' || call.path.endsWith('/messages'))).length, 0, '添加引用只编辑草稿，不自动投递');
  await page.screenshot({ path: path.join(output, `${scenario.name}-draft-${scenario.width}.png`), animations: 'disabled' });
  await page.getByRole('button', { name: scenario.referenceCreation ? '创建对话并发送' : '发送消息', exact: true }).click();
  if (scenario.referenceCreation) await page.getByRole('heading', { name: '已创建隔离聊天', exact: true }).waitFor();
  await page.waitForFunction(() => document.querySelector('[aria-label="消息内容"]').textContent === '');
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST'
    && (call.path === '/api/threads' || call.path.endsWith('/messages')));
  assert.equal(writes.length, 1);
  assert.equal(writes[0].path, scenario.referenceCreation ? '/api/threads' : '/api/threads/' + source.id + '/messages');
  assert.equal(writes[0].input.prompt, expectedPrompt);
  assert.deepEqual(writes[0].input.images, [imageId], '文件聊天引用仅存在于正文，不冒充图片附件');
  assert.equal(writes[0].input.model, 'gpt-6.1-sol');
  assert.equal(writes[0].input.thinking, 'high');
  assert.equal(writes[0].contract, '2');
  if (scenario.referenceCreation) {
    assert.equal(writes[0].input.sourceThreadId, source.id);
    assert.equal(writes[0].input.projectId, project.projectId);
  }
  assert.equal(await page.locator('.composer-context').count(), 0);
  await page.locator('.composer-attachments').waitFor({ state: 'hidden' });
}

async function verifyWebUpgrade(page) {
  let navigations = 0;
  page.on('request', request => { if (request.isNavigationRequest() && request.frame() === page.mainFrame()) navigations++; });
  await page.goto(origin + '/?thread=' + source.id);
  const input = page.getByRole('textbox', { name: '消息内容' });
  await input.waitFor();
  await page.getByText('选择测试项', { exact: true }).waitFor();
  await page.waitForFunction(() => !!navigator.serviceWorker.controller);
  const workerRegistration = await page.evaluateHandle(() => navigator.serviceWorker.getRegistration());
  const options = page.getByRole('button', { name: '模型与思考强度', exact: true });
  await options.click();
  await page.locator('#chat-model').selectOption('gpt-6.1-sol');
  await page.locator('#chat-effort').selectOption('high');
  await page.getByRole('button', { name: '用于下次发送', exact: true }).click();
  await page.locator('dialog[open]').waitFor({ state: 'hidden' });
  await page.getByLabel('选择发送的图片').setInputFiles({ name: 'synthetic.png', mimeType: 'image/png', buffer: syntheticImage });
  await page.locator('.composer-attachments img').waitFor();
  await page.waitForFunction(() => !document.querySelector('.composer-attachments .image-state'));
  await input.fill('网页升级必须保留的新草稿');
  const pendingBefore = await page.evaluate(() => localStorage.getItem('codex-mobile-pending-v2'));
  const draftsBefore = await page.evaluate(() => ({
    text: sessionStorage.getItem('codex-mobile-drafts'),
    model: sessionStorage.getItem('codex-mobile-model-drafts'),
    images: localStorage.getItem('codex-mobile-image-drafts'),
  }));
  assert.equal(JSON.parse(draftsBefore.text)[source.id], '网页升级必须保留的新草稿');
  assert.deepEqual(JSON.parse(draftsBefore.model)[source.id], { model: 'gpt-6.1-sol', thinking: 'high' });
  assert.deepEqual(JSON.parse(draftsBefore.images)[source.id], [{
    id: imageId, name: 'synthetic.png', url: '/api/images/' + imageId, size: syntheticImage.length, type: 'image/png',
  }]);
  const postsBefore = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
  assert.deepEqual(postsBefore.map(call => call.path), ['/api/images'], '更新前只有用户选择图片的上传请求');
  const initialLoads = navigations;
  scenario.webVersion = 'synthetic-upgraded-build';
  scenario.holdUpgradeAsset = true;
  await Promise.all([
    page.waitForResponse(response => new URL(response.url()).pathname === '/version.json'),
    page.evaluate(() => window.dispatchEvent(new Event('online'))),
  ]);
  assert.equal(navigations, initialLoads, '输入聚焦时不可自动刷新');
  await page.waitForFunction(registration => registration.installing?.state === 'installing', workerRegistration);
  await input.blur();
  // 服务端刻意阻塞一个真实预缓存请求；失焦也不能绕过安装阶段。
  const deadline = Date.now() + 10000;
  while (!scenario.releaseUpgradeAsset && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.ok(scenario.releaseUpgradeAsset, '新 Worker 应当请求新构建资源');
  await page.waitForTimeout(100);
  assert.equal(navigations, initialLoads, '资源安装未完成时不刷新');
  scenario.releaseUpgradeAsset(503);
  await page.waitForFunction(registration => !registration.installing, workerRegistration);
  await input.focus();
  await input.blur();
  await page.waitForTimeout(100);
  assert.equal(navigations, initialLoads, '资源安装失败后失焦不能刷新');
  assert.equal(await input.innerText(), '网页升级必须保留的新草稿');
  assert.equal(await page.evaluate(() => localStorage.getItem('codex-mobile-pending-v2')), pendingBefore);
  assert.equal(await page.evaluate(() => caches.has('codex-mobile-shell-v2-synthetic-upgraded-build')), false, '失败候选缓存应回收');
  scenario.holdUpgradeAsset = false;
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await page.waitForFunction(() => document.querySelector('meta[name="app-version"]').content === 'synthetic-upgraded-build');
  await input.waitFor();
  await page.getByText('选择测试项', { exact: true }).waitFor();
  assert.equal(await input.innerText(), '网页升级必须保留的新草稿');
  assert.equal(await options.getAttribute('title'), 'gpt-6.1-sol');
  assert.match(await options.innerText(), /High/);
  assert.equal(await page.locator('.composer-attachments img').count(), 1);
  assert.match(await page.locator('.composer-attachments img').getAttribute('src'), new RegExp(imageId.replace('.', '\\.')));
  assert.equal(await page.evaluate(() => localStorage.getItem('codex-mobile-pending-v2')), pendingBefore);
  assert.deepEqual(await page.evaluate(() => ({
    text: sessionStorage.getItem('codex-mobile-drafts'),
    model: sessionStorage.getItem('codex-mobile-model-drafts'),
    images: localStorage.getItem('codex-mobile-image-drafts'),
  })), draftsBefore, '同一次安全刷新后文字、模型强度和图片引用草稿逐字保留');
  assert.equal(await page.getByRole('button', { name: '发送消息', exact: true }).isDisabled(), true, '升级保留真实 unknown 的发送保护');
  assert.equal(navigations, initialLoads + 1, '只读卡片不阻塞安全刷新，且只刷新一次');
  const writes = calls.filter(call => call.scenario === scenario.name && call.viewport === scenario.width && call.method === 'POST');
  assert.deepEqual(writes.map(call => call.path), ['/api/images'], '仅选择图片时上传一次；重载不能重发图片或消息');
  assert.deepEqual(writes, postsBefore, '检查版本与刷新过程中不能增加任何POST');
  const viewport = page.viewportSize();
  const send = await page.getByRole('button', { name: '发送消息', exact: true }).boundingBox();
  assert.ok(send.y >= 0 && send.y + send.height <= viewport.height, '待确认提示、只读问题和图片并存时发送按钮完整留在可视区');
  const model = await options.boundingBox();
  assert.ok(model.y >= 0 && model.y + model.height <= viewport.height, '模型选择不能被辅助信息挤出可视区');
  const retry = page.locator('.composer-region').getByRole('button', { name: '重新核对', exact: true });
  await retry.scrollIntoViewIfNeeded();
  await retry.click({ trial: true });
  const retryBounds = await retry.boundingBox();
  assert.ok(retryBounds.y >= 0 && retryBounds.y + retryBounds.height <= viewport.height, '辅助状态仍可滚动到达并核对');
}

try {
  for (const viewport of [{ width: 390, height: 844 }, { width: 844, height: 390 }, { width: 1280, height: 900 }]) {
    for (const item of [...cases,
      { name: 'create-image-model-retry', ids: ['SEND-001', 'SEND-002', 'SEND-004', 'SEND-009', 'CONTROL-004'], creation: true },
      { name: 'upgrade-progress-keeps-draft', ids: ['UPDATE-002', 'UPDATE-003', 'UPDATE-006'], upgrade: true },
      { name: 'recovered-creation-in-chat', ids: ['OPS-002', 'SEND-002'], restoreCreation: true, terminal: 'accepted', local: true },
      { name: 'native-hooks-read-only', ids: ['CONTROL-008'], hooks: true },
      { name: 'history-hooks-read-only', ids: ['CONTROL-008'], hooks: true, historyOnly: true },
      { name: 'native-hooks-open-desktop', ids: ['CONTROL-007', 'CONTROL-008'], hooks: true, desktopNavigation: true },
      { name: 'created-first-turn-failed-explicit-model', ids: ['SEND-002', 'SEND-007', 'SEND-009', 'SEND-010', 'CONTROL-002'], failedCreation: true },
      { name: 'web-upgrade-keeps-unknown-and-readonly-question', ids: ['UPDATE-007', 'UPDATE-009', 'OPS-004', 'INTERACT-005'], webUpgrade: true, local: true, terminal: 'unknown', question: true, readonlyQuestion: true },
      { name: 'context-references-existing-chat', ids: ['CONTROL-009', 'CONTROL-010', 'SEND-003', 'SEND-004', 'SEND-009'], references: true, contextFailure: true },
      { name: 'context-references-new-chat', ids: ['CONTROL-009', 'CONTROL-010', 'SEND-002', 'SEND-004', 'SEND-009'], references: true, referenceCreation: true },
      { name: 'composer-menu-over-markdown', ids: ['UI-005'], composerMenu: true },
      { name: 'current-model-in-settings', ids: ['SEND-009', 'UI-006'], currentModel: true },
    ]) {
      scenario = { ...item, width: viewport.width, rejectCreation: !item.failedCreation && !item.referenceCreation, createdInputs: [] };
      const { context, page } = await pageFor(viewport);
      const result = { scenario: item.name, ids: [...item.ids, 'UI-001'], viewport, status: 'passed' };
      try {
        if (item.composerMenu) await verifyComposerMenu(page);
        else if (item.currentModel) await verifyCurrentModel(page);
        else if (item.webUpgrade) await verifyWebUpgrade(page);
        else if (item.references) await verifyContextReferences(page);
        else if (item.creation) await verifyCreation(page);
        else if (item.upgrade) await verifyUpgrade(page);
        else if (item.restoreCreation) await verifyRecoveredCreation(page);
        else if (item.hooks) await verifyHooks(page);
        else if (item.failedCreation) await verifyFailedFirstTurn(page);
        else await verifySubmission(page);
      } catch (error) {
        result.status = 'failed';
        result.error = error.stack || String(error);
        console.error(`${item.name} ${viewport.width}: ${result.error}`);
      } finally {
        await page.screenshot({ path: path.join(output, `${item.name}-${viewport.width}.png`) });
        results.push(result);
        await context.close();
      }
      console.log(`${result.status}: ${item.name} ${viewport.width}x${viewport.height}`);
    }
  }
} finally {
  const report = { environment: 'Docker Chromium; synthetic localhost HTTP/SSE only', results, pageErrors, calls };
  fs.writeFileSync(path.join(output, 'report.json'), JSON.stringify(report, null, 2));
  await browser.close();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
await import('./desktop-update.mjs');
await import('./service-worker-update.mjs');
await import('./reply-layout.mjs');
await import('./managed-controls.mjs');
await import('./relay-removal.mjs');
assert.deepEqual(pageErrors, [], '页面执行不能产生未处理异常');
assert.equal(results.filter(result => result.status === 'failed').length, 0, '浏览器回归存在失败，查看 report.json 与截图');
console.log(`Docker browser regression: ${results.length} passed; artifacts: ${output}`);
