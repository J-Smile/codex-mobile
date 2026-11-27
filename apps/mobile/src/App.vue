<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onMounted, onUnmounted, reactive, ref, shallowRef, watch, watchEffect } from 'vue';
import { AnimatePresence, motion, useReducedMotion } from 'motion-v';
import ImagePreview from './features/conversation/preview/ImagePreview.vue';
import FilePreview from './features/conversation/preview/FilePreview.vue';
import { previewFile } from './features/conversation/preview/filePreview';
import MessageQueue from './features/composer/MessageQueue.vue';
import PendingInteractions from './features/conversation/PendingInteractions.vue';
import ComposerContext from './features/composer/ComposerContext.vue';
import NativeSheet from './components/NativeSheet.vue';
import UpdateSettings from './features/settings/UpdateSettings.vue';
import CollapseMotion from './components/CollapseMotion.vue';
import PlainTextInput from './features/composer/PlainTextInput.vue';
import UsagePanel from './features/usage/UsagePanel.vue';
import ThreadBrowser from './features/threads/ThreadBrowser.vue';
import ThreadManagement from './features/threads/ThreadManagement.vue';
import GoalPanel from './features/goals/GoalPanel.vue';
import { goalStatusLabels, type GoalInput, type GoalState } from './features/goals/goals';
import { preloadMarkdownPreview } from './features/conversation/preview/markdownPreview';
import { composeDraftContext, splitDraftContext } from './features/composer/composerContext';
import { ApiError, request, threadUrl } from './api';
import { desktopStorageKey } from './transport';
import { mediaUrl } from './relayMedia';
import { conversationParams, setConversationAddress } from './features/conversation/links';
import { useBrowserTheme } from './browserChrome';
import { useAppViewport } from './useAppViewport';
import type { DesktopThread, HistoryPage, MessageModel, Project, StreamSnapshot, StreamUpdate, Turn } from './types';
import type { PendingInteraction } from './features/conversation/interactions';
import { subagentTargets, type DelegationRequest } from './features/composer/subagentDelegation';
import { useMessageQueue, type MessageMode, type QueueSnapshot } from './features/composer/useMessageQueue';
import { useVoiceInput } from './features/composer/useVoiceInput';
import { useSubmission, type PendingOperation } from './features/composer/useSubmission';
import { useImageAttachments, type UploadedImage } from './features/composer/useImageAttachments';
import { useConversationHistory, type ConversationFrame } from './features/conversation/useConversationHistory';
import { useNotifications } from './features/settings/useNotifications';
import { mergeStreamUpdate, useThreadStream } from './features/conversation/useThreadStream';
import { projectPreviewCount, recentThreads, threadActive, timestampMs, useThreadCatalog } from './features/threads/useThreadCatalog';
import { useAppUpdate } from './useAppUpdate';
import { useDesktopUpgrade } from './features/settings/useDesktopUpgrade';
import { Aim, ArrowDown, ArrowLeft, ArrowRight, Bell, ChatLineRound, Check, Close, Connection, DocumentAdd, EditPen, FolderOpened, FullScreen, Histogram, Loading, Microphone, Monitor, Moon, MoreFilled, Picture, Plus, Refresh, Search, Setting, Sunny, SwitchButton, Top, VideoPause } from '@element-plus/icons-vue';

const QrConnect = defineAsyncComponent(() => import('./features/pairing/QrConnect.vue'));
const TurnView = defineAsyncComponent(() => import('./features/conversation/TurnView.vue'));
const ItemView = defineAsyncComponent(() => import('./features/conversation/ItemView.vue'));
const ModelSettings = defineAsyncComponent(() => import('./features/settings/ModelSettings.vue'));
const PermissionSettings = defineAsyncComponent(() => import('./features/settings/PermissionSettings.vue'));
const HookReview = defineAsyncComponent(() => import('./features/settings/HookReview.vue'));
const AgentDelegation = defineAsyncComponent(() => import('./features/composer/AgentDelegation.vue'));
const props = defineProps<{ initialSession?: Promise<unknown>; desktopName?: string; devicePickerOpen?: boolean }>();
const emit = defineEmits<{ chooseDesktop: [] }>();

const authenticated = ref(false);
const capabilities = ref<Record<string, boolean>>({});
const submissions = useSubmission();
const unresolvedOperations = submissions.unresolvedOperations;
const recoveredCreations = submissions.recoveredCreations;
const operationsReady = submissions.operationsReady;
const operationsError = submissions.operationsError;
const loadOperations = submissions.loadOperations;
const closeOperation = submissions.closeOperation;
const booting = ref(true);
const loginKey = ref('');
const loggingIn = ref(false);
const loginError = ref('');
const loginFailureStatus = ref<number | null>(null);
const serviceUnavailable = computed(() => loginFailureStatus.value === 0);
const {
  threads, projects, knownThreads, latestByProject, loadingThreads, loadingProjects, error: catalogError,
  loadThreads, refresh: refreshCatalog, hasMoreProjectHistory, loadingProjectHistory, projectHistoryError, loadProjectHistory, updateCatalogThread, removeCatalogThread,
} = useThreadCatalog({
  authenticated: () => authenticated.value,
  expired: expireConnection,
  capabilities: value => {
    capabilities.value = selectedId.value ? { ...capabilities.value, create: value.create, native: value.native, modelSelection: value.modelSelection } : value;
  },
});
const hasProjects = computed(() => projects.value.length > 0 || !!capabilities.value.catalog);
const selectedProjectId = ref('');
const newChat = ref(false);
const creationSource = ref('');
const creationProjects = ref<Project[]>([]);
const creationLoading = ref(false);
const creationError = ref('');
const firstTurnPending = ref(false);
const createdDraft = ref<{threadId:string; key:string; prompt:string; images:string[]; selection:MessageModel} | null>(null);
const navigationOpen = ref(true);
const usageOpen = ref(false);
const usagePresented = ref(false);
const usagePanel = ref<{ back: () => boolean; hasParent: boolean }>();
const usageThreadId = ref('');
const usageThreadTitle = ref('');
const usageReturnToChat = ref(false);
const view = computed(() => usageOpen.value ? 'usage' : !navigationOpen.value && (selectedId.value || newChat.value) ? 'chat' : 'projects');
const projectPickerOpen = ref(false);
const projectsExpanded = ref(true);
const catalogScope = ref<Project | null>();
const archivedCatalog = ref(false);
const managementOpen = ref(false);
let catalogTrigger: HTMLElement | undefined;
const navigationScroll = ref<HTMLElement>();
const projectsToggle = ref<HTMLButtonElement>();
const searchToggle = ref<HTMLButtonElement>();
const searchProjectsExpanded = ref(true);
const collapsedProjects = reactive<Record<string, boolean>>({});
const searchCollapsedProjects = reactive<Record<string, boolean>>({});
const searchOpen = ref(false);
const searchInput = ref<InstanceType<typeof PlainTextInput>>();
const settingsOpen = ref(false);
const chatMenuTrigger = ref<HTMLButtonElement>();
const disconnecting = ref(false);
const disconnectError = ref('');
const disconnectFailure = ref<HTMLElement>();
const updateSettingsOpen = ref(false);
const goalOpen = ref(false);
const modeBusy = ref(false);
const scanOpen = ref(false);
const selectedId = ref('');
const filePreview = ref<InstanceType<typeof FilePreview>>();
const selectedThread = ref<DesktopThread | null>(null);
const search = ref('');
watch(search, () => {
  for (const id of Object.keys(searchCollapsedProjects)) delete searchCollapsedProjects[id];
  searchProjectsExpanded.value = true;
});
const projectsOpen = computed(() => search.value.trim() ? searchProjectsExpanded.value : projectsExpanded.value);
// 快照、分页与流式合并都替换轮次数组，正文不需要深层响应式代理。
const turns = shallowRef<Turn[]>([]);
const historyCursor = ref<string | null>(null);
const changedFileCount = ref<number>();
const loadingHistory = ref(false);
const historyError = ref('');
let historyRequest: AbortController | undefined;
let historyGap: { olderId: string; newerId: string; cursor: string } | undefined;
let restoringFrame: ConversationFrame | undefined;
const pendingSends = submissions.pendingSends;
const stopping = ref(false);
const resuming = ref(false);
const composerMenu = ref(false);
const reduceMotion = useReducedMotion();
const controlSpring = computed(() => reduceMotion.value
  ? { duration: 0 } : { type: 'spring' as const, visualDuration: .22, bounce: .12 });
const closedComposerMenu = computed(() => ({ opacity: 0, y: reduceMotion.value ? 0 : 8, scale: reduceMotion.value ? 1 : .97 }));
const composerPlus = ref<HTMLButtonElement>();
const chatMenu = ref(false);
const contextOpen = ref(false);
const delegationOpen = ref(false);
const delegationError = ref('');
const delegationSource = ref('');
const interactions = ref<PendingInteraction[]>([]);
const interactionState = ref({ expanded: false, pending: false });
const pendingPanel = ref<{ minimize: () => void }>();
const notice = ref('');
watch(notice, (value, _, cleanup) => {
  if (!value) return;
  const timer = setTimeout(() => { notice.value = ''; }, 3500);
  cleanup(() => clearTimeout(timer));
});
const pendingMessages = ref<{ threadId: string; requestId: string; prompt: string; images: UploadedImage[]; accepted: boolean }[]>([]);
const drafts = reactive<Record<string, string>>(readDrafts());
const modelDrafts = reactive<Record<string, MessageModel>>(readModelDrafts());
const error = ref('');
const messageQueue = useMessageQueue({ threadId: () => capabilities.value.queue ? selectedId.value : '', authenticated: () => authenticated.value });
const { queue: queueState, loading: queueLoading, busy: queueBusy, error: queueError } = messageQueue;
const { status: streamStatus, loading: loadingInitial, error: connectionError, resume: resumeStream } = useThreadStream({
  threadId: () => selectedId.value,
  authenticated: () => authenticated.value,
  snapshot: applySnapshot,
  update: applyUpdate,
  queue: messageQueue.acceptSnapshot,
  expired: expireConnection,
});
const shell = ref<HTMLElement>();
const scroller = ref<HTMLElement>();
const chatPane = ref<HTMLElement>();
const chatTopbar = ref<HTMLElement>();
const composerRegion = ref<HTMLElement>();
const composer = ref<InstanceType<typeof PlainTextInput>>();
const imagePicker = ref<HTMLInputElement>();
const stickToBottom = ref(true);
const newOutput = ref(false);
const latestInHeader = ref(false);
const theme = useBrowserTheme(() => authenticated.value && view.value === 'projects' ? 'navigation' : 'chat');
const notifications = useNotifications(() => authenticated.value, openNotifiedThread);
const activeProject = computed(() => projects.value.find((project) => project.projectId === selectedProjectId.value));
const projectTitle = computed(() => activeProject.value?.label || (selectedProjectId.value === 'other' ? '其他对话' : '项目'));
const draftKey = computed(() => newChat.value ? 'new:' + selectedProjectId.value : selectedId.value);
const sending = computed(() => pendingSends.has(draftKey.value) || resuming.value);
const currentOperation = computed(() => unresolvedOperations.value.find(operation => operation.target === draftKey.value));
const currentCreation = computed(() => recoveredCreations.value.find(operation => operation.target === draftKey.value));
const attachments = useImageAttachments({ draftKey: () => draftKey.value });
const { images: draftImages, uploading: imagesUploading, hasErrors: imageErrors } = attachments;
const conversationHistory = useConversationHistory({
  capture: () => selectedId.value ? {
    threadId: selectedId.value, projectId: selectedProjectId.value, thread: selectedThread.value,
    // 未补齐的尾部不写入阅读缓存，返回后仍从连续的已读范围继续同步。
    turns: historyGap ? turns.value.slice(0, turns.value.findIndex(turn => turn.id === historyGap?.olderId) + 1) : turns.value,
    historyCursor: historyCursor.value, changedFileCount: changedFileCount.value, scrollTop: scroller.value?.scrollTop || 0,
    stickToBottom: stickToBottom.value, newOutput: newOutput.value,
  } : undefined,
  activate: activateConversation,
});

const draft = computed({
  get: () => drafts[draftKey.value] || '',
  set: (value: string) => { drafts[draftKey.value] = value; },
});
const modelSelection = computed({
  get: () => modelDrafts[draftKey.value] || {},
  set: (value: MessageModel) => {
    if (value.model || value.thinking) modelDrafts[draftKey.value] = value;
    else delete modelDrafts[draftKey.value];
  },
});
const draftParts = computed(() => splitDraftContext(draft.value));
const draftText = computed({
  get: () => draftParts.value.text,
  set: (value: string) => { draft.value = composeDraftContext(draftParts.value.contexts, value); },
});
const voice = useVoiceInput({
  contextKey: () => draftKey.value,
  readDraft: () => draftText.value,
  writeDraft: (text) => {
    draftText.value = text;
    void nextTick(resizeComposer);
  },
});
function canReloadApp() {
  return !props.devicePickerOpen && !booting.value && !pendingSends.size && !stopping.value && !voice.busy.value && !settingsOpen.value && !updateSettingsOpen.value && !goalOpen.value && !modeBusy.value
    && !pendingMessages.value.length && !scanOpen.value && !projectPickerOpen.value
    && !notifications.busy.value && !delegationOpen.value && !contextOpen.value && !queueBusy.value && !interactions.value.some(entry => entry.replySupported)
    && attachments.canReload.value
    && !document.querySelector('dialog[open]');
}
function openConversationLink(event: MouseEvent) {
  const target = event.target instanceof Element ? event.target.closest<HTMLButtonElement>('button.file-preview-link') : null;
  const url = target?.dataset.filePreview;
  if (!target || !url) return conversationHistory.openLink(event);
  if (!url.startsWith(threadUrl(selectedId.value, 'attachment') + '?') || !url.endsWith('&preview=file')) return;
  const file = previewFile(new URL(url, location.origin).searchParams.get('path') || '');
  if (file && file.kind !== 'disabled') void filePreview.value?.open(url, { name: file.name, kind: file.kind }, target);
}
function preserveBeforeReload() {
  if (!saveDrafts() && Object.values(drafts).some(Boolean)) return false;
  if (!saveModelDrafts() && Object.keys(modelDrafts).length) return false;
  if (!attachments.saveDrafts()) return false;
  updateAddress(navigationOpen.value ? '' : selectedId.value);
  return true;
}
const appUpdate = useAppUpdate({ enabled: import.meta.env.PROD, canReload: canReloadApp, beforeReload: preserveBeforeReload });
const desktopUpgrade = useDesktopUpgrade(result => {
  if (result.failed) error.value = result.message;
  else notice.value = result.message;
  resumeStream();
  if (result.updated) void appUpdate.check();
});
const visibleNotice = computed(() => notice.value || (desktopUpgrade.reconnecting.value
  ? '正在等待升级后的连接…' : desktopUpgrade.upgrading.value ? desktopUpgrade.progress.value?.message || '电脑正在后台升级…' : ''));
