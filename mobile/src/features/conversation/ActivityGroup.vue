<script setup lang="ts">
import { computed, ref, useId } from 'vue';
import { ArrowDown, Connection, Warning } from '@element-plus/icons-vue';
import ItemView from './ItemView.vue';
import ThinkingDots from './ThinkingDots.vue';
import CollapseMotion from '../../components/CollapseMotion.vue';
import { activitySummary, itemFailed, runningActivityLabel } from './itemPresentation';
import type { ThreadItem } from '../../types';
import { useConversationDisclosure } from './useConversationHistory';

const props = defineProps<{ items: ThreadItem[]; running: boolean; threadId: string; theme: 'dark' | 'light'; live: boolean }>();
const contentId = useId();
const toggle = ref<HTMLButtonElement>();
const expandedByUser = useConversationDisclosure(() => JSON.stringify([props.threadId, 'activity', props.items[0]?.id]));
const failed = computed(() => props.items.some(itemFailed));
const expanded = computed(() => expandedByUser.value ?? (props.running || failed.value));
const label = computed(() => {
  if (props.running) return props.live ? runningActivityLabel(props.items) : '等待连接同步…';
  return failed.value ? '执行遇到问题' : activitySummary(props.items);
});
const rows = computed(() => props.items.flatMap((item) => item.type === 'fileChange' && item.changes?.length
  ? item.changes.map((change: { path: string }) => ({ item, changePath: change.path, key: item.id + ':' + change.path }))
  : [{ item, changePath: undefined, key: item.id }]));
</script>

<template>
  <section :class="['activity-group', { 'activity-running': running, 'activity-failed': failed && !running }]">
    <button ref="toggle" class="activity-toggle" :aria-expanded="expanded" :aria-controls="contentId" @click="expandedByUser = !expanded">
      <Warning v-if="failed && !running" /><Connection v-else-if="!running || !live" />
      <span class="activity-label"><span class="activity-title">{{ label }}</span><ThinkingDots v-if="running && live" /></span><ArrowDown :class="['activity-chevron', { expanded }]" />
    </button>
    <CollapseMotion :id="contentId" :open="expanded" :focus-target="toggle">
      <div class="activity-items">
        <ItemView v-for="row in rows" :key="row.key" :item="row.item" :change-path="row.changePath" :thread-id="threadId" :theme="theme" :live="live" />
      </div>
    </CollapseMotion>
  </section>
</template>

<style scoped>
.activity-toggle .activity-chevron { margin-inline-start: auto; transition: transform .24s var(--motion-ease); }
.activity-running { padding-block: 0; border-block: 0; }
.activity-running .activity-toggle { padding-inline: 10px; border-radius: 12px; background: var(--panel); color: var(--text); }
.activity-failed .activity-toggle > svg:first-child { color: var(--error); }
.activity-items { margin-block-end: 6px; padding-inline-start: 10px; }
.activity-label { display: inline-flex; align-items: center; min-width: 0; }
.activity-title { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.activity-label :deep(.thinking-dots) { flex: none; }
@media (prefers-reduced-motion: reduce) {
  .activity-toggle .activity-chevron { transition: none; }
}
</style>
