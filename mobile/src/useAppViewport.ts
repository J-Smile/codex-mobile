import { onMounted, onScopeDispose, ref } from 'vue';

export function useAppViewport(options: {
  composerFocused: () => boolean;
  hasFixedShell: () => boolean;
  onResize: (widthChanged: boolean) => void;
}) {
  const keyboardOpen = ref(false);
  const shortKeyboardViewport = ref(false);
  const viewport = window.visualViewport;
  const root = document.documentElement;
  let width = 0, height = 0, restingHeight = 0;
  let frame = 0, settleTimer = 0;
  let closingKeyboard = false;

  function clearGeometry() {
    root.style.removeProperty('--app-height');
    root.style.removeProperty('--viewport-top');
    root.style.removeProperty('--safe-bottom-visible');
  }

  function measure() {
    frame = 0;
    const nextWidth = viewport?.width || window.innerWidth;
    const layoutHeight = Math.max(window.innerHeight, root.clientHeight);
    const visibleHeight = viewport?.height || window.innerHeight;
    const widthChanged = nextWidth !== width;
    const focused = document.activeElement as HTMLElement | null;
    const editing = focused?.isContentEditable || focused?.matches('input, textarea, select');
    // resizes-content 下布局视口也会随键盘缩小，基准只沿用同一宽度的无键盘高度。
    const baseline = widthChanged ? layoutHeight : Math.max(layoutHeight, restingHeight);
    const open = !!(editing || closingKeyboard) && baseline - visibleHeight > 100;
    if (open) {
      const top = Math.min(Math.max(0, viewport?.offsetTop || 0), Math.max(0, layoutHeight - visibleHeight));
      root.style.setProperty('--app-height', `${visibleHeight}px`);
      root.style.setProperty('--viewport-top', `${top}px`);
      root.style.setProperty('--safe-bottom-visible', '0px');
    } else {
      restingHeight = layoutHeight;
      closingKeyboard = false;
      clearGeometry();
      // 只归位固定外壳所在的文档，正文和输入框各自保留滚动位置。
      if (window.scrollY && options.hasFixedShell()) window.scrollTo({ top: 0, behavior: 'instant' });
    }
    keyboardOpen.value = open;
    shortKeyboardViewport.value = open && visibleHeight < 320 && (options.composerFocused() || shortKeyboardViewport.value);
    const nextHeight = open ? visibleHeight : layoutHeight;
    if (widthChanged || nextHeight !== height) {
      width = nextWidth;
      height = nextHeight;
      options.onResize(widthChanged);
    }
  }

  function schedule() {
    if (!document.hidden && !frame) frame = window.requestAnimationFrame(measure);
  }

  function recover(event: Event) {
    window.clearTimeout(settleTimer);
    window.cancelAnimationFrame(frame);
    frame = 0;
    if (document.hidden) return;
    closingKeyboard = event.type === 'focusout' && keyboardOpen.value;
    schedule();
    // 回前台或失焦时，WebKit 可能要到转场结束才给出最终尺寸，且没有后续 resize。
    settleTimer = window.setTimeout(() => { closingKeyboard = false; schedule(); }, 350);
  }

  onMounted(() => {
    window.addEventListener('resize', schedule);
    viewport?.addEventListener('resize', schedule);
    viewport?.addEventListener('scroll', schedule);
    window.addEventListener('orientationchange', recover);
    window.addEventListener('pageshow', recover);
    document.addEventListener('visibilitychange', recover);
    document.addEventListener('focusin', schedule);
    document.addEventListener('focusout', recover);
    schedule();
  });
  onScopeDispose(() => {
    window.removeEventListener('resize', schedule);
    viewport?.removeEventListener('resize', schedule);
    viewport?.removeEventListener('scroll', schedule);
    window.removeEventListener('orientationchange', recover);
    window.removeEventListener('pageshow', recover);
    document.removeEventListener('visibilitychange', recover);
    document.removeEventListener('focusin', schedule);
    document.removeEventListener('focusout', recover);
    window.clearTimeout(settleTimer);
    window.cancelAnimationFrame(frame);
    clearGeometry();
  });
  return { keyboardOpen, shortKeyboardViewport };
}