const visibleConnectionError = computed(() => !selectedId.value || streamStatus.value === 'connected' || desktopUpgrade.reconnecting.value ? '' : connectionError.value);
const projectError = computed(() => catalogError.value || visibleConnectionError.value);
const directSubagentInputError = 'direct app-server input is not allowed for multi-agent v2 sub-agents';
const subagentInputRejected = computed(() => error.value.includes(directSubagentInputError));
const visibleError = computed(() => visibleConnectionError.value || (subagentInputRejected.value ? '' : error.value));
watch(authenticated, active => { if (!active) desktopUpgrade.stop(); });
async function reloadPage() {
  if (!canReloadApp()) { error.value = '请先完成当前操作，再重新打开页面。'; return; }
  if (!preserveBeforeReload()) { error.value = '草稿暂时无法保存，请先复制草稿。'; return; }
  try {
    const reload = (window as Window & { reloadCodexPage?: () => Promise<void> }).reloadCodexPage;
    if (reload) await reload();
    else location.reload();
  } catch { error.value = '暂时无法重新打开页面，请检查连接后重试。'; }
}
const voiceNotice = computed(() => voice.error.value || ({
  idle: '', starting: '正在启动麦克风…',
  listening: '正在听，点击麦克风结束识别', stopping: '正在完成识别…',
}[voice.phase.value]));
const voiceButtonLabel = computed(() => ({
  idle: '语音输入', starting: '取消语音启动', listening: '结束语音输入', stopping: '取消语音等待',
}[voice.phase.value]));
const hasMessage = computed(() => !!draft.value.trim() || !!draftImages.value.length);
const composerExpanded = computed(() => hasMessage.value || !!voiceNotice.value || composerMenu.value);
const currentMode = computed(() => selectedThread.value?.goal && selectedThread.value.goal.status !== 'complete'
  ? 'goal' : selectedThread.value?.mode === 'plan' ? 'plan' : 'default');
const goalSummary = computed(() => selectedThread.value?.goal || selectedThread.value?.completedGoal);

watch(selectedId, (id) => {
  if (id) void Promise.all([import('./features/conversation/TurnView.vue'), import('./features/conversation/ItemView.vue'), preloadMarkdownPreview()]).catch(() => {});
});
watch(() => interactionState.value.expanded, (expanded) => {
  if (expanded) { composerMenu.value = false; voice.clear(); composer.value?.blur(); }
  else void nextTick(resizeComposer);
});

function openGoal() { if (capabilities.value.goals) { composerMenu.value = false; goalOpen.value = true; } }
function applyGoal(value: GoalState & { threadId: string }) {
  if (selectedThread.value?.id === value.threadId) updateThread({ ...selectedThread.value,
    goal: value.goal, completedGoal: value.completedGoal ?? selectedThread.value.completedGoal });
}
async function changeMode(mode: 'default' | 'plan') {
  if (!capabilities.value.settings) { error.value = '手机模式修改尚未开放，请在官方 Codex 修改。'; return; }
  if (modeBusy.value || !selectedId.value || streamStatus.value !== 'connected') return;
  const thread = selectedId.value;
  modeBusy.value = true;
  try {
    const result = await request<{ applied: boolean; mode: string }>(threadUrl(thread, 'settings'), { method: 'POST', body: JSON.stringify({ mode }) });
    if (!result.applied || result.mode !== mode) throw new Error('模式尚未确认，请重新读取聊天。');
    if (selectedThread.value?.id === thread) updateThread({ ...selectedThread.value, mode });
  }
  catch (failure) { if (selectedId.value === thread) error.value = (failure as Error).message; }
  finally { modeBusy.value = false; }
}

async function createGoal(input: GoalInput, onCreated?: (threadId: string) => void): Promise<void> {
  if (!capabilities.value.goals) throw new Error('请在官方 Codex 创建和设置目标；当前草稿保留。');
  const reason = submissionReason(true);
  if (reason) throw new Error(reason);
  if (!newChat.value) throw new Error('请先打开需要设置目标的聊天。');
  const target = draftKey.value;
  const originalDraft = draft.value;
  const prompt = composeDraftContext(draftParts.value.contexts, `请准备完成以下目标，等我设置目标与预算后再开始：\n${input.objective}`);
  const images = attachments.readyImages().map(image => image.id);
  const selection = modelSelection.value;
  const { result, requestId } = await submissions.submit<{ threadId: string; firstTurnAccepted: boolean }>({
    target, kind: 'create', url: '/api/threads',
    input: { sourceThreadId: creationSource.value, projectId: selectedProjectId.value, prompt, images, ...selection },
  });
  if (draftKey.value === target && newChat.value) {
    onCreated?.(result.threadId);
    const operation = recoveredCreations.value.find(value => value.id === requestId);
    if (operation) restoreCreation(operation);
    goalOpen.value = true;
  }
  if (selectedId.value !== result.threadId) throw new Error('聊天已创建，请打开原聊天后设置目标；当前页面不会被覆盖。');
  if (firstTurnPending.value || !capabilities.value.goals) {
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => { stop(); reject(new Error('聊天已创建，首轮或目标连接仍在确认。请在此聊天重试设置目标。')); }, 60_000);
      const stop = watch([selectedId, firstTurnPending, capabilities, authenticated], () => {
        if (selectedId.value !== result.threadId || !authenticated.value) {
          clearTimeout(timeout); stop(); reject(new Error('已离开原聊天，目标尚未设置。'));
        } else if (!firstTurnPending.value && capabilities.value.goals) {
          clearTimeout(timeout); stop(); resolve();
        }
      });
    });
  }
  const state = await request<GoalState>(threadUrl(result.threadId, 'goal'), { method: 'PATCH', body: JSON.stringify({ ...input, status: 'active' }) });
  applyGoal({ threadId: result.threadId, ...state });
  if (drafts[target] === originalDraft) drafts[target] = '';
  attachments.clear(target, images);
  if (modelDrafts[target] === selection) delete modelDrafts[target];
}

function managedThread(result: { threadId: string; title?: string; pinned?: boolean; archived?: boolean; deleted?: boolean }) {
  if (result.deleted || result.archived) removeCatalogThread(result.threadId);
  const current = selectedThread.value;
  if (current?.id === result.threadId) {
    if (result.deleted || result.archived) {
      openProjects();
      selectedId.value = '';
      selectedThread.value = null;
      updateAddress('');
    } else updateThread({ ...current, ...(result.title !== undefined ? { title: result.title } : {}),
      ...(result.pinned !== undefined ? { pinnedIndex: result.pinned ? 1 : undefined } : {}),
      ...(result.archived !== undefined ? { archived: result.archived } : {}) });
  }
  void refreshCatalog();
}

function restoreComposerFocus() {
  if (view.value === 'chat' && !interactionState.value.expanded) {
    composer.value?.focus();
    void nextTick(resizeComposer);
  }
}

function openSettings(event: MouseEvent, trigger = event.currentTarget as HTMLButtonElement) {
  // 菜单项打开设置后会隐藏，让面板在退出时回到仍然可见的入口。
  trigger.focus({ preventScroll: true });
  chatMenu.value = false;
  settingsOpen.value = true;
}

function toggleComposerMenu() {
  composerMenu.value = !composerMenu.value;
  chatMenu.value = false;
  if (composerMenu.value && document.activeElement === document.body) composerPlus.value?.focus({ preventScroll: true });
}

function toggleVoice() {
  if (voice.busy.value) voice.stop();
  else {
    composer.value?.blur();
    if (!voice.start()) composer.value?.focus();
  }
}

function onPageVisibility() {
  if (document.hidden) voice.clear();
  else retryDesktopConnection();
}

function retryDesktopConnection() {
  if (!document.hidden && authenticated.value && (!operationsReady.value || unresolvedOperations.value.length)) void loadOperations();
  if (!document.hidden && props.desktopName && !authenticated.value && !booting.value && loginError.value
    && loginFailureStatus.value !== 401) void initializeConnection();
}

function expireConnection() {
  submissions.invalidate();
  authenticated.value = false;
  conversationHistory.clear();
  clearConversation();
  loginFailureStatus.value = 401;
  loginError.value = props.desktopName ? '配对已失效，请重新扫描这台电脑的云连接二维码。' : '连接已过期，请重新扫码或输入连接密钥。';
}

function preventPageZoom(event: Event) { event.preventDefault(); }

function dismissChatError() {
  if (visibleConnectionError.value) connectionError.value = '';
  else error.value = '';
}

function readDrafts(): Record<string, string> {
  try {
    const saved = JSON.parse(sessionStorage.getItem(desktopStorageKey('codex-mobile-drafts')) || '{}');
    return Object.fromEntries(Object.entries(saved).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
  } catch { return {}; }
}
function saveDrafts() {
  try { sessionStorage.setItem(desktopStorageKey('codex-mobile-drafts'), JSON.stringify(drafts)); return true; }
  catch { return false; }
}
watch(drafts, saveDrafts, { deep: true, flush: 'sync' });
function readModelDrafts(): Record<string, MessageModel> {
  try {
    const saved = JSON.parse(sessionStorage.getItem(desktopStorageKey('codex-mobile-model-drafts')) || '{}');
    return Object.fromEntries(Object.entries(saved).flatMap(([key, value]) => {
      if (!value || typeof value !== 'object') return [];
      const { model, thinking } = value as MessageModel;
      const selection = { ...(typeof model === 'string' && model ? { model } : {}), ...(typeof thinking === 'string' && thinking ? { thinking } : {}) };
      return Object.keys(selection).length ? [[key, selection]] : [];
    }));
  } catch { return {}; }
}
function saveModelDrafts() {
  try { sessionStorage.setItem(desktopStorageKey('codex-mobile-model-drafts'), JSON.stringify(modelDrafts)); return true; }
  catch { return false; }
}
watch(modelDrafts, saveModelDrafts, { deep: true, flush: 'sync' });
function threadProjectId(thread: DesktopThread) {
  return projects.value.find((project) => project.projectId === thread.projectId)?.projectId || 'other';
}
const filteredProjects = computed(() => {
  const query = search.value.trim().toLowerCase();
  return projects.value.filter((project) => !query || (project.label + ' ' + project.path).toLowerCase().includes(query)
    || knownThreads.value.some((thread) => thread.projectId === project.projectId && thread.title.toLowerCase().includes(query)));
});
const filteredThreads = computed(() => {
  const query = search.value.trim().toLowerCase();
  return threads.value.filter((thread) => !query || thread.title.toLowerCase().includes(query));
});
const workThreads = computed(() => recentThreads(filteredThreads.value, 5));
const pinnedThreads = computed(() => filteredThreads.value
  .filter(thread => thread.pinnedIndex !== undefined)
  .sort((a, b) => a.pinnedIndex! - b.pinnedIndex!));
function threadProjectLabel(thread: DesktopThread) {
  return projects.value.find((project) => project.projectId === threadProjectId(thread))?.label || workspaceLabel(thread.cwd);
}
function leaveWorkThread(element: Element) {
  if (element.contains(document.activeElement)) projectsToggle.value?.focus({ preventScroll: true });
  (element as HTMLElement).inert = true;
}
function threadsInProject(project: Project) {
  const query = search.value.trim().toLowerCase();
  if (!query || (project.label + ' ' + project.path).toLowerCase().includes(query)) return latestByProject.value.get(project.projectId) || [];
  return recentThreads(knownThreads.value.filter(thread => thread.projectId === project.projectId && thread.title.toLowerCase().includes(query)), projectPreviewCount);
}
function projectCollapsed(project: Project) {
  return !!(search.value.trim() ? searchCollapsedProjects : collapsedProjects)[project.projectId];
}
function toggleProject(project: Project) {
  const collapsed = search.value.trim() ? searchCollapsedProjects : collapsedProjects;
  collapsed[project.projectId] = !collapsed[project.projectId];
  if (!collapsed[project.projectId]) projectHistoryError.value = false;
}

const projectElements = new Map<string, HTMLElement>();
const visibleProjects = reactive(new Set<string>());
let projectObserver: IntersectionObserver | undefined;

function observeProject(projectId: string, target: unknown) {
  const element = target instanceof HTMLElement ? target : undefined;
  const previous = projectElements.get(projectId);
  if (element === previous) return;
  if (previous) projectObserver?.unobserve(previous);
  visibleProjects.delete(projectId);
  if (!element) { projectElements.delete(projectId); return; }
  projectElements.set(projectId, element);
  projectObserver?.observe(element);
}

watch(navigationScroll, (element, _, cleanup) => {
  if (!element) return;
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      const id = (entry.target as HTMLElement).dataset.projectId!;
      if (projectElements.get(id) !== entry.target) continue;
      if (entry.isIntersecting) visibleProjects.add(id);
      else visibleProjects.delete(id);
    }
  }, { root: element });
  projectObserver = observer;
  for (const group of projectElements.values()) observer.observe(group);
  cleanup(() => {
    observer.disconnect();
    projectObserver = undefined;
    visibleProjects.clear();
  });
}, { flush: 'post' });

