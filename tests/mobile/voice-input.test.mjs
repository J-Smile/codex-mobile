import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope } from 'vue';
import { useVoiceInput } from '../../apps/mobile/src/features/composer/useVoiceInput.ts';

function fixture(t, options = {}) {
  let context = 'chat-a';
  let draft = '原有草稿';
  const instances = [];
  class Recognition {
    onstart = null;
    onresult = null;
    onerror = null;
    onend = null;
    aborted = false;
    stopped = false;
    constructor() { instances.push(this); }
    start() {
      if (options.startThrows) throw new Error('start failed');
      options.onStart?.(this);
    }
    stop() { this.stopped = true; options.onStop?.(this); }
    abort() { this.aborted = true; }
    result(...texts) { this.onresult?.({ results: texts.map((transcript) => [{ transcript }]) }); }
  }
  const scope = effectScope();
  const voice = scope.run(() => useVoiceInput({
    browser: options.unsupported ? {} : { SpeechRecognition: Recognition, isSecureContext: options.secure !== false },
    contextKey: () => context,
    readDraft: () => draft,
    writeDraft: (value) => { draft = value; },
  }));
  t.after(() => scope.stop());
  return { voice, instances, draft: () => draft, changeContext: () => { context = 'chat-b'; }, dispose: () => scope.stop() };
}

test('语音临时结果整体替换，保留原草稿并避免重复片段', (t) => {
  const { voice, instances, draft } = fixture(t);
  assert.equal(voice.start(), true);
  const recognition = instances[0];
  recognition.onstart();
  recognition.result('打开');
  assert.equal(draft(), '原有草稿 打开');
  recognition.result('打开页面');
  assert.equal(draft(), '原有草稿 打开页面');
  recognition.result('打开页面', '查看结果');
  assert.equal(draft(), '原有草稿 打开页面查看结果');
  voice.stop();
  assert.equal(voice.phase.value, 'stopping');
  recognition.result('打开页面', '查看结果。');
  recognition.onend();
  assert.equal(voice.busy.value, false);
  assert.equal(draft(), '原有草稿 打开页面查看结果。');
});

test('重复开始不会并发录音，取消后旧回调不能覆盖新对话', (t) => {
  const { voice, instances, draft, changeContext } = fixture(t);
  voice.start();
  assert.equal(voice.start(), false);
  assert.equal(instances.length, 1);
  const recognition = instances[0];
  const oldResult = recognition.onresult;
  recognition.result('第一段');
  changeContext();
  voice.clear();
  assert.equal(recognition.aborted, true);
  oldResult({ results: [[{ transcript: '迟到的结果' }]] });
  assert.equal(draft(), '原有草稿 第一段');
  voice.start();
  oldResult({ results: [[{ transcript: '再次迟到' }]] });
  assert.equal(draft(), '原有草稿 第一段');
});

test('权限和网络失败释放识别状态，草稿保留且可重试', (t) => {
  const { voice, instances, draft } = fixture(t, { secure: false });
  voice.start();
  instances[0].onerror({ error: 'not-allowed' });
  assert.equal(voice.busy.value, false);
  assert.match(voice.error.value, /HTTPS/);
  assert.equal(draft(), '原有草稿');
  assert.equal(voice.start(), true);
  instances[1].onerror({ error: 'network' });
  assert.match(voice.error.value, /连接失败/);
  assert.equal(voice.busy.value, false);
});

test('不支持的浏览器和启动异常不影响文字输入', (t) => {
  const unsupported = fixture(t, { unsupported: true });
  assert.equal(unsupported.voice.start(), false);
  assert.match(unsupported.voice.error.value, /键盘/);
  assert.equal(unsupported.draft(), '原有草稿');
  const failed = fixture(t, { startThrows: true });
  assert.equal(failed.voice.start(), false);
  assert.equal(failed.voice.busy.value, false);
  assert.match(failed.voice.error.value, /未能启动/);
});

