<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import { ArrowLeft, ArrowRight, Histogram, Loading, Refresh } from '@element-plus/icons-vue';
import { ApiError, request, threadUrl } from '../../api';
import type { UsageMetrics, UsageOverview, UsagePeriod, UsageThread, UsageThreads } from './usage';

const props = defineProps<{ visible: boolean; initialThreadId?: string; initialTitle?: string; returnToChat?: boolean }>();
const emit = defineEmits<{ back: []; expired: [] }>();
type Tab = 'overview' | 'daily' | 'models' | 'threads';
const tabs = [['overview', '概览'], ['daily', '每日'], ['models', '模型'], ['threads', '对话']] as const;
const tab = ref<Tab>('overview');
const tabIndex = computed(() => tabs.findIndex(entry => entry[0] === tab.value));
const tabDirection = ref(1);
const period = ref<UsagePeriod>('7d');
const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
const threadId = ref('');
const threadTitle = ref('');
const data = ref<UsageOverview | null>(null);
const entries = ref<UsageThread[]>([]);
const nextCursor = ref<string | null>(null);
const listLoaded = ref(false);
const loading = ref(false);
const listLoading = ref(false);
const error = ref('');
const listError = ref('');
const search = ref('');
const scroller = ref<HTMLElement>();
let controller: AbortController | undefined;
let listController: AbortController | undefined;
let timer: ReturnType<typeof setTimeout> | undefined;
let searchTimer: ReturnType<typeof setTimeout> | undefined;
let generation = 0;
type ViewState = { period: UsagePeriod; search: string; tab: Tab; scrollTop: number; data: UsageOverview | null; entries: UsageThread[]; nextCursor: string | null; listLoaded: boolean };
const savedGlobal = shallowRef<ViewState>();
const sessions = new Map<string, ViewState & { threadId: string; threadTitle: string; parent?: ViewState }>();
let entryScope: string | undefined;
const hasParent = computed(() => !!threadId.value && !!savedGlobal.value);
const backLabel = computed(() => hasParent.value ? '返回用量对话列表' : props.returnToChat ? '返回原对话' : '返回项目与聊天');
defineExpose({ back, hasParent });

