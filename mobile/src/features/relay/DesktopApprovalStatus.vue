<script setup lang="ts">
import { Monitor } from '@element-plus/icons-vue';

defineProps<{ name: string; code: string }>();
defineEmits<{ cancel: [] }>();
</script>

<template>
  <div class="approval-status">
    <div class="approval-notice" role="status">
      <div class="approval-heading"><Monitor aria-hidden="true" /><strong>等待电脑授权</strong></div>
      <p class="muted">在「{{ name }}」上核对数字，点击「确认授权」。</p>
      <p class="approval-code" aria-label="核对码">{{ code }}</p>
    </div>
    <button class="secondary-button" type="button" @click="$emit('cancel')">取消授权</button>
  </div>
</template>

<style scoped>
.approval-status { display: grid; gap: 12px; }
.approval-notice { display: grid; gap: 8px; padding: 14px; border: 1px solid var(--border); border-radius: 14px; background: var(--sidebar); }
.approval-heading { display: flex; align-items: center; gap: 8px; }
.approval-heading svg { color: var(--signal); }
.approval-status p { margin: 0; font-size: 14px; line-height: 1.5; overflow-wrap: anywhere; }
.approval-status .approval-code { color: var(--signal); font-size: 28px; font-weight: 650; font-variant-numeric: tabular-nums; }
.approval-status button { justify-self: start; }
</style>