test('启动没有任何回调时释放输入，保留草稿，迟到回调不能影响重试', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { voice, instances, draft } = fixture(t);
  assert.equal(voice.start(), true);
  const pending = instances[0];
  const oldStart = pending.onstart;
  const oldResult = pending.onresult;
  const oldEnd = pending.onend;
  t.mock.timers.tick(14_999);
  assert.equal(voice.phase.value, 'starting');
  t.mock.timers.tick(1);
  assert.equal(voice.busy.value, false);
  assert.equal(pending.aborted, true);
  assert.equal(draft(), '原有草稿');
  assert.match(voice.error.value, /启动超时/);

  assert.equal(voice.start(), true);
  oldStart();
  oldResult({ results: [[{ transcript: '旧会话的文字' }]] });
  oldEnd();
  assert.equal(voice.phase.value, 'starting');
  assert.equal(draft(), '原有草稿');
  assert.equal(voice.error.value, '');
  instances[1].onstart();
  instances[1].result('重新识别');
  assert.equal(draft(), '原有草稿 重新识别');
});

test('开始事件或实际识别结果解除启动等待，不限制正常听写时长', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const startedBy of ['start', 'result']) {
    const { voice, instances } = fixture(t);
    voice.start();
    t.mock.timers.tick(14_000);
    const recognition = instances[0];
    if (startedBy === 'start') recognition.onstart();
    else recognition.result('已开始识别');
    assert.equal(voice.phase.value, 'listening');
    t.mock.timers.tick(60_000);
    assert.equal(voice.phase.value, 'listening');
    assert.equal(recognition.aborted, false);
    assert.equal(voice.error.value, '');
    voice.clear();
  }
});

test('结束缺少确认时保留最终文字；仍有识别结果会延长等待', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { voice, instances, draft } = fixture(t);
  voice.start();
  const recognition = instances[0];
  recognition.onstart();
  recognition.result('暂时结果');
  voice.stop();
  assert.equal(recognition.stopped, true);
  t.mock.timers.tick(4_000);
  recognition.result('最后补全的结果。');
  recognition.onstart();
  assert.equal(voice.phase.value, 'stopping');
  t.mock.timers.tick(4_000);
  recognition.result('最后补全的结果。');
  t.mock.timers.tick(999);
  assert.equal(voice.phase.value, 'stopping');
  t.mock.timers.tick(1);
  assert.equal(voice.busy.value, false);
  assert.equal(recognition.aborted, true);
  assert.equal(draft(), '原有草稿 最后补全的结果。');
  assert.match(voice.error.value, /结束确认超时.*已保留/);
});

test('同步开始、结束与错误回调不会遗留计时器或误报启动成功', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { voice, instances } = fixture(t, {
    onStart: (recognition) => recognition.onstart(),
    onStop: (recognition) => recognition.onend(),
  });
  assert.equal(voice.start(), true);
  assert.equal(voice.phase.value, 'listening');
  t.mock.timers.tick(60_000);
  assert.equal(voice.busy.value, true);
  voice.stop();
  assert.equal(voice.busy.value, false);
  assert.equal(instances[0].aborted, false);
  assert.equal(voice.start(), true);
  t.mock.timers.tick(60_000);
  assert.equal(voice.phase.value, 'listening');
  assert.equal(voice.error.value, '');
  voice.clear();

  const failed = fixture(t, { onStart: (recognition) => recognition.onerror({ error: 'network' }) });
  assert.equal(failed.voice.start(), false);
  t.mock.timers.tick(60_000);
  assert.equal(failed.voice.busy.value, false);
  assert.match(failed.voice.error.value, /连接失败/);
});

test('手动取消启动、清理上下文和卸载都清除等待并保留草稿', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { voice, instances, draft, changeContext, dispose } = fixture(t);
  voice.start();
  voice.stop();
  assert.equal(voice.busy.value, false);
  assert.equal(instances[0].aborted, true);
  t.mock.timers.tick(14_000);

  voice.start();
  t.mock.timers.tick(1_000);
  assert.equal(voice.phase.value, 'starting');
  changeContext();
  voice.clear();
  t.mock.timers.tick(60_000);
  assert.equal(voice.error.value, '');
  assert.equal(voice.busy.value, false);

  voice.start();
  dispose();
  t.mock.timers.tick(60_000);
  assert.equal(instances[2].aborted, true);
  assert.equal(voice.error.value, '');
  assert.equal(voice.busy.value, false);
  assert.equal(draft(), '原有草稿');
});