watchEffect(cleanup => {
  if (view.value !== 'projects' || catalogScope.value !== undefined || !projectsOpen.value || search.value.trim()
    || !hasMoreProjectHistory.value || loadingProjectHistory.value || projectHistoryError.value) return;
  const previews = latestByProject.value;
  if (!filteredProjects.value.some(project => visibleProjects.has(project.projectId) && !projectCollapsed(project)
    && (previews.get(project.projectId)?.length || 0) < projectPreviewCount)) return;
  // 每次只补一页，给滚动和项目展开状态留出更新机会。
  const timer = setTimeout(() => { void loadProjectHistory(); }, 120);
  cleanup(() => clearTimeout(timer));
});

function browseThreads(project: Project | null = null, archived = false) {
  if (document.activeElement instanceof HTMLButtonElement) catalogTrigger = document.activeElement;
  searchInput.value?.blur();
  catalogScope.value = project;
  archivedCatalog.value = archived;
}
function toggleSearch() {
  searchOpen.value = !searchOpen.value;
  if (searchOpen.value) void nextTick(() => searchInput.value?.focus());
  else {
    searchInput.value?.blur();
    search.value = '';
    void nextTick(() => searchToggle.value?.focus({ preventScroll: true }));
  }
}
function toggleProjects() {
  if (search.value.trim()) searchProjectsExpanded.value = !searchProjectsExpanded.value;
  else projectsExpanded.value = !projectsExpanded.value;
}
const active = computed(() => turns.value.some((turn) => turn.status === 'inProgress'));
const resumableTurn = computed(() => {
  if (!capabilities.value.resume || newChat.value || !selectedId.value || active.value) return;
  const turn = turns.value.at(-1);
  return turn && (turn.status === 'failed' || (turn.status === 'interrupted' && (turn.error || capabilities.value.resumeInterrupted))) ? turn : undefined;
});
function submissionReason(hasContent = hasMessage.value || !!resumableTurn.value) {
  if (resuming.value) return '正在继续上一任务，请等待回执。';
  return submissions.readiness({
    target: draftKey.value, capable: !!(newChat.value ? capabilities.value.create : capabilities.value.send),
    connected: newChat.value ? authenticated.value : streamStatus.value === 'connected',
    hasContent, uploading: imagesUploading.value, imageErrors: imageErrors.value,
    imageCount: draftImages.value.length, imageSupported: !!capabilities.value.images, voiceBusy: voice.busy.value,
    firstTurnPending: !newChat.value && firstTurnPending.value,
    missingSource: newChat.value && (!creationSource.value || !selectedProjectId.value),
  });
}
const sendDisabledReason = computed(() => !hasMessage.value && resumableTurn.value && !capabilities.value.resume
  ? '请在电脑上的 Codex 继续任务。' : submissionReason());
const composerAction = computed(() => active.value && !hasMessage.value
  ? { kind: 'stop', busy: stopping.value, disabled: !capabilities.value.interrupt || stopping.value || streamStatus.value !== 'connected', label: capabilities.value.interrupt ? '停止任务' : '请在桌面停止' }
  : { kind: 'send', busy: sending.value, disabled: !!sendDisabledReason.value,
    label: sending.value ? '发送中' : !hasMessage.value && resumableTurn.value ? '继续任务' : newChat.value ? '创建对话并发送' : capabilities.value.native && !capabilities.value.queue && active.value ? '发送追加消息' : capabilities.value.queue && (active.value || queueState.value?.messages.length) ? '加入队列' : '发送消息' });
const delegationTargets = computed(() => subagentTargets(turns.value.flatMap((turn) => turn.items)));
const visiblePending = computed(() => pendingMessages.value.filter((message) => message.threadId === selectedId.value));
function workspaceLabel(cwd?: string) {
  return cwd?.replace(/^\\\\\?\\/, '').replace(/[\\/]+$/, '').split(/[\\/]/).at(-1) || '本地聊天';
}

async function disconnectBrowser() {
  if (disconnecting.value) return;
  disconnecting.value = true;
  disconnectError.value = '';
  voice.clear();
  try {
    await request('/api/logout', { method: 'POST', body: '{}' });
    submissions.invalidate();
    conversationHistory.clear();
    attachments.clearAll();
    clearConversation();
    threads.value = [];
    settingsOpen.value = false;
    pendingMessages.value = [];
    loginKey.value = '';
    for (const id of Object.keys(drafts)) delete drafts[id];
    for (const id of Object.keys(modelDrafts)) delete modelDrafts[id];
    authenticated.value = false;
  } catch (failure) {
    disconnectError.value = (failure as Error).message;
    await nextTick();
    disconnectFailure.value?.scrollIntoView({ block: 'nearest' });
  }
  finally { disconnecting.value = false; }
}

function updateAddress(threadId = '', projectId = selectedProjectId.value) {
  const url = new URL(location.href);
  setConversationAddress(url, threadId, projectId, newChat.value && !navigationOpen.value);
  history.replaceState(history.state, '', url.href);
}

function clearConversation() {
  usageOpen.value = false;
  if (!selectedId.value) voice.clear();
  selectedId.value = '';
  selectedThread.value = null;
  turns.value = [];
  historyCursor.value = null;
  changedFileCount.value = undefined;
  newChat.value = false;
  error.value = '';
  connectionError.value = '';
  composerMenu.value = false;
  chatMenu.value = false;
  delegationOpen.value = false;
  contextOpen.value = false;
  interactions.value = [];
  interactionState.value = { expanded: false, pending: false };
  notice.value = '';
}

function openProjects() {
  usageOpen.value = false;
  composer.value?.blur();
  composerMenu.value = false;
  chatMenu.value = false;
  navigationOpen.value = true;
  updateAddress();
}

function openUsage(forThread = false) {
  usageThreadId.value = forThread ? selectedId.value : '';
  usageThreadTitle.value = forThread ? selectedThread.value?.title || '' : '';
  usageReturnToChat.value = forThread;
  usagePresented.value = true;
  usageStart = 0;
  navigationTarget = -1;
  composer.value?.blur();
  composerMenu.value = false;
  chatMenu.value = false;
  navigationOpen.value = true;
  usageOpen.value = true;
  updateAddress();
}

function closeUsage() {
  if (usagePanel.value?.back()) { void nextTick(() => alignNavigation()); return; }
  if (!usageReturnToChat.value) { openProjects(); return; }
  usageOpen.value = false;
  navigationOpen.value = !selectedId.value && !newChat.value;
  updateAddress(selectedId.value);
}

function backToPrevious() {
  if (!conversationHistory.back()) openProjects();
}

function closeCatalog() {
  catalogScope.value = undefined;
  const trigger = catalogTrigger;
  catalogTrigger = undefined;
  void nextTick(() => { if (catalogScope.value === undefined && trigger?.isConnected) trigger.focus({ preventScroll: true }); });
}

async function activateConversation(threadId: string, projectId: string, saved?: ConversationFrame, thread?: DesktopThread) {
  if (!authenticated.value) return;
  if (!threadId) {
    clearConversation();
    selectedProjectId.value = projectId;
    openProjects();
    return;
  }
  const enteredFromList = view.value !== 'chat';
  const switching = selectedId.value !== threadId;
  usageOpen.value = false;
  newChat.value = false;
  selectedProjectId.value = projectId;
  restoringFrame = saved;
  selectedId.value = threadId;
  restoringFrame = undefined;
  selectedThread.value ||= thread || null;
  navigationOpen.value = false;
  updateAddress(threadId);
  await nextTick();
  if (selectedId.value !== threadId) return;
  if (saved && switching) {
    if (scroller.value) scroller.value.scrollTop = saved.stickToBottom ? scroller.value.scrollHeight : saved.scrollTop;
  }
  // 父子对话都属于聊天视图，返回后仍需把横向页面对齐，避免可见区域继续 inert。
  if (!enteredFromList) alignNavigation();
}

let navigationStart = 0;
let chatStart = 0;
let usageStart = 0;
let navigationTarget: number | undefined;
function measureNavigation() {
  const element = shell.value;
  if (!element) return;
  const leftInset = Number.parseFloat(getComputedStyle(element).paddingLeft) || 0;
  const navigation = element.querySelector<HTMLElement>('.navigation-pane');
  const chat = element.querySelector<HTMLElement>('.chat-pane');
  const usage = element.querySelector<HTMLElement>('.usage-pane');
  if (navigation) navigationStart = navigation.offsetLeft - leftInset;
  if (chat) chatStart = chat.offsetLeft - leftInset;
  if (usage) usageStart = usage.offsetLeft - leftInset;
}

function alignNavigation(animate = false) {
  const element = shell.value;
  if (!element) return;
  const target = () => view.value === 'usage' ? usageStart : view.value === 'chat' ? chatStart : navigationStart;
  const measured = view.value === 'usage' ? usageStart > 0 : view.value === 'chat' ? chatStart > navigationStart : !!element;
  // A native swipe already reached its destination. Do not restart it or force layout.
  if (measured && Math.abs(element.scrollLeft - target()) <= 2) { navigationTarget = undefined; finishUsageExit(); return; }
  measureNavigation();
  if (Math.abs(element.scrollLeft - target()) <= 2) { navigationTarget = undefined; finishUsageExit(); return; }
  navigationTarget = target();
  element.scrollTo({
    left: target(),
    behavior: animate && !matchMedia('(prefers-reduced-motion: reduce)').matches ? 'smooth' : 'instant',
  });
}

let navigationTimer: ReturnType<typeof setTimeout> | undefined;
let navigationTouching = false;
let backSwipe: { shell: HTMLElement; view: typeof view.value; x: number; y: number; horizontal: boolean } | undefined;

function cancelBackSwipe() {
  backSwipe?.shell.removeEventListener('touchmove', onBackSwipeMove);
  backSwipe = undefined;
}

function onBackSwipeStart(event: TouchEvent) {
  navigationTouching = true;
  clearTimeout(navigationTimer);
  cancelBackSwipe();
  const element = shell.value;
  if (!element || event.touches.length !== 1 || !(event.target instanceof Element)) return;
  if (event.target.closest('dialog, .composer-region, input, textarea, [contenteditable]')) return;
  // 同页的子层没有独立吸附位置，只在左缘接管返回手势。
  if (!(view.value === 'projects' && catalogScope.value !== undefined
    || view.value === 'chat' && conversationHistory.canGoBack.value
    || view.value === 'usage' && usagePanel.value?.hasParent)) return;
  const { clientX: x, clientY: y } = event.touches[0]!;
  if (x - element.getBoundingClientRect().left > 32) return;
  backSwipe = { shell: element, view: view.value, x, y, horizontal: false };
  element.addEventListener('touchmove', onBackSwipeMove, { passive: false });
}

function onBackSwipeMove(event: TouchEvent) {
  const swipe = backSwipe;
  if (!swipe) return;
  if (event.touches.length !== 1 || swipe.view !== view.value) { cancelBackSwipe(); return; }
  const { clientX, clientY } = event.touches[0]!;
  const x = clientX - swipe.x;
  const y = Math.abs(clientY - swipe.y);
  if (!swipe.horizontal) {
    if (x > 5 && x > y * 1.5) swipe.horizontal = true;
    else if (x < -5 || y > 8) cancelBackSwipe();
  }
  if (swipe.horizontal && event.cancelable) event.preventDefault();
}

function onBackSwipeEnd(event: TouchEvent) {
  const swipe = backSwipe;
  onNavigationTouchEnd(event);
  if (!swipe?.horizontal || swipe.view !== view.value) return;
  if (event.cancelable) event.preventDefault();
  const touch = event.changedTouches[0];
  if (!touch || touch.clientX - swipe.x < Math.min(84, swipe.shell.clientWidth * .22)) return;
  if (swipe.view === 'chat') backToPrevious();
  else if (swipe.view === 'usage') closeUsage();
  else closeCatalog();
}

function onNavigationTouchEnd(event: TouchEvent) {
  navigationTouching = event.touches.length > 0;
  cancelBackSwipe();
  onNavigationScroll();
}

function finishUsageExit() {
  if (!usageOpen.value && usagePresented.value) {
    // Cancel an interrupted opening scroll before its destination is removed.
    shell.value?.scrollTo({ left: view.value === 'chat' ? chatStart : navigationStart, behavior: 'instant' });
    usagePresented.value = false;
    void nextTick(() => { measureNavigation(); alignNavigation(); });
  }
}

function onNavigationPointerDown(event: PointerEvent) {
  // A deliberate swipe can interrupt a programmatic transition.
  if (event.pointerType === 'touch') navigationTarget = undefined;
}

function onNavigationScrollEnd() {
  clearTimeout(navigationTimer);
  if (!shell.value) return;
  const position = shell.value.scrollLeft;
  if (navigationTarget !== undefined) {
    if (Math.abs(position - navigationTarget) > 2) return;
    navigationTarget = undefined;
  }
  if (usageOpen.value) {
    const origin = usageReturnToChat.value ? chatStart : navigationStart;
    if (Math.abs(position - origin) <= 2) closeUsage();
    return;
  }
  if (Math.abs(position - navigationStart) <= 2) {
    if (!navigationOpen.value) backToPrevious();
    else finishUsageExit();
  } else if ((selectedId.value || newChat.value) && Math.abs(position - chatStart) <= 2 && view.value !== 'chat') {
    if (catalogScope.value !== undefined) { alignNavigation(); return; }
    usageOpen.value = false;
    navigationOpen.value = false;
    updateAddress(selectedId.value);
  } else if (view.value === 'chat' && Math.abs(position - chatStart) <= 2) finishUsageExit();
}

