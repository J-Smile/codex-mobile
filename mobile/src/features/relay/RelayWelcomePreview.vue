<script setup lang="ts">
import { CircleCheckFilled, FolderOpened, Monitor } from '@element-plus/icons-vue';
</script>

<template>
  <div class="relay-preview ss-stage" aria-hidden="true">
    <div class="relay-preview-window ss-window">
      <div class="relay-preview-toolbar"><Monitor /><span>工作电脑</span></div>
      <div class="relay-preview-work">
        <div class="relay-preview-project"><FolderOpened /><span>网站项目</span></div>
        <div class="relay-preview-task">验证页面改动</div>
        <div class="relay-preview-output"><span class="relay-preview-dot"></span>构建已完成</div>
        <div class="relay-preview-lines"><span></span><span></span><span></span></div>
      </div>
    </div>
    <span class="relay-preview-caption">连接示意</span>
    <div class="relay-preview-phone">
      <div class="relay-preview-toolbar"><img src="/icon.svg" alt="" width="16" height="16" /><span>手机对话</span></div>
      <div class="relay-preview-thread">
        <div class="relay-preview-question">构建结果怎么样？</div>
        <div class="relay-preview-answer">
          <span class="relay-preview-result-icon"><CircleCheckFilled /></span>
          <div><strong>构建通过</strong><span>页面改动已验证</span></div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.relay-preview {
  --preview-gutter: 16px;
  position: relative;
  isolation: isolate;
  min-width: 0;
  min-height: clamp(198px, 61vw, 260px);
  overflow: hidden;
  background: var(--sidebar);
  border: 1px solid var(--border);
  border-radius: 19px;
  line-height: 1.4;
}
.relay-preview-window,
.relay-preview-phone {
  position: absolute;
  background: var(--page);
  border: 1px solid var(--border);
  box-shadow: var(--glass-shadow);
}
.relay-preview-window {
  inset: var(--preview-gutter) calc(var(--preview-gutter) + 22px) -40px var(--preview-gutter);
  border-radius: 12px 12px 0 0;
}
.relay-preview-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 38px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  color: var(--text);
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
}
.relay-preview-toolbar svg, .relay-preview-toolbar img, .relay-preview-project svg {
  flex: none;
  width: 16px;
  height: 16px;
}
.relay-preview-work { padding: 12px; }
.relay-preview-project { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 650; }
.relay-preview-project svg { color: var(--muted); }
.relay-preview-task { margin-top: 6px; color: var(--muted); font-size: 12px; }
.relay-preview-output {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 16px;
  color: var(--text);
  font-size: 12px;
}
.relay-preview-dot {
  width: 7px;
  height: 7px;
  flex: none;
  border-radius: 50%;
  background: var(--online);
}
.relay-preview-lines {
  display: grid;
  gap: 8px;
  width: min(38%, 112px);
  margin-top: 12px;
}
.relay-preview-lines span { display: block; height: 5px; border-radius: 4px; background: var(--border); }
.relay-preview-lines span:nth-child(2) { width: 76%; }
.relay-preview-lines span:nth-child(3) { width: 55%; }
.relay-preview-caption {
  position: absolute;
  top: calc(var(--preview-gutter) + 10px);
  right: var(--preview-gutter);
  padding: 2px 8px;
  border-radius: 6px;
  background: var(--panel);
  color: var(--muted);
  font-size: 12px;
}
.relay-preview-phone {
  inset: 30% var(--preview-gutter) -46px auto;
  width: min(52%, 214px);
  border-radius: 16px 16px 0 0;
}
.relay-preview-phone .relay-preview-toolbar { background: var(--sidebar); border-radius: 16px 16px 0 0; }
.relay-preview-thread { display: grid; gap: 12px; padding: 12px; }
.relay-preview-question {
  justify-self: end;
  padding: 6px 8px;
  border-radius: 10px;
  background: var(--signal-soft);
  color: var(--text);
  font-size: 12px;
}
.relay-preview-answer {
  display: grid;
  grid-template-columns: 16px minmax(0, 1fr);
  align-items: start;
  gap: 8px;
  color: var(--text);
  font-size: 12px;
}
.relay-preview-result-icon { display: block; width: 16px; height: 16px; margin-top: 2px; color: var(--online); }
.relay-preview-result-icon svg { width: 100%; height: 100%; }
.relay-preview-answer strong { display: block; font-size: 13px; font-weight: 650; }
.relay-preview-answer > div > span { display: block; color: var(--muted); }
@media (prefers-reduced-motion: no-preference) {
  .relay-preview-answer { animation: preview-answer-arrives 480ms var(--motion-ease) 800ms both; }
  @keyframes preview-answer-arrives {
    from { opacity: 0; transform: translateY(6px); }
  }
}
@media (max-width: 360px) {
  .relay-preview { --preview-gutter: 12px; }
}
</style>
