import { computed, onScopeDispose, ref } from 'vue';

interface RecognitionResult {
  readonly [index: number]: { transcript: string };
}
interface Recognition {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  maxAlternatives: number;
  onstart: (() => void) | null;
  onresult: ((event: { results: ArrayLike<RecognitionResult> }) => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
  abort(): void;
}
interface VoiceBrowser {
  SpeechRecognition?: new () => Recognition;
  webkitSpeechRecognition?: new () => Recognition;
  isSecureContext?: boolean;
}

const START_WAIT_MS = 15_000;
const STOP_WAIT_MS = 5_000;

export function useVoiceInput(options: {
  contextKey: () => string;
  readDraft: () => string;
  writeDraft: (text: string) => void;
  browser?: VoiceBrowser;
}) {
  const browser = options.browser || window as unknown as VoiceBrowser;
  const Constructor = browser.SpeechRecognition || browser.webkitSpeechRecognition;
  const supported = Boolean(Constructor);
  const phase = ref<'idle' | 'starting' | 'listening' | 'stopping'>('idle');
  const error = ref('');
  const busy = computed(() => phase.value !== 'idle');
  let session: { recognition: Recognition; context: string; baseDraft: string } | null = null;
  let transitionTimer: ReturnType<typeof setTimeout> | undefined;

  function cancel() {
    clearTimeout(transitionTimer);
    transitionTimer = undefined;
    const current = session;
    session = null;
    phase.value = 'idle';
    if (current) {
      current.recognition.onstart = null;
      current.recognition.onresult = null;
      current.recognition.onerror = null;
      current.recognition.onend = null;
      try { current.recognition.abort(); }
      catch { /* 已结束的识别不需要再次中止。 */ }
    }
  }

  function clear() {
    cancel();
    error.value = '';
  }

  function waitForTransition() {
    clearTimeout(transitionTimer);
    const current = session;
    const waitingPhase = phase.value;
    transitionTimer = setTimeout(() => {
      if (session !== current || phase.value !== waitingPhase) return;
      cancel();
      error.value = waitingPhase === 'starting'
        ? '语音启动超时。请检查麦克风权限后重试，也可以使用手机键盘的麦克风。'
        : '语音结束确认超时，已保留当前草稿，可以继续输入或重试语音。';
    }, waitingPhase === 'starting' ? START_WAIT_MS : STOP_WAIT_MS);
  }

  function errorMessage(code: string) {
    if (code === 'not-allowed' || code === 'service-not-allowed') {
      return browser.isSecureContext === false
        ? '浏览器未允许语音识别。请在 HTTPS 页面允许麦克风，或使用手机键盘的麦克风。'
        : '麦克风或语音识别服务未获允许。请检查浏览器权限，也可以使用键盘的麦克风。';
    }
    if (code === 'audio-capture') return '没有可用的麦克风，请检查设备连接。';
    if (code === 'no-speech') return '没有识别到语音，请点击麦克风重试。';
    if (code === 'network') return '语音识别服务连接失败。请检查网络，或使用手机键盘的麦克风。';
    if (code === 'language-not-supported') return '当前浏览器不支持中文识别，请使用手机键盘的麦克风。';
    return '语音识别未能完成，请重试或使用键盘的麦克风。';
  }

  function start() {
    if (busy.value || !options.contextKey()) return false;
    error.value = '';
    if (!Constructor) {
      error.value = '此浏览器不支持页面语音识别。请使用手机键盘的麦克风，或通过 HTTPS 在 Chrome / Safari 打开。';
      return false;
    }
    try {
      const recognition = new Constructor();
      const current = { recognition, context: options.contextKey(), baseDraft: options.readDraft() };
      session = current;
      phase.value = 'starting';
      recognition.lang = 'zh-CN';
      recognition.continuous = true;
      recognition.interimResults = true;
      recognition.maxAlternatives = 1;
      recognition.onstart = () => {
        if (session !== current || phase.value !== 'starting') return;
        clearTimeout(transitionTimer);
        transitionTimer = undefined;
        phase.value = 'listening';
      };
      recognition.onresult = (event) => {
        if (session !== current || options.contextKey() !== current.context) return;
        // 每次结果包含本次识别的全部片段，整体替换转写部分可避免临时结果重复追加。
        const parts = Array.from({ length: event.results.length }, (_, index) => event.results[index][0]?.transcript || '');
        const text = parts.join('');
        if (!text) return;
        const separator = current.baseDraft && !/\s$/.test(current.baseDraft) ? ' ' : '';
        const nextDraft = current.baseDraft + separator + text;
        if (phase.value === 'starting') {
          clearTimeout(transitionTimer);
          transitionTimer = undefined;
          phase.value = 'listening';
        } else if (phase.value === 'stopping' && nextDraft !== options.readDraft()) {
          // 停止后的结果仍可能分批送达，有进展时继续等待最终确认。
          waitForTransition();
        }
        options.writeDraft(nextDraft);
      };
      recognition.onerror = (event) => {
        if (session !== current) return;
        cancel();
        if (event.error !== 'aborted') error.value = errorMessage(event.error);
      };
      recognition.onend = () => {
        if (session !== current) return;
        clearTimeout(transitionTimer);
        transitionTimer = undefined;
        session = null;
        phase.value = 'idle';
      };
      // 在调用浏览器前计时，同步回调也能正确清理；正常听写没有时长限制。
      waitForTransition();
      recognition.start();
      return session === current;
    } catch {
      cancel();
      error.value = '浏览器未能启动语音识别，请重试或使用键盘的麦克风。';
      return false;
    }
  }

  function stop() {
    if (!session) return;
    if (phase.value !== 'listening') { cancel(); return; }
    phase.value = 'stopping';
    waitForTransition();
    try { session.recognition.stop(); }
    catch { cancel(); }
  }

  onScopeDispose(clear);
  return { supported, phase, busy, error, start, stop, clear };
}
