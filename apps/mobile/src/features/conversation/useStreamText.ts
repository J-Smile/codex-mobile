import { nextTick, onScopeDispose, ref, watch } from 'vue';

// Keep the live model exact; only batch expensive Markdown rendering while
// long text is being appended. Replacements and final output render at once.
export function useStreamText(source: () => string, streaming: () => boolean, rendered: () => void) {
  const text = ref(source());
  let timer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  function commit() {
    clearTimeout(timer);
    timer = undefined;
    const next = source();
    if (text.value === next) return;
    text.value = next;
    void nextTick(() => { if (!disposed) rendered(); });
  }
  watch([source, streaming], ([next, active]) => {
    if (!active || next.length < 1000 || !next.startsWith(text.value)) { commit(); return; }
    if (!timer) timer = setTimeout(commit, 80);
  });
  onScopeDispose(() => { disposed = true; clearTimeout(timer); });
  return text;
}