function onNavigationScroll() {
  clearTimeout(navigationTimer);
  // Native scrollend waits for both the gesture and snap to settle. Only browsers
  // without it use an idle timer; running both can commit a swipe during a finger pause.
  if (shell.value && !navigationTouching && !('onscrollend' in shell.value)) {
    navigationTimer = setTimeout(onNavigationScrollEnd, 180);
  }
}

watch([view, authenticated], ([current, connected], [previous, wasConnected]) => {
  if (current !== 'chat' || !connected) voice.clear();
  const betweenChatAndList = !!selectedId.value && (previous === 'projects' && current === 'chat'
    || previous === 'chat' && current === 'projects');
  void nextTick(() => alignNavigation(connected && wasConnected && !betweenChatAndList));
});

function openProject(projectId: string) {
  usageOpen.value = false;
  selectedProjectId.value = projectId;
  navigationOpen.value = true;
  updateAddress();
}

function openThread(thread: DesktopThread) {
  const saved = conversationHistory.reset(thread.id);
  void activateConversation(thread.id, threadProjectId(thread), saved, thread);
}

function openNotifiedThread(threadId: string) {
  settingsOpen.value = false;
  updateSettingsOpen.value = false;
  goalOpen.value = false;
  projectPickerOpen.value = false;
  scanOpen.value = false;
  openThread(knownThreads.value.find(thread => thread.id === threadId) || { id: threadId, title: '' });
}

function startNewChat(projectId = '') {
  if (!capabilities.value.create) { notice.value = '请先在桌面新建聊天。'; return; }
  const project = creationProjects.value.find((entry) => entry.projectId === projectId);
  if (!project || !creationSource.value) {
    creationSource.value = selectedId.value;
    creationProjects.value = [];
    creationError.value = '';
    projectPickerOpen.value = true;
    if (creationSource.value) void chooseCreationSource(creationSource.value);
    return;
  }
  const fromPicker = projectPickerOpen.value;
  conversationHistory.reset();
  clearConversation();
  selectedProjectId.value = project.projectId;
  projectPickerOpen.value = false;
  newChat.value = true;
  navigationOpen.value = false;
  updateAddress();
  void nextTick(() => {
    resizeComposer();
    if (!fromPicker) composer.value?.focus();
  });
}

async function chooseCreationSource(source: string) {
  if (creationLoading.value) return;
  creationSource.value = source;
  creationLoading.value = true;
  creationError.value = '';
  try {
    const result = await request<{ projects: Project[] }>('/api/projects?threadId=' + encodeURIComponent(source));
    if (creationSource.value === source) creationProjects.value = result.projects;
  } catch (failure) { if (creationSource.value === source) creationError.value = (failure as Error).message; }
  finally { creationLoading.value = false; }
}

let bottomScroll: Promise<void> | undefined;
let bottomScrollKey = '';
function scrollBottom(force = false) {
  if (force) stickToBottom.value = true;
  const key = draftKey.value;
  if (bottomScroll && bottomScrollKey === key) return bottomScroll;
  bottomScrollKey = key;
  const pending = nextTick(() => {
    if (bottomScroll !== pending) return;
    bottomScroll = undefined;
    if (draftKey.value !== key || !scroller.value) return;
    if (stickToBottom.value) {
      scroller.value.scrollTop = scroller.value.scrollHeight;
      newOutput.value = false;
    } else newOutput.value = true;
  });
  return bottomScroll = pending;
}

function onScroll() {
  const element = scroller.value;
  if (!element) return;
  stickToBottom.value = element.scrollHeight - element.scrollTop - element.clientHeight < 100;
  newOutput.value = !stickToBottom.value;
}

function updateThread(thread: DesktopThread) {
  selectedThread.value = { ...selectedThread.value, ...thread };
  if (thread.projectId !== undefined) selectedProjectId.value = threadProjectId(selectedThread.value);
  updateCatalogThread(thread);
}

function mergeTurns(earlier: Turn[], later: Turn[]) {
  const values = new Map(earlier.map(turn => [turn.id, turn]));
  const order = [...values.keys()];
  let position = order.length;
  // 补页可能跨过缓存边界，沿相邻轮次插入，不能把缺口直接追加到末尾。
  for (const turn of [...later].reverse()) {
    const existing = order.indexOf(turn.id);
    if (existing >= 0) position = existing;
    else order.splice(position, 0, turn.id);
    values.set(turn.id, turn);
  }
  return order.map(id => values.get(id)!);
}

function cancelHistory() {
  historyRequest?.abort();
  historyRequest = undefined;
  historyGap = undefined;
  loadingHistory.value = false;
  historyError.value = '';
}

function reconcilePending() {
  if (!pendingMessages.value.some(message => message.threadId === selectedId.value)) return;
  const acknowledged = new Set<string>();
  for (const turn of turns.value) {
    for (const item of turn.items) {
      if (item.type !== 'userMessage') continue;
      acknowledged.add(item.id);
      if (item.clientId) acknowledged.add(item.clientId);
    }
  }
  pendingMessages.value = pendingMessages.value.filter((message) => message.threadId !== selectedId.value || !acknowledged.has(message.requestId));
}

function applySnapshot(snapshot: StreamSnapshot) {
  if (snapshot.nextCursor !== undefined) {
    if (!turns.value.length) historyCursor.value = snapshot.nextCursor;
    else if (snapshot.nextCursor && snapshot.turns.length
      && !snapshot.turns.some(turn => turns.value.some(entry => entry.id === turn.id))) {
      historyRequest?.abort();
      historyRequest = undefined;
      historyGap = { olderId: historyGap?.olderId || turns.value.at(-1)!.id,
        newerId: snapshot.turns[0]!.id, cursor: snapshot.nextCursor };
    }
  }
  turns.value = mergeTurns(turns.value, snapshot.turns);
  updateThread(snapshot.thread);
  if (snapshot.changedFileCount !== undefined) changedFileCount.value = snapshot.changedFileCount;
  interactions.value = snapshot.interactions || [];
  capabilities.value = snapshot.capabilities || {};
  if (snapshot.firstTurnPending !== undefined) firstTurnPending.value = snapshot.firstTurnPending;
  if (snapshot.firstTurnPending === false) {
    for (const operation of submissions.recoveredCreations.value.filter(value => value.receipt?.threadId === snapshot.thread.id)) {
      submissions.acknowledgeCreation(operation);
    }
  }
  if (snapshot.firstTurnPending === false && createdDraft.value?.threadId === snapshot.thread.id) {
    const sent = createdDraft.value;
    if (drafts[sent.key] === sent.prompt) drafts[sent.key] = '';
    attachments.clear(sent.key, sent.images);
    if (modelDrafts[sent.key] === sent.selection) delete modelDrafts[sent.key];
    createdDraft.value = null;
  }
  if (snapshot.queue) messageQueue.acceptSnapshot(snapshot.queue, snapshot.thread.id);
  reconcilePending();
  void scrollBottom();
  if (historyGap && !historyRequest) void loadOlder();
}

function applyUpdate(update: StreamUpdate) {
  turns.value = mergeStreamUpdate(turns.value, update);
  if (!update.turns) {
    void scrollBottom();
    return;
  }
  updateThread(update.thread);
  if (update.changedFileCount !== undefined) changedFileCount.value = update.changedFileCount;
  if (update.interactions !== undefined) interactions.value = update.interactions;
  reconcilePending();
  if (update.updates.length || update.textAppends?.length) void scrollBottom();
}

watch(selectedId, (threadId) => {
  cancelHistory();
  firstTurnPending.value = submissions.recoveredCreations.value.some(operation => operation.receipt?.threadId === threadId && !operation.receipt.firstTurnAccepted && operation.receipt.firstTurnPending !== false);
  voice.clear();
  interactions.value = [];
  delegationOpen.value = false;
  contextOpen.value = false;
  chatMenu.value = false;
  notice.value = '';
  if (!threadId) return;
  composerMenu.value = false;
  selectedThread.value = restoringFrame?.thread || knownThreads.value.find(thread => thread.id === threadId) || null;
  turns.value = restoringFrame?.turns || [];
  error.value = '';
  connectionError.value = '';
  historyCursor.value = restoringFrame?.historyCursor ?? null;
  changedFileCount.value = restoringFrame?.changedFileCount;
  stickToBottom.value = restoringFrame?.stickToBottom ?? true;
  newOutput.value = restoringFrame?.newOutput ?? false;
}, { flush: 'sync' });

async function loadOlder() {
  if ((!historyGap && !historyCursor.value) || historyRequest) return;
  const threadId = selectedId.value;
  const before = scroller.value?.scrollHeight || 0;
  const gap = historyGap;
  const controller = new AbortController();
  historyRequest = controller;
  const relevant = () => !controller.signal.aborted && historyRequest === controller && selectedId.value === threadId;
  loadingHistory.value = true;
  historyError.value = '';
  try {
    do {
      const cursor = gap?.cursor || historyCursor.value!;
      const page = await request<HistoryPage>(threadUrl(threadId, 'history') + '?cursor=' + encodeURIComponent(cursor), { signal: controller.signal });
      if (!relevant()) return;
      if (gap) {
        const boundary = page.turns.findIndex(turn => turn.id === gap.olderId);
        const additions = boundary >= 0 ? page.turns.slice(boundary) : page.turns;
        const position = turns.value.findIndex(turn => turn.id === gap.newerId);
        turns.value = mergeTurns(mergeTurns(turns.value.slice(0, position), additions), turns.value.slice(position));
        if (boundary >= 0 || !page.nextCursor) historyGap = undefined;
        else { gap.cursor = page.nextCursor; gap.newerId = page.turns[0]!.id; }
      } else {
        turns.value = mergeTurns(page.turns, turns.value);
        historyCursor.value = page.nextCursor;
      }
      reconcilePending();
      await nextTick();
      if (!relevant()) return;
      if (gap) void scrollBottom();
      else if (scroller.value) scroller.value.scrollTop += scroller.value.scrollHeight - before;
    } while (gap && historyGap === gap);
  } catch (failure) { if (relevant()) historyError.value = (failure as Error).message; }
  finally { if (historyRequest === controller) { historyRequest = undefined; loadingHistory.value = false; } }
}

function resizeComposer() {
  const field = composer.value?.element;
  if (!field || view.value !== 'chat') return;
  const limit = Number.parseFloat(getComputedStyle(field).maxHeight) || Infinity;
  const scrollTop = field.scrollTop;
  // 临时限制可视高度，读出内容实际需要的高度，保留正在过渡的 height。
  field.style.maxHeight = '0px';
  const contentHeight = field.scrollHeight;
  field.style.maxHeight = '';
  const height = Math.min(contentHeight, limit) + 'px';
  if (field.style.height !== height) field.style.height = height;
  if (document.activeElement === field && contentHeight > field.clientHeight
    && composer.value?.selectionStart() === draftText.value.length) field.scrollTop = contentHeight;
  else if (field.scrollTop !== scrollTop) field.scrollTop = scrollTop;
}

watch([draftKey, draftText, view, () => interactionState.value.expanded], () => { void nextTick(resizeComposer); });

watch([scroller, selectedId], ([element], _, cleanup) => {
  const content = element?.firstElementChild;
  if (!content) return;
  // Markdown and images may finish layout after the Vue update that requested
  // scrolling. Follow their actual height, while preserving an older reading spot.
  let frame = 0;
  const observer = new ResizeObserver(() => {
    cancelAnimationFrame(frame);
    if (stickToBottom.value) frame = requestAnimationFrame(() => { if (stickToBottom.value) void scrollBottom(); });
  });
  observer.observe(content);
  cleanup(() => { cancelAnimationFrame(frame); observer.disconnect(); });
}, { flush: 'post' });

watch([chatPane, chatTopbar, composerRegion], ([pane, topbar, region], _, cleanup) => {
  if (!pane) return;
  let frame = 0;
  let previous = '';
  const measure = () => {
    const top = Math.ceil(topbar?.getBoundingClientRect().height || 0);
    const bottom = Math.ceil(region?.getBoundingClientRect().height || 0);
    const height = pane.clientHeight;
    latestInHeader.value = height - top - bottom < 60;
    if (previous === top + ':' + bottom + ':' + height) return;
    previous = top + ':' + bottom + ':' + height;
    pane.style.setProperty('--chat-top', top + 'px');
    if (stickToBottom.value) void scrollBottom();
  };
  const observer = new ResizeObserver(() => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(measure);
  });
  if (topbar) observer.observe(topbar);
  if (region) observer.observe(region);
  observer.observe(pane);
  measure();
  cleanup(() => { cancelAnimationFrame(frame); observer.disconnect(); });
}, { flush: 'post' });

function pickImages() {
  composerMenu.value = false;
  imagePicker.value?.click();
}

function addImages(event: Event) {
  const input = event.target as HTMLInputElement;
  void attachments.add(Array.from(input.files || []));
  input.value = '';
}

function pasteImages(event: ClipboardEvent) {
  const files = Array.from(event.clipboardData?.files || []).filter(file => file.type.startsWith('image/'));
  if (!files.length) return;
  event.preventDefault();
  voice.clear();
  void attachments.add(files);
}

