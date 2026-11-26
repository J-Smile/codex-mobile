import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';

// 真实构建页面 + 合成共享协议 HTTP/SSE；不代表 App Tools 已提供这些控制入口。
const root = path.resolve('dist/mobile');
const output = path.resolve(process.argv[2] || 'test-results/browser');
const thread = { id: 'managed-chat', title: 'Mobile 合成验收', status: 'idle', model: 'fixture-model' };
const project = { projectId: 'managed-project', label: '合成项目', path: '/fixture/project' };
const capabilities = { native: false, readLive: true, send: true,
  create: true, images: true, modelSelection: true, contextReferences: true, interactions: true,
  interrupt: true, resume: true, resumeInterrupted: true, queue: true, queueEdit: true, queueReorder: true, queueStart: true };
const image = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jf8kAAAAASUVORK5CYII=', 'base64');
const imageId = 'c'.repeat(32) + '.png';
let state;
const results = [], errors = [];
function snapshot(id = thread.id) {
  return { thread: { ...thread, id, title: id === thread.id ? thread.title : 'Mobile 新建验收' }, turns: state.turns,
    interactions: state.interactions, queue: state.queue, nextCursor: null, connection: 'live', firstTurnPending: false, capabilities };
}
function publish() {
  for (const [stream, id] of state.streams) stream.write('event: snapshot\ndata: ' + JSON.stringify(snapshot(id)) + '\n\n');
}
const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://127.0.0.1');
  const json = (value, status = 200) => { response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' }); response.end(JSON.stringify(value)); };
  if (url.pathname.startsWith('/api/')) {
    const chunks = []; for await (const chunk of request) chunks.push(chunk);
    const bytes = Buffer.concat(chunks);
    const input = request.headers['content-type']?.includes('application/json') && bytes.length ? JSON.parse(bytes) : null;
    state.calls.push({ method: request.method, path: url.pathname, query: url.search, input });
    if (url.pathname === '/api/session') return json({ capabilities });
    if (url.pathname === '/api/projects') return json({ projects: [project], capabilities: { native: false, create: true, images: true } });
    if (url.pathname === '/api/operations') return json({ operations: [], results: [] });
    if (url.pathname === '/api/update-settings') return json({ configured: false, version: '0.3.29' });
    if (url.pathname === '/api/threads') {
      if (request.method === 'GET') return json({ threads: [thread], nextCursor: null });
      return json({ created: true, threadId: 'managed-created', firstTurnAccepted: true, firstTurnPending: false });
    }
    if (url.pathname === '/api/model-options') return json({ recentModels: ['fixture-model'], efforts: ['low', 'high'] });
    if (url.pathname === '/api/images') return json({ id: imageId, name: 'synthetic.png', type: 'image/png', size: image.length, url: '/api/images/' + imageId });
    if (url.pathname.startsWith('/api/images/')) { response.setHeader('content-type', 'image/png'); return response.end(image); }
    if (url.pathname.endsWith('/events')) {
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
      const id = url.pathname.split('/')[3]; state.streams.set(response, id);
      response.write('event: snapshot\ndata: ' + JSON.stringify(snapshot(id)) + '\n\n');
      response.on('close', () => state.streams.delete(response)); return;
    }
    if (url.pathname.endsWith('/history')) return json(snapshot());
    if (url.pathname.includes('/interactions/')) {
      state.interactions = []; publish(); return json({ ok: true, resolved: true });
    }
    if (url.pathname.endsWith('/interrupt')) {
      assert.equal(input.turnId, state.turns.at(-1).id);
      state.turns.at(-1).status = 'interrupted'; publish(); return json({ ok: true });
    }
    if (url.pathname.endsWith('/resume')) {
      assert.equal(input.turnId, state.turns.at(-1).id);
      state.turns.push({ id: 'continued-turn', status: 'inProgress', items: [] }); publish(); return json({ turn: state.turns.at(-1) });
    }
    if (url.pathname.endsWith('/messages')) {
      if (input.mode === 'queue') state.queue = { revision: 'queued', messages: [{ id: 'queued-image', prompt: input.prompt, status: 'queued', attachmentCount: input.images.length }] };
      else state.turns.push({ id: 'sent-turn', status: 'inProgress', items: [] });
      publish(); return json({ accepted: true, queued: input.mode === 'queue' });
    }
    if (url.pathname.endsWith('/queue')) {
      if (request.method === 'GET') return json(state.queue);
      if (input.revision !== state.queue.revision) return json({ message: '队列已变化' }, 409);
      const rows = state.queue.messages;
      if (input.action === 'edit') rows.find(row => row.id === input.messageId).prompt = input.prompt;
      else if (input.action === 'reorder') state.queue.messages = input.messageIds.map(id => rows.find(row => row.id === id));
      else state.queue.messages = rows.filter(row => row.id !== input.messageId);
      state.queue.revision += '-next'; publish(); return json(state.queue);
    }
    return json({ message: '合成接口不支持' }, 404);
  }
  const relative = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
  const file = path.resolve(root, relative);
  if (!file.startsWith(root + path.sep)) { response.writeHead(403); return response.end(); }
  try { response.setHeader('content-type', ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.json': 'application/json' })[path.extname(file)] || 'application/octet-stream'); response.end(fs.readFileSync(file)); }
  catch { response.writeHead(404); response.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = 'http://127.0.0.1:' + server.address().port;
const browser = await chromium.launch({ headless: true });
try {
  for (const viewport of [{ width: 390, height: 844 }, { width: 844, height: 390 }, { width: 1280, height: 900 }]) {
    state = { calls: [], streams: new Map(), turns: [], interactions: [], queue: { revision: 'empty', messages: [] } };
    const context = await browser.newContext({ viewport, hasTouch: true, isMobile: viewport.width < 900, serviceWorkers: 'block', reducedMotion: 'reduce' });
    await context.route('**/*', route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
    const page = await context.newPage(); page.setDefaultTimeout(12000);
    page.on('pageerror', error => errors.push({ viewport, message: error.message }));
    const result = { viewport, ids: ['MANAGED-003', 'MANAGED-004', 'MANAGED-005', 'MANAGED-006'], status: 'passed' };
    try {
      await page.goto(origin + '/?thread=' + thread.id);
      await page.getByRole('heading', { name: thread.title, exact: true }).waitFor();
      assert.equal(await page.locator('.chat-header .runtime-label').count(), 0);
      const composer = page.getByRole('textbox', { name: '消息内容', exact: true });
      await composer.fill('问题出现前的草稿');
      state.interactions = [{ id: 'question-generation-one', turnId: 'question-turn', kind: 'questions', title: '合成问题', message: '', replySupported: true, questions: [{ id: 'answer', header: '', question: '输入合成答案', options: [] }] }]; publish();
      await page.getByRole('textbox', { name: '你的回复', exact: true }).fill('合成回答');
      await page.locator('.interaction-submit').click();
      await page.locator('.pending-interactions').waitFor({ state: 'hidden' });
      assert.equal(await composer.innerText(), '问题出现前的草稿');
      state.interactions = [{ id: 'approval-generation-one', turnId: 'question-turn', kind: 'approval', title: '允许合成操作？', message: '没有真实文件操作', approveLabel: '允许本次', replySupported: true }]; publish();
      await page.getByRole('button', { name: '拒绝', exact: true }).click();
      await page.locator('.pending-interactions').waitFor({ state: 'hidden' });
      await composer.fill('');
      state.turns = [{ id: 'active-turn', status: 'inProgress', items: [] }]; publish();
      await page.getByRole('button', { name: '停止任务', exact: true }).click();
      await page.getByRole('button', { name: '继续任务', exact: true }).click();
      await page.getByLabel('选择发送的图片').setInputFiles({ name: 'synthetic.png', mimeType: 'image/png', buffer: image });
      await composer.fill('图片排队');
      await page.getByRole('button', { name: '加入队列', exact: true }).click();
      await page.getByRole('button', { name: /待执行/ }).click();
      await page.getByRole('button', { name: '编辑第 1 条消息', exact: true }).click();
      await page.getByRole('textbox', { name: '待执行内容', exact: true }).fill('修改队列文字');
      await page.getByRole('button', { name: '保存', exact: true }).click();
      await page.locator('dialog[open]').waitFor({ state: 'hidden' });
      await page.getByText('修改队列文字', { exact: true }).waitFor();
      assert.equal(state.queue.messages[0].attachmentCount, 1);
      state.queue.messages.push({ id: 'second-row', prompt: '第二条', status: 'queued' }); state.queue.revision += '-new'; publish();
      await page.getByRole('button', { name: '上移第 2 条消息', exact: true }).click();
      await page.locator('.queue-row').first().getByText('第二条', { exact: true }).waitFor();
      assert.equal(state.queue.messages[0].id, 'second-row');
      await page.getByRole('button', { name: '停止任务', exact: true }).click();
      await page.getByRole('button', { name: '执行第 1 条消息', exact: true }).click();
      await page.getByRole('button', { name: '删除第 1 条消息', exact: true }).click();
      await page.locator('.message-queue').waitFor({ state: 'hidden' });
      await page.getByRole('button', { name: '新建聊天', exact: true }).click();
      await page.locator('dialog[open]').getByRole('button', { name: project.label, exact: true }).click();
      await page.locator('dialog[open]').waitFor({ state: 'hidden' });
      await composer.fill('官方新建验收');
      await page.getByRole('button', { name: '创建对话并发送', exact: true }).click();
      await page.getByRole('heading', { name: 'Mobile 新建验收', exact: true }).waitFor();
      const created = state.calls.filter(call => call.method === 'POST' && call.path === '/api/threads');
      assert.equal(created.length, 1); assert.equal(Object.hasOwn(created[0].input, 'execution'), false);
      assert.equal(state.calls.filter(call => call.path.endsWith('/interrupt')).length, 2);
      assert.equal(state.calls.filter(call => call.path.endsWith('/resume')).length, 1);
      const writes = state.calls.filter(call => call.path.includes('/interactions/'));
      assert.deepEqual(writes.map(call => call.input), [{ answers: { answer: ['合成回答'] } }, { decision: 'decline' }]);
      assert.equal(await composer.innerText(), '');
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    } catch (error) { result.status = 'failed'; result.error = error.stack || String(error); console.error(result.error); }
    finally { await page.screenshot({ path: path.join(output, `managed-controls-${viewport.width}.png`) }); result.calls = state.calls; results.push(result); await context.close(); }
  }
} finally {
  fs.writeFileSync(path.join(output, 'managed-controls.json'), JSON.stringify({ environment: 'Docker Chromium with synthetic HTTP/SSE', results, errors }, null, 2));
  await browser.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
}
assert.deepEqual(errors, []); assert.equal(results.filter(result => result.status === 'failed').length, 0);
console.log(`Mobile control component regression: ${results.length} passed`);
