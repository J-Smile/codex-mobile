import { computed, onScopeDispose, ref, watch } from 'vue';
import { ApiError, request } from '../../api.ts';
import type { DesktopThread, Project } from '../../types';

export function threadActive(thread: DesktopThread) {
  return typeof thread.status === 'object' ? thread.status?.type === 'active' : thread.status === 'active';
}

export function timestampMs(value = 0) {
  return value > 1e12 ? value : value * 1000;
}

export function recentThreads(threads: DesktopThread[], limit: number) {
  return [...threads].sort((a, b) => timestampMs(b.updatedAt) - timestampMs(a.updatedAt)).slice(0, limit);
}

export const projectPreviewCount = 3;
type ThreadPage = { threads: DesktopThread[]; nextCursor: string | null };

function projectPreviews(threads: DesktopThread[]) {
  const groups = new Map<string, DesktopThread[]>();
  for (const thread of threads) {
    if (!thread.projectId) continue;
    const rows = groups.get(thread.projectId);
    if (rows) rows.push(thread);
    else groups.set(thread.projectId, [thread]);
  }
  for (const [id, rows] of groups) groups.set(id, recentThreads(rows, projectPreviewCount));
  return groups;
}

export function useThreadCatalog(options: { authenticated: () => boolean; expired: () => void; capabilities?: (value: Record<string, boolean>) => void }) {
  const threads = ref<DesktopThread[]>([]);
  const projects = ref<Project[]>([]);
  const projectHistory = ref<DesktopThread[]>([]);
  const previewCursor = ref<string | null>();
  const hasMoreProjectHistory = computed(() => !!previewCursor.value);
  const knownThreads = computed(() => {
    const recent = new Set(threads.value.map(thread => thread.id));
    return [...threads.value, ...projectHistory.value.filter(thread => !recent.has(thread.id))];
  });
  const latestByProject = computed(() => projectPreviews(knownThreads.value));
  const loadingThreads = ref(false);
  const loadingProjects = ref(false);
  const loadingProjectHistory = ref(false);
  const projectHistoryError = ref(false);
  const error = ref('');
  let threadRequest: Promise<void> | undefined;
  let projectRequest: Promise<void> | undefined;
  let threadController: AbortController | undefined;
  let projectController: AbortController | undefined;
  let historyController: AbortController | undefined;
  let disposed = false;

  function cancelRequests() {
    threadController?.abort();
    projectController?.abort();
    historyController?.abort();
    threadController = undefined;
    projectController = undefined;
    historyController = undefined;
    threadRequest = undefined;
    projectRequest = undefined;
    loadingThreads.value = false;
    loadingProjects.value = false;
    loadingProjectHistory.value = false;
  }

  function fail(failure: unknown, controller: AbortController) {
    if (disposed || !options.authenticated() || (controller !== threadController && controller !== projectController)) return;
    if (failure instanceof ApiError && failure.status === 401) options.expired();
    else error.value = controller.signal.aborted ? '刷新超时，请检查网络后重试。' : (failure as Error).message;
  }

  function loadProjects() {
    if (projectRequest) return projectRequest;
    if (!options.authenticated() || disposed) return Promise.resolve();
    loadingProjects.value = true;
    const controller = new AbortController();
    projectController = controller;
    const timeout = setTimeout(() => controller.abort(), 15_000);
    projectRequest = request<{ projects: Project[]; capabilities?: Record<string, boolean> }>('/api/projects', { signal: controller.signal })
      .then((result) => { if (!controller.signal.aborted && !disposed && options.authenticated()) {
        projects.value = result.projects;
        if (result.capabilities) options.capabilities?.(result.capabilities);
      } })
      .catch((failure) => fail(failure, controller))
      .finally(() => {
        clearTimeout(timeout);
        if (projectController === controller) { projectRequest = undefined; projectController = undefined; loadingProjects.value = false; }
      });
    return projectRequest;
  }

  function rememberProjectHistory(entries: DesktopThread[]) {
    const known = new Map(projectHistory.value.map(thread => [thread.id, thread]));
    for (const thread of entries) known.set(thread.id, thread);
    for (const thread of threads.value) known.delete(thread.id);
    projectHistory.value = Array.from(projectPreviews([...known.values()]).values()).flat();
  }

  function updateCatalogThread(thread: DesktopThread) {
    const current = threads.value.find(entry => entry.id === thread.id);
    if (current) Object.assign(current, thread);
    const historical = projectHistory.value.find(entry => entry.id === thread.id);
    if (historical) Object.assign(historical, thread);
  }

  function loadThreads(): Promise<void> {
    if (threadRequest) return threadRequest;
    if (!options.authenticated() || disposed) return Promise.resolve();
    loadingThreads.value = true;
    const controller = new AbortController();
    threadController = controller;
    const timeout = setTimeout(() => controller.abort(), 15_000);
    threadRequest = request<ThreadPage>('/api/threads', { signal: controller.signal })
      .then(result => {
        if (controller.signal.aborted || disposed || !options.authenticated()) return;
        const previous = threads.value;
        threads.value = result.threads;
        if (!result.nextCursor) {
          historyController?.abort();
          projectHistory.value = [];
          previewCursor.value = null;
        } else if (previewCursor.value === undefined) {
          previewCursor.value = result.nextCursor;
        } else {
          // 保留从首屏被挤出的项目记录，刷新无需从头重读已翻过的页。
          rememberProjectHistory(previous.filter(thread => thread.projectId));
        }
      }).catch((failure) => fail(failure, controller))
      .finally(() => {
        clearTimeout(timeout);
        if (threadController === controller) { threadRequest = undefined; threadController = undefined; loadingThreads.value = false; }
      });
    return threadRequest;
  }

  async function loadProjectHistory() {
    const cursor = previewCursor.value;
    if (!cursor || historyController || projectHistoryError.value || document.hidden || !options.authenticated() || disposed) return;
    const controller = new AbortController();
    historyController = controller;
    loadingProjectHistory.value = true;
    try {
      const page = await request<ThreadPage>('/api/threads?cursor=' + encodeURIComponent(cursor), { signal: controller.signal, timeoutMs: 15_000 });
      if (controller.signal.aborted) return;
      rememberProjectHistory(page.threads);
      previewCursor.value = page.nextCursor;
    } catch (failure) {
      if (controller.signal.aborted) return;
      if (failure instanceof ApiError && failure.status === 401) options.expired();
      else projectHistoryError.value = true;
    } finally {
      if (historyController === controller) {
        historyController = undefined;
        loadingProjectHistory.value = false;
      }
    }
  }

  async function refresh() {
    if (document.hidden || !options.authenticated() || disposed) return;
    error.value = '';
    await Promise.all([loadProjects(), loadThreads()]);
  }
  function resume(event?: Event) {
    if (document.hidden) cancelRequests();
    else {
      if (event?.type === 'online') projectHistoryError.value = false;
      void refresh();
    }
  }
  watch(options.authenticated, (authenticated) => {
    if (authenticated) return;
    cancelRequests();
    threads.value = [];
    projects.value = [];
    projectHistory.value = [];
    previewCursor.value = undefined;
    projectHistoryError.value = false;
    error.value = '';
  }, { flush: 'sync' });
  const interval = setInterval(resume, 30_000);
  document.addEventListener('visibilitychange', resume);
  window.addEventListener('focus', resume);
  window.addEventListener('online', resume);
  window.addEventListener('pageshow', resume);
  onScopeDispose(() => {
    disposed = true;
    cancelRequests();
    clearInterval(interval);
    document.removeEventListener('visibilitychange', resume);
    window.removeEventListener('focus', resume);
    window.removeEventListener('online', resume);
    window.removeEventListener('pageshow', resume);
  });
  return {
    threads, projects, knownThreads, latestByProject, loadingThreads, loadingProjects, error, loadThreads, refresh,
    hasMoreProjectHistory, loadingProjectHistory, projectHistoryError, loadProjectHistory, updateCatalogThread,
  };
}
