import { effectScope } from 'vue';
import { ApiError } from '../../apps/mobile/src/api.ts';
import { useSubmission } from '../../apps/mobile/src/features/composer/useSubmission.ts';

// 通过真实 API 边界模拟 HTTP；提交状态机由生产 composable 执行。
export function submissionFixture(t, respond, initial = []) {
  const previousFetch = globalThis.fetch;
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const data = new Map([['codex-mobile-pending-v2', JSON.stringify(initial)]]);
  const calls = [];
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: key => data.get(key) || null, setItem: (key, value) => data.set(key, value),
  } });
  globalThis.fetch = async (url, init) => {
    const call = { url: String(url), method: init?.method || 'GET', ...(init?.body ? { body: JSON.parse(init.body) } : {}) };
    calls.push(call);
    try {
      const result = await respond(call, init);
      return result instanceof Response ? result : Response.json(result);
    } catch (failure) {
      if (failure instanceof ApiError) return Response.json({ message: failure.message, outcome: failure.outcome }, { status: failure.status || 503 });
      return Response.json({ message: failure.message || '请求失败。' }, { status: 503 });
    }
  };
  const scope = effectScope();
  const submission = scope.run(() => useSubmission());
  t.after(() => {
    scope.stop(); globalThis.fetch = previousFetch;
    if (previousStorage) Object.defineProperty(globalThis, 'localStorage', previousStorage);
    else delete globalThis.localStorage;
  });
  return { ...submission, calls, data, scope };
}
export function emptyOperations(url) {
  const ids = new URL(url, 'http://localhost').searchParams.get('ids');
  return { operations: [], ...(ids ? { results: ids.split(',').map(id => ({ id, phase: 'missing' })) } : {}) };
}
