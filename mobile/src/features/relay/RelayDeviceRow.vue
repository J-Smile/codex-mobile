<script setup lang="ts">
import { useId } from 'vue';
import { ArrowRight, Check, Monitor } from '@element-plus/icons-vue';
import { formatRelayLatency } from './useRelayLatency';

defineProps<{ name: string; status: string; online: boolean; paired: boolean; latency?: number | null; current?: boolean; disabled?: boolean }>();
defineEmits<{ select: [] }>();
const statusId = useId();
</script>

<template>
  <div class="relay-device-row" :class="{ 'is-current': current }">
    <button class="relay-device-choice" type="button" :aria-label="name" :aria-describedby="statusId"
      :aria-current="current ? 'true' : undefined" :disabled="disabled" @click="$emit('select')">
      <span class="relay-device-symbol" aria-hidden="true"><Monitor /></span>
      <span class="relay-device-copy">
        <strong>{{ name }}</strong>
        <span :id="statusId" class="relay-device-status">
          <span class="relay-device-presence"><span class="connection-dot" :class="{ online }" aria-hidden="true"></span>{{ status }}</span>
          <span v-if="!paired" class="relay-device-unpaired">待配对</span>
          <span v-else-if="online" class="relay-device-latency">到电脑 {{ formatRelayLatency(latency) }}</span>
        </span>
      </span>
      <Check v-if="current" class="relay-device-current" aria-hidden="true" />
      <ArrowRight v-else class="relay-device-chevron" aria-hidden="true" />
    </button>
    <slot />
  </div>
</template>

<style scoped>
.relay-device-row { display: flex; align-items: center; gap: 2px; min-width: 0; }
.relay-device-row.is-current { background: var(--signal-soft); }
.relay-device-choice {
  display: flex; flex: 1; align-items: center; gap: 11px; min-width: 0; min-height: 64px;
  padding: 9px 12px; border: 0; background: transparent; color: var(--text); text-align: left;
  transition: background-color 160ms ease;
}
.relay-device-choice:focus-visible { outline-offset: -3px; }
.relay-device-choice:active:not(:disabled) { background: var(--hover); }
.relay-device-symbol {
  display: grid; place-items: center; flex: 0 0 36px; width: 36px; height: 36px; border-radius: 11px;
  background: var(--panel); color: var(--muted);
}
.is-current .relay-device-symbol { color: var(--signal); background: var(--page); }
.relay-device-copy { flex: 1; display: grid; gap: 3px; min-width: 0; }
.relay-device-copy strong { font-size: 15px; line-height: 1.4; font-weight: 600; overflow-wrap: anywhere; }
.relay-device-status { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 6px; color: var(--muted); font-size: 12px; line-height: 1.35; }
.relay-device-presence { display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
.relay-device-presence .connection-dot { flex: none; }
.relay-device-unpaired, .relay-device-latency { border-inline-start: 1px solid var(--border); padding-inline-start: 6px; }
.relay-device-latency { white-space: nowrap; font-variant-numeric: tabular-nums; }
.relay-device-chevron, .relay-device-current { width: 15px; height: 15px; color: var(--muted); }
.relay-device-current { color: var(--signal); }
@media (hover: hover) { .relay-device-choice:hover:not(:disabled) { background: var(--hover); } }
</style>
