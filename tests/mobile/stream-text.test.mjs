import test from 'node:test';
import assert from 'node:assert/strict';
import { effectScope, nextTick, ref } from 'vue';
import { useStreamText } from '../../apps/mobile/src/features/conversation/useStreamText.ts';

test('长正文渲染合并追加，最终结果与更正立即刷新，卸载取消迟到更新', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const source = ref('a'.repeat(1500)), active = ref(true);
  let renders = 0;
  const scope = effectScope();
  const display = scope.run(() => useStreamText(() => source.value, () => active.value, () => renders++));
  t.after(() => scope.stop());
  for (let i = 0; i < 5; i++) { source.value += '中文🙂'; await nextTick(); }
  assert.equal(display.value.length, 1500);
  t.mock.timers.tick(80); await nextTick();
  assert.equal(display.value, source.value);
  assert.equal(renders, 1);
  source.value += '最终内容'; await nextTick();
  active.value = false; await nextTick();
  assert.equal(display.value, source.value);
  active.value = true; source.value = '更正'; await nextTick();
  assert.equal(display.value, '更正');
  source.value = '更正' + 'b'.repeat(2000); await nextTick();
  scope.stop(); t.mock.timers.tick(1000); await nextTick();
  assert.equal(display.value, '更正');
});