async function send() {
  if (sendDisabledReason.value) { if (hasMessage.value) error.value = sendDisabledReason.value; return; }
  if (!hasMessage.value) {
    if (resumableTurn.value) await resumeTurn(resumableTurn.value.id);
    return;
  }
  if (newChat.value) { await createChat(); return; }
  const mode: MessageMode = capabilities.value.queue && (active.value || queueState.value?.messages.length) ? 'queue' : 'now';
  const prompt = draft.value;
  const threadId = selectedId.value;
  const images = attachments.readyImages();
  const selection = modelSelection.value;
  error.value = '';
  notice.value = '';
  try {
    await sendMessage(prompt, mode, images, selection);
    if (drafts[threadId] === prompt) drafts[threadId] = '';
    attachments.clear(threadId, images.map(image => image.id));
    if (modelDrafts[threadId] === selection) delete modelDrafts[threadId];
    if (selectedId.value === threadId) {
      if (mode === 'queue') notice.value = '已加入待执行';
      await nextTick();
      resizeComposer();
    }
  } catch (failure) { if ((failure as Error).name !== 'AbortError' && selectedId.value === threadId) error.value = (failure as Error).message; }
}

async function sendMessage(prompt: string, mode: MessageMode, images: UploadedImage[] = [], selection: MessageModel = {}) {
  const threadId = selectedId.value;
  const reason = submissionReason(!!prompt.trim() || !!images.length);
  if (reason) throw new Error(reason);
  let requestId = '';
  try {
    const { result } = await submissions.submit<{ queued?: boolean; queue?: QueueSnapshot }>({
      target: threadId, kind: 'message', url: threadUrl(threadId, 'messages'),
      input: { prompt, images: images.map(image => image.id), mode, ...selection },
      started(id) {
        requestId = id;
        if (mode === 'now') {
          pendingMessages.value.push({ threadId, requestId, prompt,
            images: images.map(image => ({ ...image, url: new URL(image.url, location.origin).href })), accepted: false });
          void scrollBottom(true);
        }
      },
    });
    if (result.queue) messageQueue.acceptSnapshot(result.queue, threadId);
    if (result.queued) {
      pendingMessages.value = pendingMessages.value.filter(message => message.requestId !== requestId);
      if (selectedId.value === threadId) notice.value = '已加入待执行';
    }
    const pending = pendingMessages.value.find(message => message.requestId === requestId);
    if (pending) pending.accepted = true;
    if (capabilities.value.native) pendingMessages.value = pendingMessages.value.filter(message => message.requestId !== requestId);
    if (selectedId.value === threadId && mode === 'now') {
      reconcilePending();
      void scrollBottom(true);
    }
  } catch (failure) {
    pendingMessages.value = pendingMessages.value.filter(message => message.requestId !== requestId);
    throw failure;
  }
}

function openDelegation() {
  if (!capabilities.value.send || !capabilities.value.delegation) return;
  composerMenu.value = false;
  delegationSource.value = draft.value;
  delegationError.value = '';
  delegationOpen.value = true;
}

function openContext() {
  if (!capabilities.value.contextReferences && !capabilities.value.catalog) return;
  composerMenu.value = false;
  contextOpen.value = true;
}

function closeContext() {
  contextOpen.value = false;
}

function insertContext(mention: string) {
  const parts = draftParts.value;
  if (!parts.contexts.some((entry) => entry.mention === mention)) {
    draft.value = composeDraftContext([...parts.contexts, { mention }], parts.text);
  }
  closeContext();
}

function removeContext(mention: string) {
  draft.value = composeDraftContext(draftParts.value.contexts.filter((entry) => entry.mention !== mention), draftText.value);
}

function onComposerInput(event: Event) {
  voice.clear();
  const input = event as InputEvent;
  const text = draftText.value;
  const caret = composer.value?.selectionStart() ?? text.length;
  if ((capabilities.value.contextReferences || capabilities.value.catalog) && input.data === '@' && !input.isComposing && /(?:^|\s)@$/.test(text.slice(0, caret))) {
    draftText.value = text.slice(0, caret - 1) + text.slice(caret);
    openContext();
  }
}

function closeDelegation() {
  delegationOpen.value = false;
}

async function delegate(request: DelegationRequest) {
  if (!capabilities.value.delegation) return;
  const reason = submissionReason(!!request.prompt.trim());
  if (reason) { delegationError.value = reason; return; }
  const threadId = selectedId.value;
  const source = delegationSource.value;
  const images = attachments.readyImages(threadId);
  delegationError.value = '';
  notice.value = '';
  try {
    await sendMessage(composeDraftContext(splitDraftContext(source).contexts, request.prompt), request.mode, images);
    if (drafts[threadId] === source) drafts[threadId] = '';
    attachments.clear(threadId, images.map(image => image.id));
    if (selectedId.value === threadId) {
      notice.value = request.mode === 'queue' ? '委派请求已加入待执行' : '已发送委派请求';
      closeDelegation();
      await nextTick();
      resizeComposer();
    }
  } catch (failure) {
    if ((failure as Error).name === 'AbortError' || selectedId.value !== threadId) return;
    const message = (failure as Error).message;
    if (message.includes(directSubagentInputError)) {
      error.value = message;
      closeDelegation();
    } else delegationError.value = message;
  }
}

async function openDesktop() {
  if (!capabilities.value.desktopNavigation) { notice.value = '请直接在官方 Codex 打开此对话。'; return; }
  const threadId = selectedId.value;
  notice.value = '';
  error.value = '';
  try {
    const result = await request<{ ok: boolean }>(threadUrl(threadId, 'open'), { method: 'POST', body: '{}' });
    if (!result.ok) throw new Error('官方桌面尚未确认打开此对话，请在电脑核对。');
    if (selectedId.value === threadId) notice.value = '已在电脑上打开此对话';
  } catch (failure) { if (selectedId.value === threadId) error.value = (failure as Error).message; }
}

async function createChat() {
  const reason = submissionReason();
  if (reason) { if (capabilities.value.create) error.value = reason; return; }
  const target = draftKey.value;
  const prompt = draft.value;
  const images = attachments.readyImages().map(image => image.id);
  const selection = modelSelection.value;
  error.value = '';
  try {
    const { result, requestId } = await submissions.submit<{ threadId: string; firstTurnAccepted: boolean }>({
      target, kind: 'create', url: '/api/threads',
      input: { sourceThreadId: creationSource.value, projectId: selectedProjectId.value, prompt, images, ...selection },
    });
    // 离开了原新建页就保留恢复入口，不让迟到回执抢走当前页面。
    if (draftKey.value !== target || !newChat.value) return;
    createdDraft.value = { threadId: result.threadId, key: target, prompt, images, selection };
    if (result.firstTurnAccepted) {
      if (drafts[target] === prompt) drafts[target] = '';
      attachments.clear(target, images);
      if (modelDrafts[target] === selection) delete modelDrafts[target];
      createdDraft.value = null;
    }
    const operation = recoveredCreations.value.find(value => value.id === requestId);
    if (operation) restoreCreation(operation);
  } catch (failure) { if ((failure as Error).name !== 'AbortError' && draftKey.value === target) error.value = (failure as Error).message; }
}

function restoreCreation(operation: PendingOperation) {
  const result = operation.receipt;
  if (!result?.threadId) return;
  if (result.execution === 'mobile' && !result.firstTurnAccepted && result.firstTurnPending === false) {
    // 创建已经完成；首条消息沿原聊天的操作账本核对，草稿随用户查看结果迁入原聊天。
    const from = operation.target, to = result.threadId;
    if (!drafts[to] && !modelDrafts[to] && attachments.moveDraft(from, to)) {
      if (drafts[from]) drafts[to] = drafts[from];
      if (modelDrafts[from]) modelDrafts[to] = modelDrafts[from];
      delete drafts[from];
      delete modelDrafts[from];
      if (createdDraft.value?.threadId === to) createdDraft.value = null;
    }
  }
  if (result.firstTurnAccepted || result.execution === 'mobile' && result.firstTurnPending === false) submissions.acknowledgeCreation(operation);
  openThread({ id: result.threadId, title: '新对话' });
  firstTurnPending.value = !result.firstTurnAccepted && result.firstTurnPending !== false;
  if (firstTurnPending.value) capabilities.value = { ...capabilities.value, send: false };
}

async function resumeTurn(turnId: string) {
  if (!capabilities.value.resume || resuming.value || streamStatus.value !== 'connected' || resumableTurn.value?.id !== turnId) return;
  const thread = selectedId.value;
  resuming.value = true;
  error.value = '';
  try {
    await request(threadUrl(thread, 'resume'), { method: 'POST', body: JSON.stringify({ turnId }) });
    if (selectedId.value === thread) notice.value = '继续请求已发送，等待任务状态同步。';
  } catch (failure) { if (selectedId.value === thread) error.value = (failure as Error).message; }
  finally { resuming.value = false; }
}

async function stopTurn() {
  if (!capabilities.value.interrupt) { error.value = '手机停止操作尚未开放，请在官方 Codex 停止任务。'; return; }
  const turn = [...turns.value].reverse().find(turn => turn.status === 'inProgress');
  if (!turn || !capabilities.value.interrupt || stopping.value || streamStatus.value !== 'connected') return;
  const thread = selectedId.value;
  stopping.value = true;
  error.value = '';
  try { await request(threadUrl(thread, 'interrupt'), { method: 'POST', body: JSON.stringify({ turnId: turn.id }) }); }
  catch (failure) { if (selectedId.value === thread) error.value = (failure as Error).message; }
  finally { stopping.value = false; }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing && !matchMedia('(pointer: coarse)').matches) {
    event.preventDefault();
    void send();
  }
}

async function login(key = loginKey.value.trim()) {
  if (loggingIn.value) return;
  loggingIn.value = true;
  loginError.value = '';
  loginFailureStatus.value = null;
  try {
    await request('/api/login', { method: 'POST', body: JSON.stringify({ accessKey: key }) });
    await enterApp();
  } catch (failure) {
    loginError.value = (failure as Error).message;
    loginFailureStatus.value = failure instanceof ApiError ? failure.status : null;
  } finally { loggingIn.value = false; }
}

async function enterApp(session = request('/api/session')) {
  const state = await session as { capabilities?: Record<string, boolean> };
  capabilities.value = state?.capabilities || {};
  operationsReady.value = false;
  authenticated.value = true;
  loginKey.value = '';
  loginFailureStatus.value = null;
  const params = conversationParams(new URL(location.href));
  const requested = params.get('thread');
  const project = params.get('project');
  if (requested) {
    selectedProjectId.value = project || '';
    selectedId.value = requested;
    navigationOpen.value = false;
  }
  await Promise.all([loadOperations(), refreshCatalog()]);
  if (!authenticated.value) return;
  if (selectedThread.value) {
    selectedProjectId.value = threadProjectId(selectedThread.value);
    updateAddress(selectedId.value);
  } else if (!requested && project && params.get('new') === '1') startNewChat(project);
  else if (!requested && project && (project === 'other' || projects.value.some((entry) => entry.projectId === project))) openProject(project);
}

async function connectScanned(key: string) {
  scanOpen.value = false;
  loginKey.value = key;
  await initializeConnection();
}

function takeConnectionKey() {
  if (props.desktopName) return false;
  const key = new URLSearchParams(location.hash.slice(1)).get('key');
  if (!key) return false;
  loginKey.value = key;
  history.replaceState(history.state, '', location.pathname + location.search);
  return true;
}

function onConnectionLink() {
  if (takeConnectionKey()) void initializeConnection();
  if (props.desktopName && authenticated.value) {
    const threadId = conversationParams(new URL(location.href)).get('thread');
    if (threadId && threadId !== selectedId.value) openNotifiedThread(threadId);
  }
}

function relativeTime(value?: number) {
  if (!value) return '';
  const timestamp = timestampMs(value);
  const difference = Date.now() - timestamp;
  if (difference < 60_000) return '刚刚';
  if (difference < 3_600_000) return Math.floor(difference / 60_000) + ' 分钟前';
  if (difference < 86_400_000) return Math.floor(difference / 3_600_000) + ' 小时前';
  return new Date(timestamp).toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' });
}

async function initializeConnection(session?: Promise<unknown>) {
  booting.value = true;
  loginError.value = '';
  loginFailureStatus.value = null;
  try {
    if (loginKey.value.trim()) {
      await login();
      return;
    }
    try { await enterApp(session); }
    catch (failure) {
      if (props.desktopName || !(failure instanceof ApiError && failure.status === 401)) {
        loginError.value = (failure as Error).message;
        loginFailureStatus.value = failure instanceof ApiError ? failure.status : null;
      }
    }
  } finally { booting.value = false; }
}

function onViewportResize(widthChanged: boolean) {
  resizeComposer();
  if (widthChanged) {
    clearTimeout(navigationTimer);
    measureNavigation();
    alignNavigation();
  }
  if (view.value === 'chat') void scrollBottom();
}

const { keyboardOpen, shortKeyboardViewport } = useAppViewport({
  composerFocused: () => document.activeElement === composer.value?.element,
  hasFixedShell: () => !!shell.value,
  onResize: onViewportResize,
});

function updateFocusOrigin(event: PointerEvent | KeyboardEvent) {
  // 触摸同样能触发 :focus-visible，在编辑过程中沿用进入控件的方式。
  if ('key' in event && event.key !== 'Tab' && event.key !== 'Escape') return;
  document.documentElement.toggleAttribute('data-pointer-focus', event.type === 'pointerdown');
}

