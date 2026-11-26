import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';

// 只验收已有问题表单的布局和草稿；合成回复成功不代表官方控制入口已开放。
const root = path.resolve('dist/mobile');
const output = path.resolve(process.argv[2] || 'test-results/browser');
fs.mkdirSync(output, { recursive: true });
const thread = { id: 'reply-layout-chat', title: '隔离问题布局聊天', status: 'idle' };
const project = { projectId: 'reply-layout-project', label: '隔离问题布局项目' };
const capabilities = { readLive: true, send: true, interactions: true, native: false, create: false,
  images: false, queue: false, catalog: false, settings: false, goals: false, interrupt: false };
const interaction = { id: 'reply-layout-request', turnId: 'reply-layout-turn', kind: 'questions',
  title: '隔离问题', message: '', replySupported: true, skipSupported: false, questions: [{
    id: 'choice', header: '回归问题', question: '请选择一个合成选项，或填写自己的答案以验证长内容下的布局。', secret: false,
    options: Array.from({ length: 10 }, (_, index) => ({ label: `合成选项 ${index + 1}`,
      description: '用于验证问题正文可滚动，同时保持回复输入和操作按钮可达。' })),
  }] };
const results = [], errors = [], calls = [];
let current;
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
    const input = bytes.length ? JSON.parse(bytes) : null;
    calls.push({ scenario: current.scenario, viewport: current.viewport, method: request.method, path: url.pathname, search: url.search, input,
      contract: request.headers['x-codex-mobile-contract'] });
    if (url.pathname === '/api/session') return json({ capabilities: current.capabilities });
    if (url.pathname === '/api/operations') return json({ operations: [], results: [] });
    if (url.pathname === '/api/projects') return json({ projects: current.capabilities.create ? [project] : [], capabilities: current.capabilities });
    if (url.pathname === '/api/threads') return json({ threads: [thread], nextCursor: null });
    if (url.pathname === '/api/update-settings') return json({ configured: false, source: null, version: '0.3.28', mode: 'automatic' });
    if (url.pathname.endsWith('/events')) {
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
      current.stream = response;
      current.snapshot = { thread, turns: [], nextCursor: null, connection: 'live', capabilities: current.capabilities,
        firstTurnPending: false, interactions: [interaction] };
      response.write('event: snapshot\ndata: ' + JSON.stringify(current.snapshot) + '\n\n');
      return;
    }
    if (url.pathname.endsWith('/history')) return json({ thread, turns: [], nextCursor: null });
    if (url.pathname === `/api/threads/${thread.id}/interactions/${interaction.id}` && request.method === 'POST') {
      current.submissions++;
      if (current.submissions === 1) return json({ message: '合成回复失败，请保留答案后重试。' }, 503);
      json({ accepted: true });
      current.stream.write('event: snapshot\ndata: ' + JSON.stringify({ ...current.snapshot, interactions: [] }) + '\n\n');
      return;
    }
    return json({});
  }
  const file = path.resolve(root, '.' + (url.pathname === '/' ? '/index.html' : url.pathname));
  if (!file.startsWith(root + path.sep)) { response.writeHead(403); return response.end(); }
  try {
    response.setHeader('content-type', ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css',
      '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png' })[path.extname(file)] || 'application/octet-stream');
    response.end(fs.readFileSync(file));
  } catch { response.writeHead(404); response.end(); }
});

async function visibleBounds(locator, viewport, label, touch = false) {
  if (touch) await locator.click({ trial: true });
  const box = await locator.boundingBox();
  assert.ok(box && box.width > 0 && box.height > 0, `${label}必须可见`);
  assert.ok(box.x >= -1 && box.y >= -1 && box.x + box.width <= viewport.width + 1
    && box.y + box.height <= viewport.height + 1, `${label}必须完整位于视口：${JSON.stringify(box)}`);
  const clipped = await locator.evaluate(element => {
    const rect = element.getBoundingClientRect();
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent), bounds = parent.getBoundingClientRect();
      if (/(auto|scroll|hidden|clip)/.test(style.overflowY) && (rect.top < bounds.top - 1 || rect.bottom > bounds.bottom + 1)) return parent.className;
    }
    return '';
  });
  assert.equal(clipped, '', `${label}不能被祖先容器裁剪`);
  if (touch) {
    assert.ok(box.width >= 44 && box.height >= 44, `${label}触控范围至少44px`);
  }
}

