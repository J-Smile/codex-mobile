<script setup lang="ts">
import { computed } from 'vue';
import { ArrowRight } from '@element-plus/icons-vue';
import type { ThreadItem } from '../../types';
import { subagents } from './subagentPresentation';
import { conversationLink } from './links';

const props = defineProps<{ item: ThreadItem }>();
const agents = computed(() => subagents(props.item));
const prompt = computed(() => typeof props.item.prompt === 'string' ? props.item.prompt : '');
</script>

<template>
  <div class="subagent-detail">
    <template v-if="prompt">
      <div class="tool-section-label">任务内容</div>
      <p class="subagent-text">{{ prompt }}</p>
    </template>
    <ul v-if="agents.length" class="subagent-list">
      <li v-for="agent in agents" :key="agent.id" class="subagent-entry">
        <div class="subagent-heading">
          <span class="subagent-name" :title="agent.path || agent.name">{{ agent.name }}</span>
          <span :class="['subagent-status', { 'subagent-failed': agent.failed }]">{{ agent.status }}</span>
        </div>
        <p v-if="agent.model || agent.effort" class="subagent-meta">{{ [agent.model, agent.effort].filter(Boolean).join(' · ') }}</p>
        <p v-if="agent.message" class="subagent-text">{{ agent.message }}</p>
        <a class="subagent-link" :href="conversationLink(agent.id)" :aria-label="'查看子智能体 ' + agent.name + ' 的对话'">
          {{ agent.completed ? '查看结果' : '打开子对话' }}<ArrowRight />
        </a>
      </li>
    </ul>
    <p v-else class="subagent-empty">暂无可查看的子对话。</p>
  </div>
</template>

<style scoped>
.subagent-detail { min-width: 0; font-size: 13px; line-height: 1.6; }
.subagent-list { list-style: none; margin: 0; padding: 0; }
.subagent-entry + .subagent-entry { margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--border); }
.subagent-heading { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }
.subagent-name { min-width: 0; font-weight: 500; overflow-wrap: anywhere; }
.subagent-status { flex-shrink: 0; color: var(--muted); font-size: 12px; }
.subagent-failed { color: var(--error); }
.subagent-meta, .subagent-empty { margin: 4px 0; color: var(--muted); font-size: 12px; overflow-wrap: anywhere; }
.subagent-text { margin: 6px 0 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
.subagent-link { display: inline-flex; align-items: center; gap: 5px; min-height: 44px; color: var(--signal); text-decoration: underline; text-underline-offset: 3px; }
.subagent-link svg { width: 13px; height: 13px; }
.subagent-link:hover { text-decoration-thickness: 2px; }
.subagent-link:focus-visible { outline: 2px solid var(--focus); outline-offset: 3px; border-radius: 3px; }
</style>