function captureView(): ViewState {
  return { period: period.value, search: search.value, tab: tab.value, scrollTop: scroller.value?.scrollTop || 0, data: data.value, entries: entries.value, nextCursor: nextCursor.value, listLoaded: listLoaded.value };
}
function restoreView(saved: ViewState) {
  period.value = saved.period;
  search.value = saved.search;
  tab.value = saved.tab;
  data.value = saved.data;
  entries.value = saved.entries;
  nextCursor.value = saved.nextCursor;
  listLoaded.value = saved.listLoaded;
}
function rememberSession() {
  if (entryScope !== undefined) sessions.set(entryScope, { ...captureView(), threadId: threadId.value, threadTitle: threadTitle.value, parent: savedGlobal.value });
}
function back() {
  if (!hasParent.value) return false;
  void showAll();
  return true;
}
const numberFormat = new Intl.NumberFormat('zh-CN');
const integer = (value: number | null | undefined) => value == null ? '未记录' : numberFormat.format(value);
const timestamp = (value: string) => new Date(value).toLocaleString('zh-CN', { timeZone, month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
const periodLabel = computed(() => ({ today: '今天', '7d': '近 7 天', '30d': '近 30 天', all: '全部时间' })[period.value]);
const scopeLabel = computed(() => threadId.value ? data.value?.scope.kind === 'thread' ? data.value.scope.title || threadTitle.value || '未命名聊天' : threadTitle.value || '未命名聊天' : '全部对话');
const dailyGroups = computed(() => {
  const days = [...(data.value?.daily || [])].sort((a, b) => b.date.localeCompare(a.date));
  if (period.value !== 'all') return [{ month: '', days, total: 0 }];
  const groups = new Map<string, UsageOverview['daily']>();
  for (const day of days) {
    const month = day.date.slice(0, 7);
    if (!groups.has(month)) groups.set(month, []);
    groups.get(month)!.push(day);
  }
  return [...groups].map(([month, days]) => ({ month, days, total: days.reduce((sum, day) => sum + day.metrics.totalTokens, 0) }));
});
const peakDay = computed(() => data.value?.daily.reduce((peak, day) => Math.max(peak, day.metrics.totalTokens), 0) || 0);
const modelShare = (metrics: UsageMetrics) => data.value?.selected?.totalTokens ? (metrics.totalTokens / data.value.selected.totalTokens * 100).toLocaleString('zh-CN', { maximumFractionDigits: 1 }) + '%' : '0%';
const detailRows = (metrics: UsageMetrics) => [
  ['输入', metrics.inputTokens], ['输出', metrics.outputTokens], ['缓存命中（输入内）', metrics.cachedInputTokens],
  ['缓存写入（输入内）', metrics.cacheWriteInputTokens], ['推理（输出内）', metrics.reasoningOutputTokens],
] as const;

function stop() {
  clearTimeout(timer);
  clearTimeout(searchTimer);
  timer = undefined;
  generation++;
  controller?.abort();
  listController?.abort();
  controller = undefined;
  listController = undefined;
  loading.value = false;
  listLoading.value = false;
}
function canLoad() { return props.visible && document.visibilityState !== 'hidden'; }
function schedule() {
  clearTimeout(timer);
  if (canLoad()) timer = setTimeout(() => void load(), 10_000);
}
function query() { return new URLSearchParams({ period: period.value, timeZone }); }
async function load(clear = false, refreshList = false) {
  if (!canLoad()) return;
  clearTimeout(timer);
  controller?.abort();
  const pending = new AbortController();
  controller = pending;
  const version = generation;
  if (clear) data.value = null;
  loading.value = true;
  error.value = '';
  try {
    const url = threadId.value ? threadUrl(threadId.value, 'usage') : '/api/usage';
    const result = await request<UsageOverview>(url + '?' + query(), { signal: pending.signal });
    if (pending.signal.aborted || generation !== version) return;
    data.value = result;
    if (!threadId.value && tab.value === 'threads') void loadThreads(false, !refreshList);
  } catch (failure) {
    if (!pending.signal.aborted && generation === version) {
      error.value = (failure as Error).message;
      if (failure instanceof ApiError && failure.status === 401) emit('expired');
    }
  } finally {
    if (controller === pending) { controller = undefined; loading.value = false; schedule(); }
  }
}
async function loadThreads(append = false, preserveCount = false) {
  if (!canLoad() || threadId.value || (append && (!nextCursor.value || listLoading.value))) return;
  listController?.abort();
  const pending = new AbortController();
  listController = pending;
  const version = generation;
  listLoading.value = true;
  listError.value = '';
  try {
    const params = query();
    params.set('limit', '30');
    if (search.value.trim()) params.set('search', search.value.trim());
    if (append && nextCursor.value) params.set('cursor', nextCursor.value);
    let result = await request<UsageThreads>('/api/usage/threads?' + params, { signal: pending.signal });
    const items = [...result.items];
    while (preserveCount && !append && result.nextCursor && items.length < entries.value.length && !pending.signal.aborted) {
      params.set('cursor', result.nextCursor);
      result = await request<UsageThreads>('/api/usage/threads?' + params, { signal: pending.signal });
      if (!result.items.length) break;
      items.push(...result.items);
    }
    if (pending.signal.aborted || generation !== version) return;
    entries.value = append ? [...entries.value, ...items.filter(item => !entries.value.some(known => known.threadId === item.threadId))] : items;
    nextCursor.value = result.nextCursor;
    listLoaded.value = result.status !== 'indexing';
    if (data.value && result.coverage) data.value.coverage = result.coverage;
  } catch (failure) {
    if (!pending.signal.aborted && generation === version) {
      listError.value = (failure as Error).message;
      if (failure instanceof ApiError && failure.status === 401) emit('expired');
    }
  } finally { if (listController === pending) { listController = undefined; listLoading.value = false; } }
}
function resetData() {
  stop();
  data.value = null;
  entries.value = [];
  nextCursor.value = null;
  listLoaded.value = false;
  error.value = '';
  listError.value = '';
}
function changePeriod() { resetData(); void load(); }
function changeSearch() {
  listController?.abort();
  clearTimeout(searchTimer);
  listError.value = '';
  listLoaded.value = false;
  nextCursor.value = null;
  entries.value = [];
  searchTimer = setTimeout(() => void loadThreads(), 250);
}
function selectTab(value: Tab) {
  tabDirection.value = tabs.findIndex(entry => entry[0] === value) < tabIndex.value ? -1 : 1;
  tab.value = value;
  if (scroller.value) scroller.value.scrollTop = 0;
  if (value === 'threads' && !threadId.value && !listLoaded.value) void loadThreads();
}
function openThread(entry: UsageThread) {
  savedGlobal.value = captureView();
  resetData();
  threadId.value = entry.threadId;
  threadTitle.value = entry.title;
  tab.value = 'overview';
  if (scroller.value) scroller.value.scrollTop = 0;
  void load();
}
async function showAll() {
  resetData();
  threadId.value = '';
  threadTitle.value = '';
  const saved = savedGlobal.value;
  savedGlobal.value = undefined;
  if (saved) restoreView(saved);
  else tab.value = 'threads';
  await nextTick();
  if (scroller.value) scroller.value.scrollTop = saved?.scrollTop || 0;
  schedule();
  if (!saved) void load();
}
function visibilityChanged() { if (canLoad()) void load(); else stop(); }
watch([() => props.visible, () => props.initialThreadId], ([visible], previous) => {
  const wasVisible = previous?.[0];
  if (!visible) {
    if (wasVisible) rememberSession();
    stop();
    if (wasVisible) void nextTick(() => {
      const preferred = document.querySelector<HTMLElement>(props.returnToChat
        ? '.chat-topbar button[aria-label="对话更多操作"]' : '.usage-navigation-button');
      const target = preferred?.closest('[inert]')
        ? document.querySelector<HTMLElement>('.usage-navigation-button') : preferred;
      target?.focus({ preventScroll: true });
    });
    return;
  }
  const scope = props.initialThreadId || '';
  if (scope !== entryScope) {
    if (wasVisible) rememberSession();
    resetData();
    entryScope = scope;
    const saved = sessions.get(scope);
    savedGlobal.value = saved?.parent;
    threadId.value = saved?.threadId ?? scope;
    threadTitle.value = saved?.threadTitle ?? props.initialTitle ?? '';
    if (saved) restoreView(saved);
    else { tab.value = 'overview'; period.value = '7d'; search.value = ''; }
    void nextTick(() => { if (scroller.value) scroller.value.scrollTop = saved?.scrollTop || 0; });
  } else if (!wasVisible) {
    const scrollTop = sessions.get(scope)?.scrollTop || 0;
    void nextTick(() => { if (scroller.value) scroller.value.scrollTop = scrollTop; });
  }
  void load();
}, { immediate: true });
onMounted(() => document.addEventListener('visibilitychange', visibilityChanged));
onBeforeUnmount(() => { stop(); document.removeEventListener('visibilitychange', visibilityChanged); });
</script>

<template>
  <section class="usage-pane" aria-label="Codex 用量" :inert="!visible" :style="{ '--usage-direction': tabDirection }">
    <div class="usage-page">
      <header class="usage-header">
        <button class="icon-button" :aria-label="backLabel" @click="emit('back')"><ArrowLeft /></button>
        <h1>Token 用量</h1>
        <button class="icon-button" aria-label="刷新用量" :disabled="loading || listLoading" @click="load(false, true)"><Refresh :class="{ 'is-spin': loading }" /></button>
      </header>
      <nav class="usage-tabs" aria-label="用量页面">
        <div class="usage-tab-track">
        <span class="usage-tab-indicator" aria-hidden="true" :style="{ transform: `translateX(${tabIndex * 100}%)` }"></span>
        <button v-for="entry in tabs" :key="entry[0]" :aria-current="tab === entry[0] ? 'page' : undefined" @click="selectTab(entry[0])">{{ entry[1] }}</button>
        </div>
      </nav>
      <div ref="scroller" class="usage-scroll">
        <div class="usage-filters">
          <div class="usage-scope"><span>{{ threadId ? '仅此对话' : '统计范围' }}</span><strong>{{ scopeLabel }}</strong><button v-if="threadId" class="text-button" @click="showAll"><ArrowLeft />全部对话</button></div>
          <label>时间<select v-model="period" aria-label="用量时间范围" @change="changePeriod"><option value="today">今天</option><option value="7d">近 7 天</option><option value="30d">近 30 天</option><option value="all">全部</option></select></label>
        </div>
        <div v-if="error" class="usage-message error-text" role="alert"><span>{{ error }}</span><button class="text-button" :disabled="loading" @click="load(false, true)">重试</button></div>
        <p v-if="loading && !data" class="usage-empty usage-loading" role="status"><Loading class="is-spin" />正在读取用量…</p>
        <template v-if="data">
          <div v-if="data.status === 'indexing'" class="usage-empty" role="status"><h2>正在整理本机用量</h2><p class="muted">已处理 {{ integer(data.coverage.processedSources) }} / {{ integer(data.coverage.discoveredSources) }} 份记录，完成后显示统计。</p></div>
          <div v-else-if="data.dataState === 'unavailable'" class="usage-empty"><h2>用量明细不可用</h2><p class="muted">{{ threadId ? '此对话' : '本机历史' }}缺少可核验的用量明细，暂时无法统计。可查看下方数据覆盖说明，或稍后刷新。</p></div>
          <div v-else-if="!data.lifetime" class="usage-empty" role="status"><h2>正在整理本机用量</h2><p class="muted">索引首次初始化完成后会显示统计，请稍后刷新。</p></div>
          <template v-else>
            <section v-if="tab === 'overview'" class="usage-view" aria-label="用量概览">
              <div v-if="data.selected" class="usage-overview-summary">
                <dl class="usage-total-focus"><div><dt><Histogram aria-hidden="true" />{{ periodLabel }}用量</dt><dd><span>{{ integer(data.selected.totalTokens) }}</span><small>Token</small></dd></div></dl>
                <dl class="usage-split"><div><dt>输入</dt><dd>{{ integer(data.selected.inputTokens) }}</dd></div><div><dt>输出</dt><dd>{{ integer(data.selected.outputTokens) }}</dd></div></dl>
              </div>
              <dl class="usage-totals"><div><dt>已记录累计</dt><dd><span>{{ integer(data.lifetime.totalTokens) }}</span> <small>Token</small></dd></div><div><dt>今天</dt><dd><span>{{ integer(data.today?.totalTokens) }}</span> <small v-if="data.today">Token</small></dd></div></dl>
              <details v-if="data.selected" class="usage-disclosure"><summary><span>查看 Token 明细</span><ArrowRight /></summary><dl class="usage-details"><div v-for="[label, value] in detailRows(data.selected).slice(2)" :key="label"><dt>{{ label }}</dt><dd>{{ integer(value) }}</dd></div></dl><p class="usage-caption">缓存和推理统计在输入、输出内。缺少字段显示“未记录”。</p></details>
              <p v-if="data.lifetime.totalTokens === 0" class="usage-empty muted">暂无用量记录</p>
            </section>
            <section v-else-if="tab === 'daily'" class="usage-view" aria-label="每日用量">
              <h2>每日用量</h2><p class="usage-caption">按 {{ timeZone }} 分日 · {{ periodLabel }}</p>
              <p v-if="!data.daily.length" class="usage-empty muted">这段时间暂无用量记录。</p>
              <template v-else>
                <component v-for="(group, index) in dailyGroups" :is="group.month ? 'details' : 'div'" :key="group.month" :class="{ 'usage-month usage-disclosure': group.month }" :open="group.month ? index === 0 : undefined">
                  <summary v-if="group.month"><span>{{ group.month }}</span><span>{{ integer(group.total) }} Token</span><ArrowRight /></summary>
                  <dl class="usage-details usage-daily">
                    <div v-for="day in group.days" :key="day.date">
                      <dt>{{ day.date }}</dt>
                      <dd>
                        <span>{{ integer(day.metrics.totalTokens) }} Token</span>
                        <span v-if="peakDay" class="usage-day-bar" aria-hidden="true"><span :style="{ width: (day.metrics.totalTokens / peakDay * 100) + '%' }"></span></span>
                      </dd>
                    </div>
                  </dl>
                </component>
              </template>
            </section>
            <section v-else-if="tab === 'models'" class="usage-view" aria-label="模型用量">
              <h2>记录模型</h2><p class="usage-caption">按调用时记录的模型统计 · {{ periodLabel }}</p>
              <p v-if="!data.models.length" class="usage-empty muted">这段时间暂无用量记录。</p>
              <details v-for="model in data.models" :key="model.model ?? ''" class="usage-disclosure usage-model"><summary><span><strong>{{ model.model || '未记录模型' }}</strong><small>{{ integer(model.metrics.totalTokens) }} Token · {{ modelShare(model.metrics) }}</small></span><ArrowRight /></summary><dl class="usage-details"><div v-for="[label, value] in detailRows(model.metrics)" :key="label"><dt>{{ label }}</dt><dd>{{ integer(value) }}</dd></div></dl></details>
            </section>
            <section v-else class="usage-view" aria-label="对话用量">
              <h2>对话用量</h2><p class="usage-caption">{{ threadId ? '仅统计此对话自身新增用量，子对话单独统计。' : '按所选范围 Token 排序，每个对话只统计自身新增用量。' }}</p>
              <template v-if="threadId"><p class="usage-empty">{{ scopeLabel }}<br><strong>{{ integer(data.selected?.totalTokens) }} Token</strong></p><button class="secondary-button" @click="showAll">查看全部对话用量</button></template>
              <template v-else>
                <label class="usage-search"><span class="sr-only">搜索用量对话</span><input v-model="search" type="search" placeholder="搜索对话标题" aria-label="搜索用量对话" @input="changeSearch" /></label>
                <div v-if="listError" class="usage-message error-text" role="alert"><span>{{ listError }}</span><button class="text-button" :disabled="listLoading" @click="loadThreads(false)">重新加载列表</button></div>
                <p v-else-if="!entries.length" class="usage-empty muted" role="status">{{ listLoading || !listLoaded ? '正在读取对话用量…' : '暂无符合条件的对话。' }}</p>
                <ul class="usage-thread-list"><li v-for="entry in entries" :key="entry.threadId"><button @click="openThread(entry)"><span><strong>{{ entry.title || '未命名聊天' }}</strong><small>{{ entry.metrics ? integer(entry.metrics.totalTokens) + ' Token' : '历史记录不可用' }}<template v-if="entry.parentThreadId"> · 子对话</template></small><small v-if="entry.lastRecordedAt">最近用量 {{ timestamp(entry.lastRecordedAt) }}</small></span><ArrowRight /></button></li></ul>
                <p v-if="data.coverage.unavailableThreads" class="usage-caption">{{ integer(data.coverage.unavailableThreads) }} 个对话缺少可核验的用量明细，仍显示在列表中。</p>
                <button v-if="nextCursor" class="load-more text-button" :disabled="listLoading" @click="loadThreads(true)">{{ listLoading ? '加载中…' : '加载更多对话' }}</button>
              </template>
            </section>
          </template>
          <details class="usage-disclosure usage-coverage"><summary><span>数据覆盖</span><span class="muted">{{ data.status === 'indexing' ? '正在整理' : data.status === 'partial' ? '部分历史不可用' : '本机已记录' }}</span><ArrowRight /></summary><p class="usage-caption">仅统计这台电脑可核验的 Codex 用量，不代表账号全部设备的账单。已保存的用量会在原记录缺失后保留。</p><p v-if="data.coverage.indexUnavailable" class="usage-caption">对话索引暂时无法读取，部分标题或历史可能缺失。</p><dl class="usage-details"><div><dt>已处理记录</dt><dd>{{ integer(data.coverage.processedSources) }} / {{ integer(data.coverage.discoveredSources) }}</dd></div><div><dt>缺失记录</dt><dd>{{ integer(data.coverage.missingSources) }}</dd></div><div><dt>无法读取</dt><dd>{{ integer(data.coverage.unreadableSources) }}</dd></div><div v-if="data.coverage.unsupportedSources"><dt>格式暂不支持</dt><dd>{{ integer(data.coverage.unsupportedSources) }}</dd></div><div v-if="data.coverage.gapEvents"><dt>未能还原的历史段</dt><dd>{{ integer(data.coverage.gapEvents) }}</dd></div><div><dt>待核对用量</dt><dd>{{ integer(data.coverage.unassignedEvents) }}</dd></div><div v-if="data.coverage.unavailableThreads"><dt>历史缺失的对话</dt><dd>{{ integer(data.coverage.unavailableThreads) }}</dd></div></dl><p v-if="data.coverage.firstRecordedAt" class="usage-caption">事件记录：{{ timestamp(data.coverage.firstRecordedAt) }} 至 {{ timestamp(data.coverage.lastRecordedAt!) }}</p></details>
          <p class="usage-updated muted" role="status">{{ loading || data.refreshing ? '正在刷新… · 上次 ' + timestamp(data.generatedAt) : timestamp(data.generatedAt) + ' 更新' }} · {{ timeZone }}</p>
        </template>
      </div>
    </div>
  </section>
</template>

<style scoped>
.usage-pane { --usage-inset: clamp(16px, 5vw, 24px); min-width: 0; min-height: 0; background: var(--page); color: var(--text); }
.usage-pane :focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.usage-page { display: flex; flex-direction: column; width: 100%; height: 100%; margin: 0 auto; }
.usage-header { display: flex; align-items: center; gap: 12px; padding: calc(var(--toolbar-padding) + var(--safe-top-visible)) var(--usage-inset) var(--toolbar-padding); }
.usage-header .icon-button { background: var(--panel); }
.usage-header .icon-button:hover:not(:disabled) { background: var(--hover); }
.usage-header h1 { flex: 1; min-width: 0; font-size: 21px; font-weight: 650; margin: 0; }
.usage-tabs { padding: 0 var(--usage-inset) 16px; border-bottom: 1px solid var(--border); }
.usage-tab-track { position: relative; display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border-radius: 14px; background: var(--panel); }
.usage-tab-indicator { position: absolute; inset: 2px auto 2px 0; width: 25%; border-inline: 2px solid transparent; border-radius: 13px; background: var(--signal-soft) padding-box; transition: transform 280ms var(--motion-ease); pointer-events: none; }
.usage-tabs button { position: relative; min-height: 44px; padding: 8px; border: 0; border-radius: 12px; background: transparent; color: var(--muted); font-size: 15px; transition: color 140ms ease; }
.usage-tabs button[aria-current] { color: var(--signal); font-weight: 650; }
.usage-tabs button:focus-visible { outline-offset: -4px; }
@media (hover: hover) { .usage-tabs button:hover { color: var(--signal); } }
.usage-scroll { flex: 1; min-height: 0; overflow-y: auto; padding: 16px var(--usage-inset) max(24px, var(--safe-bottom-visible)); overscroll-behavior-y: contain; }
.usage-filters { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; margin-bottom: 18px; }
.usage-filters label { flex: 0 0 110px; color: var(--muted); font-size: 12px; }
.usage-filters select { display: block; width: 100%; min-height: 44px; margin-top: 6px; padding: 8px 10px; border: 1px solid var(--border); border-radius: 10px; background: var(--panel); color: var(--text); font-size: 16px; }
.usage-scope { min-width: 0; display: flex; flex-direction: column; align-items: flex-start; gap: 6px; }
.usage-scope > span { color: var(--muted); font-size: 12px; }
.usage-scope strong { font-size: 16px; line-height: 1.45; font-weight: 600; overflow-wrap: anywhere; }
.usage-scope button { display: flex; align-items: center; gap: 4px; min-height: 44px; color: var(--signal); }
.usage-scope button svg { width: 14px; height: 14px; }
h2 { font-size: 19px; font-weight: 650; margin: 24px 0 8px; }
.usage-overview-summary { padding: clamp(14px, 4vw, 20px); border-radius: 18px; background: var(--signal-soft); }
.usage-total-focus { margin: 0; }
.usage-total-focus dt { display: flex; align-items: center; gap: 7px; color: var(--signal); font-size: 13px; font-weight: 600; }
.usage-total-focus dt svg { width: 16px; height: 16px; }
.usage-total-focus dd { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; margin: 8px 0 16px; font-size: clamp(28px, 8.2vw, 42px); font-weight: 650; letter-spacing: -.045em; font-variant-numeric: tabular-nums; line-height: 1.12; }
.usage-total-focus dd span { min-width: 0; overflow-wrap: anywhere; }
.usage-total-focus dd small { color: var(--signal); font-size: 13px; letter-spacing: normal; font-weight: 500; }
.usage-totals, .usage-split { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px 16px; }
.usage-split { padding-top: 12px; margin: 0; border-top: 1px solid color-mix(in srgb, var(--signal) 24%, transparent); }
.usage-totals { padding: 16px 0; margin: 4px 0 0; border-bottom: 1px solid var(--border); }
.usage-totals dt, .usage-split dt { color: var(--muted); font-size: 12px; }
.usage-totals dd, .usage-split dd { margin: 5px 0 0; min-width: 0; overflow-wrap: anywhere; font-size: clamp(16px, 4.5vw, 19px); font-weight: 600; line-height: 1.4; font-variant-numeric: tabular-nums; }
.usage-totals dd small { color: var(--muted); font-size: 11px; font-weight: 400; }
.usage-details { margin: 12px 0; font-size: 14px; }
.usage-details > div { display: flex; justify-content: space-between; gap: 16px; padding: 12px 0; }
.usage-details dt { color: var(--muted); min-width: 0; }
.usage-details dd { margin: 0; text-align: right; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; min-width: 0; max-width: 58%; }
.usage-disclosure { border-bottom: 1px solid var(--border); }
.usage-disclosure summary { display: flex; align-items: center; gap: 12px; min-height: 48px; padding: 12px 0; cursor: pointer; list-style: none; font-size: 14px; }
.usage-disclosure summary::-webkit-details-marker { display: none; }
.usage-disclosure summary > span:first-child { flex: 1; min-width: 0; }
.usage-disclosure summary > svg { width: 14px; height: 14px; margin-left: auto; transition: transform 180ms var(--motion-ease); }
.usage-disclosure[open] > summary > svg { transform: rotate(90deg); }
.usage-disclosure summary strong { display: block; font-size: 14px; font-weight: 600; overflow-wrap: anywhere; line-height: 1.65; }
.usage-disclosure summary small { display: block; color: var(--muted); font-size: 12px; margin-top: 4px; font-variant-numeric: tabular-nums; }
.usage-caption { margin: 8px 0 12px; color: var(--muted); font-size: 12px; line-height: 1.75; overflow-wrap: anywhere; }
.usage-daily { margin-block: 0 8px; }
.usage-daily > div { align-items: start; border-bottom: 1px solid var(--border); padding: 14px 0; }
.usage-daily > div:last-child { border-bottom: 0; }
.usage-daily dd { display: grid; gap: 8px; width: 54%; font-weight: 550; }
.usage-day-bar { height: 6px; border-radius: 3px; overflow: hidden; background: var(--panel); }
.usage-day-bar > span { display: block; height: 100%; border-radius: inherit; background: var(--signal); transform-origin: left; }
.usage-month summary > span:nth-child(2) { font-size: 12px; font-variant-numeric: tabular-nums; }
.usage-thread-list { list-style: none; margin: 12px 0 0; padding: 0; }
.usage-search { display: block; margin-top: 16px; }
.usage-search input { width: 100%; min-height: 44px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 10px; background: var(--page); color: var(--text); font-size: 16px; }
.usage-search input::placeholder { color: var(--muted); }
.usage-thread-list li { border-bottom: 1px solid var(--border); }
.usage-thread-list button { display: flex; align-items: center; justify-content: space-between; gap: 16px; width: 100%; padding: 14px 10px; min-height: 60px; border: 0; border-radius: 10px; background: transparent; text-align: left; }
.usage-thread-list button:focus-visible { outline-offset: -2px; }
.usage-thread-list button:hover { background: var(--hover); }
.usage-thread-list button > span { min-width: 0; }
.usage-thread-list strong { display: block; font-size: 14px; font-weight: 500; line-height: 1.65; overflow-wrap: anywhere; }
.usage-thread-list small { display: block; color: var(--muted); font-size: 12px; margin-top: 4px; font-variant-numeric: tabular-nums; }
.usage-thread-list svg { width: 14px; height: 14px; }
.usage-message { display: flex; align-items: center; flex-wrap: wrap; gap: 4px 12px; margin: 18px 0; font-size: 14px; line-height: 1.65; }
.usage-message > span { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.usage-message button { min-height: 44px; flex: none; font-size: 14px; }
.usage-empty { padding: 16px; border-radius: 14px; margin: 0; background: var(--panel); color: var(--muted); font-size: 14px; line-height: 1.65; overflow-wrap: anywhere; }
.usage-empty h2 { margin-top: 0; }
.usage-empty p { margin-bottom: 0; }
.usage-empty + .secondary-button { margin-top: 12px; }
.usage-totals ~ .usage-empty { margin-top: 12px; }
.usage-loading { display: flex; align-items: center; gap: 8px; }
.usage-loading > svg { width: 16px; height: 16px; }
.usage-coverage { margin-top: 28px; }
.usage-coverage summary > span:nth-child(2) { font-size: 12px; }
.usage-updated { margin: 18px 0 0; font-size: 12px; line-height: 1.7; }
@media (max-width: 360px) { .usage-filters { gap: 12px; } .usage-totals, .usage-split { column-gap: 12px; } }
@media (max-height: 450px) { .usage-tabs { padding-bottom: 6px; } .usage-scroll { padding-top: 10px; } .usage-filters { margin-bottom: 12px; } }
@media (prefers-reduced-motion: no-preference) {
  .usage-view { animation: usage-change 220ms var(--motion-ease); }
  .usage-day-bar > span { animation: usage-bar-arrive 300ms var(--motion-ease) both; }
  @keyframes usage-change { from { opacity: .4; transform: translateX(calc(var(--usage-direction) * 8px)); } }
  @keyframes usage-bar-arrive { from { transform: scaleX(0); } }
  @supports (interpolate-size: allow-keywords) {
    .usage-disclosure { interpolate-size: allow-keywords; }
    .usage-disclosure::details-content { height: 0; opacity: 0; overflow: clip; transition: height 280ms var(--motion-ease), opacity 180ms ease, content-visibility 280ms allow-discrete; }
    .usage-disclosure[open]::details-content { height: auto; opacity: 1; }
  }
}
@media (prefers-reduced-motion: reduce) { .usage-disclosure summary > svg, .usage-tab-indicator { transition: none; } }
</style>