onMounted(() => {
  document.addEventListener('pointerdown', updateFocusOrigin, { capture: true, passive: true });
  document.addEventListener('keydown', updateFocusOrigin, true);
  window.addEventListener('hashchange', onConnectionLink);
  window.addEventListener('codex-reload-requested', reloadPage);
  window.addEventListener('online', retryDesktopConnection);
  window.addEventListener('pageshow', retryDesktopConnection);
  window.addEventListener('pagehide', voice.clear);
  document.addEventListener('visibilitychange', onPageVisibility);
  document.addEventListener('gesturestart', preventPageZoom, { passive: false });
  document.addEventListener('gesturechange', preventPageZoom, { passive: false });
  takeConnectionKey();
  void initializeConnection(props.initialSession);
});
onUnmounted(() => {
  document.removeEventListener('pointerdown', updateFocusOrigin, true);
  document.removeEventListener('keydown', updateFocusOrigin, true);
  document.documentElement.removeAttribute('data-pointer-focus');
  cancelHistory();
  cancelBackSwipe();
  clearTimeout(navigationTimer);
  window.removeEventListener('hashchange', onConnectionLink);
  window.removeEventListener('codex-reload-requested', reloadPage);
  window.removeEventListener('online', retryDesktopConnection);
  window.removeEventListener('pageshow', retryDesktopConnection);
  window.removeEventListener('pagehide', voice.clear);
  document.removeEventListener('visibilitychange', onPageVisibility);
  document.removeEventListener('gesturestart', preventPageZoom);
  document.removeEventListener('gesturechange', preventPageZoom);
});
</script>

