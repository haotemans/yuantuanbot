<template>
  <div class="empty-state">
    <svg width="72" height="56" viewBox="0 0 72 56" fill="none" stroke="currentColor"
         stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M20 40a10 10 0 1 1 2-19.9A13 13 0 0 1 47 14a11 11 0 0 1 5 21"
            stroke-dasharray="1 0" />
      <rect x="24" y="34" width="24" height="16" rx="3" stroke-dasharray="4 3" />
      <path d="M30 42h12" opacity="0.7" />
    </svg>
    <div class="empty-state-title">{{ title }}</div>
    <div v-if="hint" class="empty-state-hint">{{ hint }}</div>
  </div>
</template>

<script setup>
// 全站统一空态：线性云团插画 + 一句话现状 + 下一步引导
defineProps({
  title: { type: String, default: '这里还是空的' },
  hint: { type: String, default: '' },
})
</script>
