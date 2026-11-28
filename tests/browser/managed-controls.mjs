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
  settings: true, goals: true, delegation: true, contextPlugins: true, contextBrowser: true,
  threadRename: true, threadPin: true, threadArchive: true, threadDelete: true,
  interrupt: true, resume: true, resumeInterrupted: true, queue: true, queueEdit: true, queueReorder: true, queueStart: true };
const image = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jf8kAAAAASUVORK5CYII=', 'base64');
const imageId = 'c'.repeat(32) + '.png';
let state;
const results = [], errors = [];
function snapshot(id = thread.id) {
  return { thread: { ...thread, ...state.metadata, goal: state.goal, mode: state.mode, id, title: state.metadata?.title || (id === thread.id ? thread.title : 'Mobile 新建验收') }, turns: state.turns,
    interactions: state.interactions, queue: state.queue, nextCursor: null, connection: 'live', firstTurnPending: !!state.creatingGoal,
    capabilities: state.creatingGoal ? {...capabilities, goals:false, send:false} : capabilities };
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
    if (url.pathname === '/api/projects') return json({ projects: [project], capabilities: { native: capabilities.native, create: true, images: true } });
    if (url.pathname === '/api/operations') return json({ operations: [], results: [] });
    if (url.pathname === '/api/update-settings') return json({ configured: false, version: '0.3.29' });
    if (url.pathname === '/api/threads') {
      if (request.method === 'GET') return json({ threads: [thread], nextCursor: null });
      if (input.prompt.includes('等我设置目标与预算后再开始')) {
        state.creatingGoal = true;
        setTimeout(() => { state.creatingGoal = false; publish(); }, 500);
        return json({created:true,threadId:'managed-created-goal',firstTurnAccepted:false});
      }
      return json({ created: true, threadId: 'managed-created', firstTurnAccepted: true, firstTurnPending: false });
    }
    if (url.pathname === '/api/goal-service') return json({ready:true});
    if (url.pathname === '/api/thread-catalog') return json({ threads: [{ ...thread, archived: url.searchParams.get('archived') === 'true' }], nextCursor: null });
    if (url.pathname === '/api/context') return json({ mode: 'context', plugins: [{id:'synthetic',kind:'plugin',label:'合成插件',description:'验收插件',mention:'[合成插件](plugin://synthetic@test)'}], browserTabs:[{id:'tab',kind:'browser',label:'合成标签页',description:'https://example.test',mention:'[合成标签页](plugin://browser@openai-bundled?mention=tab-v1&browserId=managed-chat&tabId=synthetic)'}],files:[] });
    if (url.pathname.endsWith('/settings')) {
      if (request.method === 'POST') {
        if (input.mode) state.mode = input.mode;
        if (input.permissions) state.permission = input.permissions;
        publish();
      }
      return json({applied:true,mode:state.mode || 'default',model:'fixture-model',permissions:{mode:state.permission || 'workspace-write',profile:':workspace'}});
    }
    if (url.pathname.endsWith('/goal')) {
      if (request.method === 'PATCH') state.goal = {threadId:thread.id,objective:'',status:'active',tokenBudget:null,tokensUsed:0,timeUsedSeconds:0,createdAt:1,updatedAt:2,...state.goal,...input};
      if (request.method === 'DELETE') state.goal = null;
      if (request.method !== 'GET') publish();
      return json({goal:state.goal || null,completedGoal:null});
    }
    if (url.pathname.endsWith('/manage')) {
      const action = url.searchParams.get('action');
      state.metadata = {...state.metadata,...input,...(action === 'pin' ? {pinnedIndex:input.pinned ? 1 : null} : {})};
      publish(); return json({threadId:url.pathname.split('/')[3],...input,...(action==='delete'?{deleted:true}:{})});
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
      if (capabilities.asyncReplies && !capabilities.interactions) {
        state.interactions[0].deliveryStatus = 'accepted'; state.interactions[0].replySupported = false;
        publish(); return json({ resolved: false, deliveryStatus: 'accepted' });
      }
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
      else {
        if (input.action === 'send') {
          const message = rows.find(row => row.id === input.messageId);
          const item = { id: message.id, type: 'userMessage', content: [{ type: 'text', text: message.prompt }] };
          if (state.turns.at(-1)?.status === 'inProgress') state.turns.at(-1).items.push(item);
          else state.turns.push({ id: 'queue-started-turn', status: 'inProgress', items: [item] });
        }
        state.queue.messages = rows.filter(row => row.id !== input.messageId);
      }
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
  for (const native of [false, true]) for (const viewport of [{ width: 320, height: 568 }, { width: 390, height: 844 }, { width: 844, height: 390 }, { width: 1280, height: 900 }]) {
    Object.assign(capabilities, { native, interactions: true, asyncReplies: native, interrupt: true, queue: true });
    state = { calls: [], streams: new Map(), turns: [], interactions: [], queue: { revision: 'empty', messages: [] } };
    const context = await browser.newContext({ viewport, hasTouch: true, isMobile: viewport.width < 900, serviceWorkers: 'block', reducedMotion: 'reduce' });
    await context.route('**/*', route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
    const page = await context.newPage(); page.setDefaultTimeout(12000);
    page.on('pageerror', error => errors.push({ viewport, message: error.message }));
    const result = { viewport, native, ids: ['MANAGED-003', 'MANAGED-004', 'MANAGED-005', 'MANAGED-006', ...(native ? ['CONTROL-014'] : [])], status: 'passed' };
    try {
      await page.goto(origin + '/?thread=' + thread.id);
      await page.getByRole('heading', { name: thread.title, exact: true }).waitFor();
      assert.equal(await page.locator('.chat-header .runtime-label').count(), 0);
      const composer = page.getByRole('textbox', { name: '消息内容', exact: true });
      capabilities.settings = false; capabilities.images = false; publish();
      await page.getByRole('button', { name: '更多操作', exact: true }).click();
      await page.getByText(/工作模式控制连接尚未就绪/).waitFor();
      assert.equal(await page.getByRole('menuitemradio', { name: '规划', exact: true }).isDisabled(), true);
      assert.equal(state.calls.filter(call => call.method === 'POST' && call.path.endsWith('/settings')).length, 0);
      await page.screenshot({path:path.join(output,`mode-disconnected-${native}-${viewport.width}.png`),animations:'disabled'});
      capabilities.settings = true; capabilities.images = true; publish();
      await page.waitForFunction(() => document.querySelector('[role="menuitemradio"][aria-checked="false"]')?.disabled === false);
      await page.getByRole('button', { name: '收起更多操作', exact: true }).click();
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
      const activeSend = page.getByRole('button', { name: '立即发送第 2 条消息', exact: true });
      assert.equal(await activeSend.isEnabled(), true);
      assert.equal(await activeSend.getAttribute('title'), '立即追加到当前任务');
      await page.screenshot({ path: path.join(output, `queue-active-${native ? 'native-' : ''}${viewport.width}.png`) });
      await activeSend.click();
      await page.waitForFunction(() => document.querySelectorAll('.queue-row').length === 1);
      assert.equal(state.turns.at(-1).id, 'continued-turn');
      assert.equal(state.turns.at(-1).status, 'inProgress');
      assert.equal(state.turns.at(-1).items.at(-1).id, 'queued-image');
      assert.equal(state.calls.filter(call => call.path.endsWith('/interrupt')).length, 1);
      assert.equal(state.calls.filter(call => call.path.endsWith('/queue') && call.input?.action === 'send').length, 1);
      state.queue.messages.push({ id: 'delete-row', prompt: '待删除样例', status: 'queued' }); state.queue.revision += '-new'; publish();
      await page.getByText('待删除样例', { exact: true }).waitFor();
      await page.getByRole('button', { name: '停止任务', exact: true }).click();
      const idleSend = page.getByRole('button', { name: '立即发送第 1 条消息', exact: true });
      await page.waitForFunction(() => document.querySelector('[aria-label="立即发送第 1 条消息"]')?.title === '立即执行');
      await idleSend.click();
      await page.waitForFunction(() => document.querySelectorAll('.queue-row').length === 1);
      assert.equal(state.turns.at(-1).id, 'queue-started-turn');
      assert.equal(state.turns.at(-1).items[0].id, 'second-row');
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
      await page.getByRole('button',{name:'新建聊天',exact:true}).click();
      await page.locator('dialog[open]').getByRole('button',{name:project.label,exact:true}).click();
      await page.locator('dialog[open]').waitFor({state:'hidden'});
      await composer.fill('新目标原草稿');
      await page.getByRole('button',{name:'更多操作',exact:true}).click();
      await page.getByRole('menuitemradio',{name:'目标',exact:true}).click();
      await page.getByLabel('希望完成什么',{exact:true}).fill('新聊天等待首轮后设置目标');
      await page.getByPlaceholder('不设预算',{exact:true}).fill('7000');
      await page.getByRole('button',{name:'设置目标',exact:true}).click();
      await page.locator('.goal-panel').waitFor({state:'hidden'});
      assert.equal(state.calls.filter(call=>call.method==='POST'&&call.path==='/api/threads').length,2);
      assert.ok(state.calls.some(call=>call.method==='PATCH'&&call.path==='/api/threads/managed-created-goal/goal'&&call.input.tokenBudget===7000));
      assert.equal(await composer.innerText(),'');
      await page.getByRole('button',{name:'更多操作',exact:true}).click();
      await page.getByRole('menuitemradio',{name:'目标',exact:true}).click();
      await page.getByRole('button',{name:'结束目标',exact:true}).click();
      await page.locator('.goal-panel').waitFor({state:'hidden'});
      // 原生仅开放答案投递；刷新仍保留状态，普通输入和继续任务都可操作。
      Object.assign(capabilities, { native: true, interactions: false, asyncReplies: true, interrupt: false, queue: false });
      state.turns = [];
      state.interactions = [{ id: 'native-question', source: 'async-message', turnId: 'native-question-turn', kind: 'questions', title: '问题', message: '', replySupported: true, skipSupported: false, questions: [{ id: 'native-answer', question: '原生合成问题', header: '', options: [] }] }]; publish();
      await page.getByRole('textbox', { name: '你的回复', exact: true }).fill('原生合成答案');
      await page.locator('.interaction-submit').click();
      await page.getByRole('button', { name: '答案已发送', exact: true }).waitFor();
      await composer.fill('答案送达后的普通草稿');
      await page.reload();
      await page.getByRole('button', { name: '答案已发送', exact: true }).waitFor();
      assert.equal(await composer.innerText(), '答案送达后的普通草稿');
      await page.getByRole('button', { name: '答案已发送', exact: true }).click();
      await page.getByText('答案已送达，问题状态等待同步。你可以继续发送消息。', { exact: true }).waitFor();
      await composer.fill('');
      state.turns = [{ id: 'native-stopped', status: 'interrupted', items: [] }]; publish();
      await Promise.all([
        page.waitForResponse(response => response.url().endsWith('/resume') && response.request().method() === 'POST' && response.status() === 200),
        page.getByRole('button', { name: '继续任务', exact: true }).click(),
      ]);
      assert.equal(state.calls.filter(call => call.path.includes('/interactions/native-question')).length, 1);
      assert.equal(state.calls.filter(call => call.path.endsWith('/resume')).length, 2);
      result.ids.push('INTERACT-008', 'CONTROL-012');
      state.turns = []; state.interactions = []; publish();
      const menu = async () => { await page.getByRole('button', {name:'更多操作',exact:true}).click(); };
      await menu();
      await page.getByRole('menuitemradio',{name:'规划',exact:true}).click();
      await page.waitForFunction(() => [...document.querySelectorAll('[role="menuitemradio"]')].some(button => button.textContent.includes('规划') && button.getAttribute('aria-checked') === 'true' && !button.disabled));
      await page.getByRole('menuitemradio',{name:'执行',exact:true}).click();
      await page.waitForFunction(() => [...document.querySelectorAll('[role="menuitemradio"]')].some(button => button.textContent.includes('执行') && button.getAttribute('aria-checked') === 'true' && !button.disabled));
      await page.getByRole('menuitemradio',{name:'目标',exact:true}).click();
      await page.getByLabel('希望完成什么',{exact:true}).fill('完成合成目标');
      await page.getByPlaceholder('不设预算',{exact:true}).fill('5000');
      await page.getByRole('button',{name:'设置目标',exact:true}).click();
      await page.locator('.goal-panel').waitFor({state:'hidden'});
      await menu(); await page.getByRole('menuitemradio',{name:'目标',exact:true}).click();
      await page.getByRole('button',{name:'暂停目标',exact:true}).click();
      await page.getByRole('button',{name:'继续目标',exact:true}).click();
      await page.getByRole('button',{name:'结束目标',exact:true}).click();
      await page.locator('.goal-panel').waitFor({state:'hidden'});
      await page.getByRole('button',{name:/访问权限：/}).click();
      await page.getByRole('radio',{name:/只读/}).check();
      await page.getByRole('button',{name:'应用权限',exact:true}).click();
      await page.getByRole('button',{name:'关闭访问权限',exact:true}).waitFor({state:'hidden'});
      await menu(); await page.getByRole('menuitem',{name:'交给子智能体',exact:true}).click();
      await page.getByLabel('任务内容',{exact:true}).fill('检查合成内容');
      await page.getByRole('button',{name:'委派任务',exact:true}).click();
      await page.locator('.delegation-sheet').waitFor({state:'hidden'});
      assert.ok(state.calls.some(call=>call.path.endsWith('/messages') && call.input.prompt.includes('请创建一个子智能体')));
      state.turns=[]; publish();
      for (const label of ['合成插件','合成标签页']) {
        await menu(); await page.getByRole('menuitem',{name:'添加上下文',exact:true}).click();
        await page.locator('.context-result').filter({hasText:label}).click();
        await page.getByRole('button',{name:'移除 '+label,exact:true}).waitFor();
      }
      assert.equal(await page.locator('.composer-context').count(),2);
      await page.getByRole('button',{name:'对话更多操作',exact:true}).click();
      await page.getByRole('menuitem',{name:'管理聊天',exact:true}).click();
      await page.getByLabel('聊天名称',{exact:true}).fill('合成重命名');
      await page.getByRole('menu',{name:'对话操作',exact:true}).waitFor({state:'hidden'});
      await page.screenshot({path:path.join(output,`thread-management-${native}-${viewport.width}.png`), animations:'disabled'});
      await page.getByRole('button',{name:'保存名称',exact:true}).click();
      await page.getByRole('heading',{name:'合成重命名',exact:true}).waitFor();
      await page.getByRole('button',{name:'对话更多操作',exact:true}).click();
      await page.getByRole('menuitem',{name:'管理聊天',exact:true}).click();
      await page.getByRole('button',{name:'置顶聊天',exact:true}).click();
      await page.locator('.thread-management').waitFor({state:'hidden'});
      await page.getByRole('button',{name:'对话更多操作',exact:true}).click();
      await page.getByRole('menuitem',{name:'管理聊天',exact:true}).click();
      await page.getByRole('button',{name:'取消置顶',exact:true}).click();
      await page.locator('.thread-management').waitFor({state:'hidden'});
      await page.getByRole('button',{name:'对话更多操作',exact:true}).click();
      await page.getByRole('menuitem',{name:'管理聊天',exact:true}).click();
      await page.getByRole('button',{name:'归档聊天',exact:true}).click();
      await page.locator('.thread-management').waitFor({state:'hidden'});
      await page.getByRole('button',{name:'已归档聊天',exact:true}).click();
      await page.locator('.thread-browser').getByRole('button').filter({hasText:thread.title}).click();
      await page.getByRole('heading',{name:'合成重命名',exact:true}).waitFor();
      await page.getByRole('button',{name:'对话更多操作',exact:true}).click();
      await page.getByRole('menuitem',{name:'管理聊天',exact:true}).click();
      await page.getByRole('button',{name:'恢复聊天',exact:true}).click();
      await page.locator('.thread-management').waitFor({state:'hidden'});
      await page.getByRole('button',{name:'对话更多操作',exact:true}).click();
      await page.getByRole('menuitem',{name:'管理聊天',exact:true}).click();
      await page.getByRole('button',{name:'删除聊天',exact:true}).click();
      assert.equal(state.calls.filter(call=>call.query==='?action=delete').length,0);
      await page.getByRole('button',{name:'确认删除',exact:true}).click();
      await page.locator('.thread-management').waitFor({state:'hidden'});
      assert.equal(state.calls.filter(call=>call.query==='?action=delete').length,1);
      result.ids.push('CONTROL-016','CONTROL-017','CONTROL-018','CONTROL-019');
      Object.assign(capabilities, { native: false, interactions: true, asyncReplies: false, interrupt: true, queue: true });
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    } catch (error) { result.status = 'failed'; result.error = error.stack || String(error); console.error(result.error); }
    finally { await page.screenshot({ path: path.join(output, `managed-controls-${native ? 'native-' : ''}${viewport.width}.png`) }); result.calls = state.calls; results.push(result); await context.close(); }
  }
} finally {
  fs.writeFileSync(path.join(output, 'managed-controls.json'), JSON.stringify({ environment: 'Docker Chromium with synthetic HTTP/SSE', results, errors }, null, 2));
  await browser.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
}
assert.deepEqual(errors, []); assert.equal(results.filter(result => result.status === 'failed').length, 0);
console.log(`Mobile control component regression: ${results.length} passed`);