<template>
  <main v-if="!authenticated" class="login-screen desktop-login">
    <section class="login-card">
      <div class="login-intro">
        <div class="login-symbol" aria-hidden="true"><Monitor /></div>
        <h1>{{ desktopName || (serviceUnavailable ? '无法连接电脑' : '连接电脑') }}</h1>
        <p>{{ desktopName ? '通过云端连接' : serviceUnavailable ? '在电脑上打开 Codex Mobile 并启动连接服务。' : '用手机扫描桌面端的局域网二维码。' }}</p>
      </div>
      <div class="login-connect">
        <p v-if="booting" class="desktop-login-status" role="status"><Loading class="is-spin" aria-hidden="true" />正在连接…</p>
        <div v-else-if="desktopName || serviceUnavailable" class="service-offline">
          <p v-if="loginError" class="login-failure" role="alert">{{ loginError }}</p>
          <button v-if="loginFailureStatus !== 401" type="button" class="primary-button" @click="initializeConnection()">重试连接</button>
          <button v-if="desktopName" type="button" class="secondary-button" @click="emit('chooseDesktop')">管理电脑</button>
        </div>
        <template v-else>
          <button type="button" class="primary-button scan-entry" :disabled="loggingIn" @click="scanOpen = true"><FullScreen aria-hidden="true" />扫码连接</button>
          <form class="desktop-key-form" :aria-busy="loggingIn" @submit.prevent="login()">
            <label for="pair-key">连接密钥</label>
            <input id="pair-key" v-model="loginKey" type="password" autocomplete="off" placeholder="粘贴桌面密钥" :readonly="loggingIn" :aria-invalid="loginFailureStatus === 401" :aria-describedby="loginError ? 'login-error' : undefined" />
            <p v-if="loginError" id="login-error" class="error-text" role="alert">{{ loginError }}</p>
            <button class="secondary-button login-submit" type="submit" :disabled="!loginKey.trim() || loggingIn"><Loading v-if="loggingIn" class="is-spin" aria-hidden="true" />{{ loggingIn ? '正在连接…' : '连接电脑' }}</button>
          </form>
        </template>
      </div>
    </section>
  </main>
  <div v-else ref="shell" :class="['app-shell', { 'show-chat': view === 'chat', 'usage-visible': usagePresented, 'usage-from-projects': usagePresented && !usageReturnToChat, 'has-conversation': selectedId || newChat }]" @scroll.passive="onNavigationScroll" @scrollend.passive="onNavigationScrollEnd" @pointerdown.passive="onNavigationPointerDown" @touchstart.passive="onBackSwipeStart" @touchend="onBackSwipeEnd" @touchcancel.passive="onNavigationTouchEnd" @keydown.esc="composerMenu = false; chatMenu = false">
    <UsagePanel ref="usagePanel" :visible="usageOpen" :initial-thread-id="usageThreadId" :initial-title="usageThreadTitle" :return-to-chat="usageReturnToChat" @back="closeUsage" @expired="expireConnection" />
    <aside class="navigation-pane" :inert="view !== 'projects'">
      <Transition name="catalog-view" @before-leave="element => (element as HTMLElement).inert = true"><ThreadBrowser v-if="catalogScope !== undefined" :key="archivedCatalog ? 'archived' : catalogScope?.projectId || 'all'" :project="catalogScope" :archived="archivedCatalog" :initial-query="search" @back="closeCatalog" @select="openThread" /></Transition>
      <div v-show="catalogScope === undefined" class="navigation-content">
      <header class="navigation-header">
        <div class="navigation-toolbar">
          <h1>{{ hasProjects ? '项目' : '本机历史' }}</h1>
          <button :disabled="!capabilities.create" title="新建聊天" type="button" class="navigation-create" @click="startNewChat()"><Plus aria-hidden="true" />新聊天</button>
        </div>
        <button v-if="desktopName" type="button" class="desktop-switch" :title="desktopName" :aria-expanded="!!devicePickerOpen" @click="emit('chooseDesktop')" aria-haspopup="dialog" aria-label="切换电脑">
          <Monitor aria-hidden="true" />
          <span>{{ desktopName }}</span>
          <ArrowDown class="desktop-switch-chevron" aria-hidden="true" />
        </button>
        <button v-show="!searchOpen" ref="searchToggle" type="button" class="navigation-search-launch" :aria-expanded="searchOpen" aria-controls="navigation-searchbar" @click="toggleSearch"><Search aria-hidden="true" />搜索聊天</button>
        <div v-show="searchOpen" id="navigation-searchbar" class="navigation-searchbar" :inert="!searchOpen" :aria-hidden="!searchOpen || undefined" @keydown.esc.stop.prevent="toggleSearch">
          <div class="navigation-search"><Search aria-hidden="true" /><PlainTextInput ref="searchInput" v-model="search" role="searchbox" placeholder="搜索项目和聊天" aria-label="搜索项目和聊天" /><button v-show="search" type="button" class="icon-button search-clear" aria-label="清空搜索" @mousedown.prevent @click="search = ''; searchInput?.focus()"><Close /></button></div>
          <button type="button" class="search-cancel" @click="toggleSearch">取消</button>
        </div>
      </header>
      <section v-if="recoveredCreations.length" class="navigation-notice" aria-label="已创建的聊天">
        <div v-for="operation in recoveredCreations" :key="operation.id"><span>聊天已创建，草稿已保留。</span> <button type="button" class="text-button" @click="restoreCreation(operation)">查看原聊天</button></div>
      </section>
      <section v-if="unresolvedOperations.length" class="error-banner" aria-label="待确认的操作">
        <div v-for="operation in unresolvedOperations" :key="operation.id">
          <p role="status">{{ operation.target.startsWith('new:') ? '新建聊天' : knownThreads.find(thread => thread.id === operation.target)?.title || '聊天' }}：此前操作的结果待确认，请先核对。</p>
          <button type="button" class="text-button" :disabled="operation.phase !== 'unknown'" @click="closeOperation(operation)">{{ operation.phase === 'unknown' ? '已核对，结束等待' : '正在确认结果…' }}</button>
        </div>
      </section>
      <p v-if="operationsError" class="navigation-notice muted" role="status">{{ operationsError }} <button type="button" class="text-button" @click="loadOperations">重新核对</button></p>
      <p v-if="!capabilities.readLive" class="navigation-notice muted" role="status">本机历史阅读模式。新建和任务操作请在官方 Codex 完成。</p>
      <p v-if="visibleNotice && view === 'projects'" class="navigation-notice muted" role="status">{{ visibleNotice }}</p>
      <div v-if="projectError && view === 'projects'" class="error-banner" role="alert">
        <span>{{ projectError }}</span>
        <button v-if="catalogError" type="button" class="text-button" :disabled="loadingProjects || loadingThreads" aria-label="重新读取项目和聊天" @click="refreshCatalog">重试</button>
        <button v-else-if="visibleConnectionError" type="button" class="text-button" :disabled="streamStatus === 'connecting'" @click="resumeStream">重新连接</button>
        <button class="icon-button" aria-label="关闭提示" @click="catalogError ? catalogError = '' : connectionError = ''"><Close /></button>
      </div>
      <div ref="navigationScroll" class="navigation-scroll">
        <section class="work-section" aria-labelledby="work-heading">
          <div class="work-heading">
            <h2 id="work-heading" class="list-heading">最近工作</h2>
            <button type="button" class="text-button work-history" @click="browseThreads()">{{ search.trim() ? '搜索全部聊天' : '查看全部聊天' }}<ArrowRight aria-hidden="true" /></button>
            <button type="button" class="text-button work-history" @click="browseThreads(null, true)">已归档聊天<ArrowRight aria-hidden="true" /></button>
          </div>
          <AnimatePresence v-if="workThreads.length" as="nav" mode="popLayout" :initial="false" class="work-list" aria-labelledby="work-heading" @before-leave="leaveWorkThread">
            <motion.div v-for="thread in workThreads" :key="thread.id" class="work-thread-row" :initial="{ opacity: 0, y: reduceMotion ? 0 : 6 }" :animate="{ opacity: 1, y: 0 }" :exit="{ opacity: 0 }" :transition="controlSpring">
              <button type="button" class="thread-button" :title="thread.title" @click="openThread(thread)">
                <span class="work-thread-copy">
                  <span class="thread-title">{{ thread.title || '未命名聊天' }}</span>
                  <span class="work-thread-meta">{{ threadProjectLabel(thread) }}</span>
                </span>
                <span class="work-thread-detail">
                  <span v-if="threadActive(thread)" class="thread-executing">执行中</span>
                  <span v-if="thread.updatedAt" class="work-thread-time">{{ relativeTime(thread.updatedAt) }}</span>
                </span>
                <ArrowRight class="work-thread-chevron" aria-hidden="true" />
              </button>
            </motion.div>
          </AnimatePresence>
          <p v-else-if="loadingThreads" class="empty-list navigation-list-state muted" role="status"><Loading class="is-spin" aria-hidden="true" />正在读取聊天…</p>
          <p v-else-if="!catalogError" class="empty-list muted">{{ search.trim() ? '没有找到近期聊天' : '还没有聊天' }}</p>
        </section>
        <details v-if="pinnedThreads.length" class="pinned-section">
          <summary class="list-heading project-section-toggle"><span>置顶聊天</span><ArrowRight class="navigation-disclosure" aria-hidden="true" /></summary>
          <nav aria-label="置顶聊天" class="pinned-list">
            <button v-for="thread in pinnedThreads" :key="thread.id" type="button" class="thread-button" :title="thread.title" @click="openThread(thread)">
              <span class="thread-title">{{ thread.title || '未命名聊天' }}</span><span v-if="threadActive(thread)" class="thread-executing">执行中</span>
            </button>
          </nav>
        </details>
        <button v-if="hasProjects" ref="projectsToggle" class="list-heading project-section-toggle" :aria-expanded="projectsOpen" aria-controls="project-groups" @click="toggleProjects"><span>全部项目</span><ArrowRight class="navigation-disclosure" :class="{ expanded: projectsOpen }" aria-hidden="true" /></button>
        <CollapseMotion v-if="hasProjects" id="project-groups" :open="projectsOpen" :focus-target="projectsToggle">
          <p v-if="loadingProjects && !projects.length" class="empty-list navigation-list-state muted" role="status"><Loading class="is-spin" aria-hidden="true" />正在读取项目…</p>
          <section v-for="project in filteredProjects" :key="project.projectId" :ref="element => observeProject(project.projectId, element)" :data-project-id="project.projectId" class="project-group">
            <div class="project-heading">
              <button class="project-name" :aria-expanded="!projectCollapsed(project)" :aria-controls="'project-threads-' + project.projectId" @click="toggleProject(project)"><FolderOpened aria-hidden="true" /><span>{{ project.label }}</span><ArrowRight class="navigation-disclosure" :class="{ expanded: !projectCollapsed(project) }" aria-hidden="true" /></button>
              <button :disabled="!capabilities.create" class="icon-button project-create" :aria-label="'在 ' + project.label + ' 中新建聊天'" title="新聊天" @click="startNewChat(project.projectId)"><Plus /></button>
            </div>
            <CollapseMotion :id="'project-threads-' + project.projectId" :open="!projectCollapsed(project)" :focus-target="projectsToggle" class="project-threads">
              <button v-for="(thread, index) in threadsInProject(project)" :key="thread.id" type="button" :style="{ '--row-index': index }" class="thread-button" :title="thread.title" @click="openThread(thread)">
                <span class="thread-title">{{ thread.title || '未命名聊天' }}</span><span v-if="threadActive(thread)" class="thread-executing">执行中</span>
              </button>
              <p v-if="!search.trim() && !threadsInProject(project).length && !catalogError" class="project-preview-state muted">
                {{ loadingThreads && !threads.length ? '正在读取聊天…' : hasMoreProjectHistory ? (projectHistoryError ? '近期聊天暂不可用' : '正在查找近期聊天…') : '暂无聊天' }}
              </p>
              <button type="button" class="expand-project text-button" :aria-label="'查看 ' + project.label + ' 的全部聊天'" @click="browseThreads(project)"><ArrowRight />查看全部聊天</button>
            </CollapseMotion>
          </section>
          <div v-if="!filteredProjects.length && !loadingProjects && !catalogError" class="project-list-empty">
            <span class="workspace-symbol" aria-hidden="true"><component :is="search.trim() ? Search : FolderOpened" /></span>
            <div><p>{{ search.trim() ? '没有找到项目' : '暂无项目' }}</p><small v-if="!search.trim()">请先在电脑 Codex 中添加项目。</small></div>
          </div>
        </CollapseMotion>
      </div>
      <footer class="navigation-footer">
        <span class="navigation-connection" role="status"><span :class="['connection-dot', { online: !connectionError && (!selectedId || streamStatus === 'connected') }]" aria-hidden="true"></span>{{ connectionError ? '等待重新连接' : selectedId ? streamStatus === 'connected' ? '已连接电脑' : streamStatus === 'history' ? '本机历史' : '正在连接电脑' : '已连接网页服务' }}</span>
        <button type="button" class="workspace-utility usage-navigation-button" aria-label="Codex Token 用量" title="Codex Token 用量" @click="openUsage()"><Histogram aria-hidden="true" /><span>Token 用量</span></button>
        <button type="button" class="workspace-utility settings-trigger" aria-label="打开设置" aria-haspopup="dialog" @click="openSettings"><Setting aria-hidden="true" /><span>设置</span></button>
      </footer>
      </div>
      <div class="navigation-shade" aria-hidden="true"></div>
    </aside>
    <section ref="chatPane" class="chat-pane" :class="{ 'composer-menu-open': composerMenu }" :inert="view !== 'chat'" @click="composerMenu = false">
      <Transition name="menu-shade"><button v-if="composerMenu || chatMenu" type="button" class="composer-menu-backdrop" aria-label="关闭菜单" @click="composerMenu = false; chatMenu = false"></button></Transition>
      <div ref="chatTopbar" class="chat-topbar">
        <header class="chat-header">
          <button type="button" class="icon-button back-project" :aria-label="conversationHistory.canGoBack.value ? '返回上级对话：' + conversationHistory.parentTitle.value : '返回项目与聊天'" @click="backToPrevious"><ArrowLeft aria-hidden="true" /></button>
          <div class="header-title">
            <div class="header-name">
              <h1 :title="selectedThread?.title">{{ newChat ? '新对话' : selectedThread?.title || 'Codex' }}</h1>
              <span v-if="selectedId" :class="['header-status', { disconnected: streamStatus === 'disconnected' }]" role="status">
                <span :class="['connection-dot', { online: streamStatus === 'connected' }]" aria-hidden="true"></span>
                <span :class="{ 'sr-only': streamStatus === 'connected' }">{{ streamStatus === 'connected' ? capabilities.native ? '桌面同步' : '实时连接' : streamStatus === 'connecting' ? '正在连接电脑' : streamStatus === 'history' ? '历史阅读' : '连接已断开' }}</span>
              </span>
            </div>
            <div class="header-detail">
              <span v-if="selectedProjectId" class="cwd-label">{{ projectTitle }}</span>
              <span v-if="changedFileCount" class="header-files">改动 {{ changedFileCount }} 个文件</span>
            </div>
          </div>
          <div class="header-actions glass-control">
            <button v-if="newOutput && latestInHeader" type="button" class="icon-button" aria-label="回到最新消息" @mousedown.prevent @click="scrollBottom(true)"><ArrowDown /></button>
            <button v-if="active && hasMessage && capabilities.interrupt" type="button" class="icon-button" aria-label="停止任务" :disabled="!capabilities.interrupt || stopping || streamStatus !== 'connected'" @click="stopTurn"><Loading v-if="stopping" class="is-spin" /><span v-else class="task-stop-mark" aria-hidden="true"></span></button>
            <button v-else type="button" class="icon-button" aria-label="新建聊天" :disabled="!capabilities.create" @click="startNewChat()"><EditPen /></button>
            <button ref="chatMenuTrigger" type="button" class="icon-button settings-trigger" aria-label="对话更多操作" aria-haspopup="menu" :aria-expanded="chatMenu" @click="chatMenu = !chatMenu; composerMenu = false"><MoreFilled /></button>
          </div>
        </header>
        <div v-if="visibleError && view === 'chat'" class="error-banner" role="alert">
          <span>{{ visibleError }}</span>
          <button v-if="visibleConnectionError" type="button" class="text-button" :disabled="streamStatus === 'connecting'" @click="resumeStream">重新连接</button>
          <button class="icon-button" aria-label="关闭提示" @click="dismissChatError"><Close /></button>
        </div>
        <Transition name="menu">
          <div v-show="chatMenu" class="composer-menu chat-menu" role="menu" aria-label="对话操作">
            <button type="button" role="menuitem" @click="openSettings($event, chatMenuTrigger)"><Setting />对话设置</button>
            <button v-if="selectedId && (capabilities.threadRename || capabilities.threadPin || capabilities.threadArchive || capabilities.threadDelete)" type="button" role="menuitem" @click="managementOpen = true; chatMenu = false"><EditPen />管理聊天</button>
            <button v-if="selectedId" type="button" role="menuitem" @click="openUsage(true)"><Histogram />Token 用量<span class="menu-item-detail">此对话</span></button>
            <HookReview v-if="selectedId" :thread-id="selectedId" :desktop-supported="!!capabilities.desktopNavigation" @open="chatMenu = false" @after-close="restoreComposerFocus" />
            <button v-if="selectedId" type="button" role="menuitem" :disabled="!capabilities.desktopNavigation" @click="openDesktop(); chatMenu = false"><Connection />在电脑上打开</button>
          </div>
        </Transition>
      </div>
      <div ref="scroller" class="conversation-scroll" @scroll.passive="onScroll" @click="openConversationLink">
        <ImagePreview :key="selectedId" class="conversation" :class="{ 'conversation-empty-layout': newChat || (!turns.length && !visiblePending.length && !loadingInitial) }">
          <Transition name="welcome" appear>
            <section v-if="newChat" :key="selectedProjectId" class="new-chat-intro">
              <span class="workspace-symbol" aria-hidden="true"><FolderOpened /></span>
              <h2>{{ projectTitle }}</h2>
              <p v-if="activeProject?.path">{{ activeProject.path }}</p>
            </section>
          </Transition>
          <template v-if="!newChat">
            <p v-if="historyError" class="error-text small" role="alert">历史消息加载失败。{{ historyError }}</p>
            <button v-if="historyCursor || historyError || loadingHistory" type="button" class="older-button" :disabled="loadingHistory" @click="loadOlder()">{{ loadingHistory ? '加载中…' : historyError ? '重试加载历史消息' : '加载更早的消息' }}</button>
            <div v-if="loadingInitial && !turns.length && !visiblePending.length" class="conversation-loading" role="status">
              <span class="sr-only">正在读取对话…</span>
              <div class="conversation-skeleton" aria-hidden="true"><span></span><span></span><span></span><span></span></div>
            </div>
            <section v-else-if="!turns.length && !visiblePending.length && !visibleError" class="conversation-empty">
              <span class="workspace-symbol" aria-hidden="true"><ChatLineRound /></span>
              <h2>还没有消息</h2>
              <p>{{ capabilities.send ? '在下方输入第一条消息。' : '尚无已保存的消息，请在官方 Codex 查看。' }}</p>
            </section>
            <TurnView v-for="turn in turns" :key="turn.id" :turn="turn" :time="relativeTime(turn.startedAt)" :thread-id="selectedId" :theme="theme" :live="streamStatus === 'connected'" @rendered="scrollBottom()" />
            <ItemView v-for="message in visiblePending" :key="message.requestId" class="message-arrival" :thread-id="selectedId" :theme="theme" :item="{ id: message.requestId, type: 'userMessage', content: [...(message.prompt ? [{ type: 'text', text: message.prompt }] : []), ...message.images.map(image => ({ type: 'image', url: image.url }))], pending: true, pendingLabel: message.accepted ? '已发送，等待同步…' : '正在发送…' }" />
          </template>
        </ImagePreview>
        <FilePreview :key="selectedId" ref="filePreview" :theme="theme" />
      </div>
      <div v-if="selectedId || newChat" ref="composerRegion" class="composer-region" :class="{ 'short-keyboard': shortKeyboardViewport, 'keyboard-open': keyboardOpen, 'question-mode': interactionState.expanded }">
        <Transition name="latest-output"><button v-if="newOutput && !latestInHeader" type="button" class="new-output-button" aria-label="回到最新消息" @mousedown.prevent @click="scrollBottom(true)"><ArrowDown /></button></Transition>
        <div class="composer-support">
        <button v-if="goalSummary" v-show="!interactionState.expanded" type="button" class="goal-summary" :disabled="!capabilities.goals" :aria-label="'查看目标：' + goalSummary.objective" @click="openGoal"><Aim /><span><strong>{{ goalSummary.objective }}</strong><small>{{ goalStatusLabels[goalSummary.status] }} · {{ goalSummary.tokensUsed.toLocaleString('zh-CN') }} Token<template v-if="goalSummary.tokenBudget !== null"> / {{ goalSummary.tokenBudget.toLocaleString('zh-CN') }}</template></small></span><ArrowRight /></button>
        <p v-if="firstTurnPending" class="muted small" role="status">聊天已创建，等待官方首轮确认；不会重复创建。</p>
        <p v-else-if="selectedId && !capabilities.send" class="muted small" role="status">当前仅供阅读，草稿会保留。</p>
        <p v-else-if="draftImages.length && !capabilities.images" class="muted small" role="status">原生图片连接尚未就绪，草稿已保留。请在电脑客户端检查 Codex 连接，原生图片控制连接恢复后重试。</p>
        <div v-if="currentOperation || operationsError || ((hasMessage || resumableTurn) && sendDisabledReason)" class="composer-status muted small" role="status">
          <span>{{ operationsError || sendDisabledReason || '本聊天有一笔操作等待确认，草稿已保留。' }}</span>
          <div v-if="currentOperation || operationsError" class="composer-status-actions">
          <button type="button" class="text-button" @click="loadOperations">重新核对</button>
          <button v-if="currentOperation?.phase === 'unknown'" type="button" class="text-button" @click="closeOperation(currentOperation)">已核对，结束等待</button>
          </div>
        </div>
        <p v-if="currentCreation" class="muted small" role="status">聊天已创建，草稿已保留。 <button type="button" class="text-button" @click="restoreCreation(currentCreation)">查看原聊天</button></p>
        <p v-if="modelSelection.model || modelSelection.thinking" class="muted small" role="status">已选择下次发送的模型选项；发送后由桌面确认。</p>
        <MessageQueue v-if="selectedId && capabilities.queue" :key="selectedId" :queue="queueState" :loading="queueLoading" :busy="queueBusy" :error="queueError" :actions="messageQueue" :capabilities="capabilities" :active="active" :compact="interactionState.expanded" :disabled="!capabilities.queue || sending || streamStatus !== 'connected'" @reload="messageQueue.reload" @expand="pendingPanel?.minimize()" />
        <PendingInteractions v-if="selectedId" ref="pendingPanel" :keyboard-open="keyboardOpen" :thread-id="selectedId" :interactions="interactions" :desktop-supported="!!capabilities.desktopNavigation" :disabled="!(capabilities.interactions || capabilities.asyncReplies) || streamStatus !== 'connected'" :unavailable-reason="!(capabilities.interactions || capabilities.asyncReplies) ? '当前连接不支持在手机回复此请求，请在电脑上的 Codex 处理。' : '连接恢复后可回复，已填写的答案会保留。'" @state="interactionState = $event" @expand="composer?.blur()" @desktop="openDesktop" />
        <Transition name="composer-notice">
          <div v-if="(visibleNotice || subagentInputRejected) && !interactionState.expanded" class="composer-notice" role="status">
            <template v-if="subagentInputRejected">
              <button v-if="conversationHistory.canGoBack.value" type="button" class="text-button composer-parent-link" @click="backToPrevious">在上级对话继续<ArrowRight aria-hidden="true" /></button>
              <span v-else>请在发起任务的对话继续。</span>
            </template>
            <span v-else>{{ visibleNotice }}</span>
          </div>
        </Transition>
        </div>
        <div class="composer-dock" :inert="interactionState.expanded && !!(capabilities.interactions || capabilities.asyncReplies)" :aria-hidden="interactionState.expanded && !!(capabilities.interactions || capabilities.asyncReplies) || undefined"><div class="composer-dock-content">
        <form class="composer" :class="{ expanded: composerExpanded, 'has-message': hasMessage, 'task-active': active }" @submit.prevent="send()" @click.self="voice.clear(); composer?.focus()">
          <input ref="imagePicker" class="hidden-input" type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple aria-label="选择发送的图片" @change="addImages" />
          <div class="composer-content" @input="($event as InputEvent).isComposing && resizeComposer()">
            <ImagePreview v-if="draftImages.length" class="composer-attachments" aria-label="待发送的图片">
              <TransitionGroup name="attachment" appear>
              <div v-for="image in draftImages" :key="image.id" class="draft-image" :class="{ 'image-failed': image.status === 'error' }">
                <img :src="image.previewUrl || mediaUrl(image.url)" :alt="image.name" role="button" tabindex="0" :aria-label="'查看待发送图片：' + image.name" aria-haspopup="dialog" @error="attachments.checkPreview(image.id, draftKey)" />
                <button type="button" class="remove-image" :aria-label="'移除图片：' + image.name" @click="attachments.remove(image.id)"><Close /></button>
                <span v-if="image.status === 'uploading'" class="image-state" role="status"><Loading class="is-spin" />上传中</span>
                <span v-else-if="image.unavailable" class="image-state" role="status" :title="image.error || '请移除后重新选择图片'">已失效</span>
                <button v-else-if="image.status === 'error'" type="button" class="image-retry" :aria-label="'重试上传：' + image.name" :title="image.error" @click="attachments.retry(image.id)"><Refresh />重试</button>
              </div>
              </TransitionGroup>
            </ImagePreview>
            <div v-if="draftParts.contexts.length" class="composer-contexts" aria-label="已添加的上下文"><span v-for="entry in draftParts.contexts" :key="entry.mention" class="composer-context"><span>{{ entry.label }}</span><button type="button" :aria-label="'移除 ' + entry.label" @click="removeContext(entry.mention)"><Close /></button></span></div>
            <PlainTextInput :key="draftKey" ref="composer" v-model="draftText" class="composer-input" multiline :placeholder="!(newChat ? capabilities.create : capabilities.send) ? '记录草稿（当前仅供阅读）' : newChat ? '描述任务…' : '发送消息'" aria-label="消息内容" @click="voice.clear(); composer?.focus()" @focus="voice.clear" @beforeinput="voice.clear" @compositionstart="voice.clear" @paste="pasteImages" @input="onComposerInput" @keydown="onKeydown" />
            <p v-if="attachments.error.value || imageErrors" class="image-upload-error" role="alert">{{ attachments.error.value || '有图片尚未上传，请重试或移除后发送。' }}</p>
            <div v-if="voiceNotice" :class="['voice-notice', { 'voice-error': !!voice.error.value }]" role="status"><span v-if="voice.busy.value" class="voice-dot"></span><span>{{ voiceNotice }}</span><button v-if="voice.error.value" type="button" class="voice-dismiss" aria-label="关闭语音提示" @click="voice.error.value = ''"><Close /></button></div>
          </div>
          <div class="composer-toolbar" @mousedown.prevent>
            <div class="composer-left">
              <button ref="composerPlus" type="button" class="composer-plus icon-button" :aria-label="composerMenu ? '收起更多操作' : '更多操作'" aria-haspopup="menu" aria-controls="composer-menu" :aria-expanded="composerMenu" @click.stop="toggleComposerMenu"><motion.span class="composer-plus-icon" :initial="false" :animate="{ rotate: composerMenu ? 45 : 0 }" :transition="controlSpring" aria-hidden="true"><Plus /></motion.span></button>
              <PermissionSettings :thread="selectedThread" :new-thread="newChat" :editable="!!capabilities.settings" :connected="newChat ? authenticated && !sending : streamStatus === 'connected'" :active="active" />
            </div>
            <div class="composer-actions">
              <ModelSettings :key="draftKey" v-model:selection="modelSelection" :thread="selectedThread" :new-thread="newChat" :connected="!!capabilities.modelSelection && !sending && (newChat ? authenticated : streamStatus === 'connected')" />
              <button type="button" class="voice-button icon-button" :class="{ recording: voice.busy.value }" :aria-label="voiceButtonLabel" :aria-pressed="voice.busy.value" :disabled="sending" @click="toggleVoice"><Close v-if="voice.phase.value === 'starting' || voice.phase.value === 'stopping'" /><VideoPause v-else-if="voice.phase.value === 'listening'" /><Microphone v-else /></button>
              <button type="button" class="send-button" :class="{ 'stop-button': composerAction.kind === 'stop', 'is-busy': composerAction.busy }" :disabled="composerAction.disabled" :aria-label="composerAction.label" @click="composerAction.kind === 'stop' ? stopTurn() : send()"><Transition name="action-symbol" mode="out-in"><span :key="composerAction.busy ? 'busy' : composerAction.kind" class="action-symbol"><Loading v-if="composerAction.busy" class="is-spin" /><span v-else-if="composerAction.kind === 'stop'" class="stop-square"></span><Top v-else /></span></Transition></button>
            </div>
          </div>
          <AnimatePresence :initial="false" @before-leave="(el: Element) => { (el as HTMLElement).inert = true }">
            <motion.div v-if="composerMenu" key="composer-menu" id="composer-menu" class="composer-menu" role="menu" aria-label="添加内容和工作模式" :initial="closedComposerMenu" :animate="{ opacity: 1, y: 0, scale: 1 }" :exit="closedComposerMenu" :transition="controlSpring" @click.stop @keydown.esc.stop.prevent="composerMenu = false; composerPlus?.focus({ preventScroll: true })">
              <p class="composer-menu-heading">添加内容</p>
              <button type="button" role="menuitem" @click="pickImages"><Picture />添加照片</button>
              <button type="button" role="menuitem" :disabled="!capabilities.contextReferences && !capabilities.catalog" @click="openContext"><DocumentAdd />添加上下文</button>
              <button v-if="selectedId && capabilities.delegation" type="button" role="menuitem" :disabled="!capabilities.send || sending || imagesUploading || imageErrors || streamStatus !== 'connected'" @click="openDelegation"><Connection />交给子智能体</button>
              <div class="composer-menu-divider"></div>
              <p class="composer-menu-heading">工作模式</p>
              <button type="button" role="menuitemradio" :aria-checked="currentMode === 'default'" :disabled="!capabilities.settings || !selectedId || sending || modeBusy || streamStatus !== 'connected'" @click="changeMode('default')"><EditPen /><span>执行</span><Check v-if="currentMode === 'default'" class="mode-check" /></button>
              <button type="button" role="menuitemradio" :aria-checked="currentMode === 'plan'" :disabled="!capabilities.settings || !selectedId || sending || modeBusy || streamStatus !== 'connected'" @click="changeMode('plan')"><DocumentAdd /><span>规划</span><Check v-if="currentMode === 'plan'" class="mode-check" /></button>
              <button type="button" role="menuitemradio" :aria-checked="currentMode === 'goal'" :disabled="!capabilities.goals || sending || modeBusy" @click="openGoal"><Aim /><span>目标</span><Check v-if="currentMode === 'goal'" class="mode-check" /></button>
              <p v-if="!capabilities.settings" class="composer-menu-hint">工作模式控制连接尚未就绪。请在电脑客户端检查 Codex 连接，控制连接恢复后即可切换。</p>
              <p v-else-if="!selectedId" class="composer-menu-hint">新对话沿用桌面默认模式。</p>
              <p v-if="modeBusy" class="composer-menu-hint" role="status">正在应用模式…</p>
            </motion.div>
          </AnimatePresence>
        </form>
        </div></div>
      </div>
    </section>
    <AgentDelegation v-if="selectedId && capabilities.send && capabilities.delegation" :open="delegationOpen" :targets="delegationTargets" :queue-supported="!!capabilities.queue" :initial-task="splitDraftContext(delegationSource).text" :busy="sending" :error="delegationError" :unavailable="streamStatus !== 'connected' ? '连接恢复后可委派任务' : imagesUploading ? '图片上传完成后可委派任务' : imageErrors ? '请先重试或移除上传失败的图片' : ''" @submit="delegate" @close="closeDelegation" @after-close="restoreComposerFocus" />
    <ThreadManagement :open="managementOpen" :thread="selectedThread" :capabilities="capabilities" :connected="streamStatus === 'connected'" @close="managementOpen = false" @changed="managedThread" @after-close="restoreComposerFocus" />
    <ComposerContext :open="contextOpen && !!(capabilities.contextReferences || capabilities.catalog)" :thread-id="selectedId" :project-id="selectedProjectId" :source-thread-id="creationSource" :threads="threads" @close="closeContext" @select="insertContext" @after-close="restoreComposerFocus" />
    <GoalPanel :open="goalOpen" :thread-id="selectedId" :goal="selectedThread?.goal" :completed-goal="selectedThread?.completedGoal" :initial-objective="draftParts.text" :create-goal="createGoal" @close="goalOpen = false" @changed="applyGoal" @after-close="restoreComposerFocus" />
    <NativeSheet :open="projectPickerOpen" labelledby="project-picker-title" panel-class="settings-modal workspace-sheet" @close="projectPickerOpen = false" @after-close="newChat && restoreComposerFocus()">
      <div class="modal-heading"><h2 id="project-picker-title">{{ creationSource ? '选择项目' : '选择来源聊天' }}</h2><button type="button" class="icon-button" aria-label="关闭新建选择" @click="projectPickerOpen = false"><Close aria-hidden="true" /></button></div>
      <p class="muted small">{{ creationSource ? '使用所选聊天的官方桌面入口，不创建控制聊天。' : '请选择来源聊天，再选择官方项目。不会自动借用历史聊天。' }}</p>
      <div v-if="!creationSource" class="settings-cluster">
        <button v-for="thread in knownThreads" :key="thread.id" type="button" class="settings-row settings-action" @click="chooseCreationSource(thread.id)"><span><strong>{{ thread.title || '未命名聊天' }}</strong></span><ArrowRight aria-hidden="true" /></button>
        <p v-if="!knownThreads.length" class="settings-empty muted">暂无来源聊天，请先在官方桌面新建。</p>
      </div>
      <p v-else-if="creationLoading" class="settings-empty navigation-list-state muted" role="status"><Loading class="is-spin" aria-hidden="true" />正在读取官方项目…</p>
      <div v-else-if="creationError" class="settings-empty"><p class="error-text" role="alert">{{ creationError }}</p><button type="button" class="secondary-button" @click="chooseCreationSource(creationSource)">重新读取</button><button type="button" class="text-button" @click="creationSource = ''; creationProjects = []">改选来源聊天</button></div>
      <template v-else>
        <p v-if="!creationProjects.length" class="settings-empty muted">暂无官方本机项目，请先在桌面添加项目。</p>
        <div v-else class="settings-cluster">
          <button v-for="project in creationProjects" :key="project.projectId" type="button" class="settings-row settings-action" @click="startNewChat(project.projectId)"><span><span class="settings-item-icon" aria-hidden="true"><FolderOpened /></span><strong>{{ project.label }}</strong></span><ArrowRight aria-hidden="true" /></button>
        </div>
        <button type="button" class="text-button" @click="creationSource = ''; creationProjects = []">改选来源聊天</button>
      </template>
    </NativeSheet>
    <NativeSheet :open="settingsOpen" labelledby="settings-title" panel-class="settings-modal workspace-sheet app-settings" @close="settingsOpen = false">
      <div class="modal-heading"><h2 id="settings-title">设置</h2><button type="button" class="icon-button" aria-label="关闭设置" @click="settingsOpen = false"><Close aria-hidden="true" /></button></div>
      <section class="settings-group" aria-labelledby="preferences-heading">
        <h3 id="preferences-heading">偏好</h3>
        <div class="settings-cluster">
          <button type="button" class="settings-row settings-action" :aria-label="theme === 'dark' ? '外观，切换到浅色主题' : '外观，切换到深色主题'" @click="theme = theme === 'dark' ? 'light' : 'dark'">
            <span><span class="settings-item-icon" aria-hidden="true"><Moon v-if="theme === 'dark'" /><Sunny v-else /></span><strong>外观</strong></span>
            <span class="settings-meta">{{ theme === 'dark' ? '深色' : '浅色' }}<Refresh aria-hidden="true" /></span>
          </button>
          <div class="settings-section">
            <div class="settings-row"><span><span class="settings-item-icon" aria-hidden="true"><Bell /></span><strong>所有对话通知</strong></span><button type="button" role="switch" :aria-checked="notifications.enabled.value" aria-label="所有对话通知" class="switch-button" :disabled="!notifications.supported || notifications.busy.value" @click="notifications.toggle()"><span aria-hidden="true"></span></button></div>
            <p class="muted small">{{ notifications.hint }}</p>
            <button v-if="notifications.enabled.value" type="button" class="text-button notification-test" :disabled="notifications.busy.value" @click="notifications.sendTest()">发送测试通知</button>
            <p v-if="notifications.error.value" class="error-text" role="alert">{{ notifications.error.value }}</p>
            <p v-if="notifications.notice.value" class="muted small" role="status">{{ notifications.notice.value }}</p>
          </div>
        </div>
      </section>
      <section class="settings-group" aria-labelledby="connection-settings-heading">
        <h3 id="connection-settings-heading">连接与维护</h3>
        <div class="settings-cluster">
          <button v-if="desktopName" type="button" class="settings-row settings-action" aria-label="管理电脑" @click="settingsOpen = false; emit('chooseDesktop')"><span><span class="settings-item-icon" aria-hidden="true"><Monitor /></span><strong>管理电脑</strong></span><span class="settings-meta"><span :title="desktopName">{{ desktopName }}</span><ArrowRight aria-hidden="true" /></span></button>
          <button type="button" class="settings-row settings-action" @click="settingsOpen = false; updateSettingsOpen = true"><span><span class="settings-item-icon" aria-hidden="true"><Refresh /></span><strong>在线升级</strong></span><span class="settings-meta"><span v-if="desktopUpgrade.upgrading.value" class="muted small">升级中…</span><ArrowRight aria-hidden="true" /></span></button>
          <button v-if="!desktopName" type="button" class="settings-row settings-action settings-disconnect" :disabled="disconnecting" :aria-busy="disconnecting" :aria-describedby="disconnectError ? 'disconnect-error' : undefined" @click="disconnectBrowser"><span><span class="settings-item-icon" aria-hidden="true"><Loading v-if="disconnecting" class="is-spin" /><SwitchButton v-else /></span><strong>{{ disconnecting ? '正在断开…' : disconnectError ? '重试断开此设备' : '断开此设备' }}</strong></span></button>
        </div>
        <p v-if="appUpdate.error.value" class="error-text" role="alert">{{ appUpdate.error.value }}</p>
        <p v-if="disconnectError" id="disconnect-error" ref="disconnectFailure" class="error-text" role="alert">{{ disconnectError }}</p>
      </section>
    </NativeSheet>
    <UpdateSettings :open="updateSettingsOpen" :upgrading="desktopUpgrade.upgrading.value" :progress="desktopUpgrade.progress.value" :start-upgrade="desktopUpgrade.start" @close="updateSettingsOpen = false" @closed="appUpdate.reload" />
  </div>
  <QrConnect v-if="scanOpen && !authenticated" @close="scanOpen = false" @connect="connectScanned" />
</template>