async function waitDock(page, inert) {
  await page.waitForFunction(expected => {
    const dock = document.querySelector('.composer-dock');
    return dock?.inert === expected && Number(getComputedStyle(dock).opacity) === (expected ? 0 : 1);
  }, inert);
}

const browser = await chromium.launch({ headless: true });
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = 'http://127.0.0.1:' + server.address().port;
try {
  for (const viewport of [{ width: 390, height: 844 }, { width: 844, height: 390 }, { width: 1280, height: 900 }]) {
   for (const scenario of ['reply-layout-draft-and-short-viewport', 'reply-layout-new-chat-clears-view']) {
    const shortened = { width: viewport.width, height: Math.min(420, viewport.height - 140) };
    const newChat = scenario === 'reply-layout-new-chat-clears-view';
    current = { scenario, viewport, submissions: 0, capabilities: { ...capabilities, create: newChat } };
    const context = await browser.newContext({ viewport, hasTouch: true, isMobile: viewport.width < 900,
      serviceWorkers: 'block', reducedMotion: 'reduce' });
    await context.route('**/*', route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
    await context.addInitScript(({ id, projectId }) => sessionStorage.setItem('codex-mobile-drafts', JSON.stringify({
      [id]: '问题出现前的普通消息草稿', ['new:' + projectId]: '原先保留的新建草稿',
    })), { id: thread.id, projectId: project.projectId });
    const page = await context.newPage();
    page.setDefaultTimeout(10_000);
    page.on('pageerror', error => errors.push({ scenario, viewport, message: error.message }));
    const result = { scenario, ids: ['UI-001', 'INTERACT-001', 'INTERACT-004', 'INTERACT-005', ...(newChat ? ['SEND-001'] : [])],
      viewport, shortened, status: 'passed' };
    try {
      await page.goto(origin + '/?thread=' + thread.id);
      const dock = page.locator('.composer-dock');
      const close = page.getByRole('button', { name: '收起待回复的问题', exact: true });
      const reopen = page.getByRole('button', { name: '有问题待回复', exact: true });
      const answer = page.getByRole('textbox', { name: '自行撰写回复', exact: true });
      const submit = page.locator('.interaction-submit');
      const footer = page.locator('.question-footer');
      await waitDock(page, true);
      await answer.fill('收起和键盘变化后仍保留这份合成答案');
      if (newChat) {
        await page.getByRole('button', { name: '新建聊天', exact: true }).click();
        await page.locator('dialog[open]').getByRole('button', { name: project.label, exact: true }).click();
        await page.locator('dialog[open]').waitFor({ state: 'hidden' });
        const create = page.getByRole('button', { name: '创建对话并发送', exact: true });
        await create.waitFor();
        await waitDock(page, false);
        assert.equal(await page.locator('.composer-region.question-mode').count(), 0, '旧问题卸载后不能把新建页留在回复模式');
        assert.equal(await page.locator('.pending-interactions').count(), 0, '新建页不复制旧问题面板');
        const composer = page.getByRole('textbox', { name: '消息内容', exact: true });
        assert.equal(await composer.innerText(), '原先保留的新建草稿');
        await composer.fill('问题之后继续编辑的新建草稿');
        await visibleBounds(create, viewport, '问题页切到新建的发送按钮', true);
        const drafts = await page.evaluate(() => JSON.parse(sessionStorage.getItem('codex-mobile-drafts')));
        assert.equal(drafts[thread.id], '问题出现前的普通消息草稿', '结束聊天视图不清原普通草稿');
        assert.equal(drafts['new:' + project.projectId], '问题之后继续编辑的新建草稿');
        const activity = calls.filter(call => call.scenario === scenario && call.viewport === viewport);
        assert.ok(activity.some(call => call.path === '/api/projects' && call.search === '?threadId=' + thread.id), '来源使用原正式聊天');
        assert.equal(activity.filter(call => call.method === 'POST').length, 0, '新建草稿与视图切换不自动创建或回复');
        continue;
      }
      await visibleBounds(footer, viewport, '问题回复栏');
      await visibleBounds(close, viewport, '收起问题按钮', true);
      await visibleBounds(submit, viewport, '回复发送按钮', true);
      assert.ok(await page.locator('.question-scroll').evaluate(element => element.scrollHeight > element.clientHeight), '长问题内容使用原滚动区');
      await page.screenshot({ path: path.join(output, `reply-layout-expanded-${viewport.width}.png`) });

      await close.click();
      await waitDock(page, false);
      const composer = page.getByRole('textbox', { name: '消息内容', exact: true });
      assert.equal(await composer.innerText(), '问题出现前的普通消息草稿');
      await composer.fill('收起问题时编辑的普通消息草稿');
      await visibleBounds(reopen, viewport, '重新展开问题按钮', true);
      await reopen.click();
      await waitDock(page, true);
      assert.equal(await answer.inputValue(), '收起和键盘变化后仍保留这份合成答案');

      await answer.focus();
      await page.setViewportSize(shortened);
      await page.locator('.composer-region.keyboard-open').waitFor();
      await visibleBounds(footer, shortened, '缩短视口的问题回复栏');
      await visibleBounds(close, shortened, '缩短视口的收起按钮', true);
      await visibleBounds(submit, shortened, '缩短视口的发送按钮', true);
      await page.screenshot({ path: path.join(output, `reply-layout-short-${viewport.width}.png`) });
      await close.click();
      await waitDock(page, false);
      await page.waitForFunction(() => !document.querySelector('.composer-region.keyboard-open'));
      assert.equal(await composer.innerText(), '收起问题时编辑的普通消息草稿');
      await visibleBounds(reopen, shortened, '缩短视口的展开按钮', true);
      await visibleBounds(page.getByRole('button', { name: '发送消息', exact: true }), shortened, '缩短视口的普通发送按钮', true);
      await page.screenshot({ path: path.join(output, `reply-layout-minimized-short-${viewport.width}.png`) });
      await reopen.click();
      await waitDock(page, true);
      assert.equal(await answer.inputValue(), '收起和键盘变化后仍保留这份合成答案');
      await page.setViewportSize(viewport);
      await visibleBounds(footer, viewport, '恢复视口的问题回复栏');

      await submit.click();
      await page.getByText('合成回复失败，请保留答案后重试。', { exact: true }).waitFor();
      assert.equal(await answer.inputValue(), '收起和键盘变化后仍保留这份合成答案');
      assert.equal(await dock.evaluate(element => element.inert), true, '失败后继续保留回复模式');
      await visibleBounds(submit, viewport, '失败后的重试发送按钮', true);
      await submit.click();
      await waitDock(page, false);
      await page.locator('.pending-interactions.has-pending').waitFor({ state: 'hidden' });
      assert.equal(await composer.innerText(), '收起问题时编辑的普通消息草稿');
      const writes = calls.filter(call => call.scenario === scenario && call.viewport === viewport && call.method === 'POST');
      assert.equal(writes.length, 2, '仅点击回复两次；收起、展开、缩短视口不提交消息');
      for (const call of writes) {
        assert.equal(call.path, `/api/threads/${thread.id}/interactions/${interaction.id}`);
        assert.equal(call.contract, '2');
        assert.deepEqual(call.input, { answers: { choice: ['收起和键盘变化后仍保留这份合成答案'] } });
      }
    } catch (error) {
      result.status = 'failed'; result.error = error.stack || String(error);
      result.layout = await page.locator('.interaction-reopen, .pending-interaction, .pending-interactions, .composer-support, .composer-region, .composer-dock').evaluateAll(elements => elements.map(element => ({
        className: element.className, rect: element.getBoundingClientRect().toJSON(),
        height: getComputedStyle(element).height, maxHeight: getComputedStyle(element).maxHeight,
        flex: getComputedStyle(element).flex, scrollHeight: element.scrollHeight, clientHeight: element.clientHeight,
      })));
      console.error(`${scenario} ${viewport.width}: ${result.error}`);
    } finally {
      await page.screenshot({ path: path.join(output, `${newChat ? scenario : 'reply-layout-result'}-${viewport.width}.png`) });
      results.push(result);
      await context.close();
      console.log(`${result.status}: ${scenario} ${viewport.width}x${viewport.height}`);
    }
   }
  }
} finally {
  fs.writeFileSync(path.join(output, 'reply-layout-report.json'), JSON.stringify({
    environment: 'Docker Chromium; synthetic HTTP/SSE and reply capability only; no official control verification', results, errors, calls,
  }, null, 2));
  await browser.close();
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
assert.deepEqual(errors, [], '问题布局回归不能产生未处理异常');
assert.equal(results.filter(result => result.status === 'failed').length, 0, '问题布局回归失败，查看reply-layout-report.json');
console.log(`Docker reply layout regression: ${results.length} passed; artifacts: ${output}`);
