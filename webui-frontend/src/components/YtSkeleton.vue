<template>
  <div class="yt-skeleton" :style="{ height: height + 'px', borderRadius: radius + 'px' }" aria-hidden="true" />
</template>

<script setup>
// 通用占位骨架块：替代加载中的"空数据"闪烁，与亮暗主题自动适配
defineProps({
  height: { type: Number, default: 16 },
  radius: { type: Number, default: 8 },
})
</script>

<style scoped>
.yt-skeleton {
  position: relative;
  overflow: hidden;
  background: var(--yt-card-border);
}
.yt-skeleton::after {
  content: '';
  position: absolute;
  inset: 0;
  background: linear-gradient(90deg, transparent, var(--yt-header-bg), transparent);
  animation: yt-shimmer 1.4s ease infinite;
  transform: translateX(-100%);
}
@keyframes yt-shimmer {
  to { transform: translateX(100%); }
}
@media (prefers-reduced-motion: reduce) {
  .yt-skeleton::after { animation: none; }
}
</style>
