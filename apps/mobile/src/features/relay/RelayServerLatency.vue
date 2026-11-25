<script setup lang="ts">
import { Connection } from '@element-plus/icons-vue';
import { formatRelayLatency } from './useRelayLatency';

defineProps<{ sample?: number | null }>();
</script>

<template>
  <p class="relay-server-latency">
    <Connection aria-hidden="true" />
    <span>到中转服务器</span>
    <strong>{{ formatRelayLatency(sample) }}</strong>
  </p>
</template>

<style scoped>
.relay-server-latency {
  display: flex; align-items: center; gap: 8px; min-height: 40px; margin: 0; padding: 6px 12px;
  border: 1px solid var(--border); border-radius: 12px; background: var(--sidebar);
  color: var(--muted); font-size: 13px;
}
.relay-server-latency svg { width: 15px; height: 15px; flex: none; }
.relay-server-latency strong { flex: none; margin-inline-start: auto; color: var(--text); font-weight: 600; font-variant-numeric: tabular-nums; }
</style>
