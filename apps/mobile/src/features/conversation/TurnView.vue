<script setup lang="ts">
import { computed } from 'vue';
import ActivityGroup from './ActivityGroup.vue';
import ItemView from './ItemView.vue';
import ThinkingDots from './ThinkingDots.vue';
import { conversationBlocks, type ConversationBlock } from './itemPresentation';
import type { Turn } from '../../types';

const props = defineProps<{ turn: Turn; time: string; threadId: string; theme: 'dark' | 'light'; live: boolean }>();
const emit = defineEmits<{ rendered: [] }>();
const blocks = computed<ConversationBlock[]>(previous => conversationBlocks(props.turn.items, previous));
const failed = computed(() => props.turn.status === 'failed' || (props.turn.status === 'interrupted' && !!props.turn.error));
const failureMessage = computed(() => {
  const error = props.turn.error;
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') return error.message;
  return '任务因异常中断。';
});
// Existing replies are history; only text that arrives after this view opens enters.
const initialReplies = new Set(props.turn.items.filter(item => item.type === 'agentMessage' && item.text).map(item => item.id));
</script>

<template>
  <section v-if="blocks.length || turn.status === 'inProgress' || failed" class="turn">
    <div class="turn-time">{{ time }}</div>
    <template v-for="(block, index) in blocks" :key="block.id">
      <ActivityGroup v-if="block.type === 'activity'" :items="block.items" :running="turn.status === 'inProgress' && index === blocks.length - 1" :thread-id="threadId" :theme="theme" :live="live" />
      <ItemView v-else :item="block.items[0]!" :thread-id="threadId" :theme="theme" :streaming="turn.status === 'inProgress'" :live="live" :arrive="live && !initialReplies.has(block.id)" @rendered="emit('rendered')" />
    </template>
    <Transition name="working-indicator">
      <div v-if="turn.status === 'inProgress' && blocks.at(-1)?.type !== 'activity'" class="working-indicator" role="status"><span>{{ live ? '正在思考' : '等待连接同步…' }}<ThinkingDots v-if="live" /></span></div>
    </Transition>
    <div v-if="failed" class="turn-failure" role="alert">
      <p>{{ failureMessage }}</p>
    </div>
  </section>
</template>
