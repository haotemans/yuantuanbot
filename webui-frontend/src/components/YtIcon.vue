<template>
  <svg :width="size" :height="size" viewBox="0 0 24 24" fill="none"
       stroke="currentColor" :stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <template v-for="(d, i) in paths" :key="i">
      <path :d="d" />
    </template>
  </svg>
</template>

<script setup>
import { computed } from 'vue'

// 全站手写线性图标（stroke 风格，24 网格），侧栏菜单 / 登录能力点 / 空态共用
const ICONS = {
  dashboard: ['M3 3h8v10H3z', 'M13 3h8v6h-8z', 'M13 11h8v10h-8z', 'M3 15h8v6H3z'],
  trace: ['M3 12h4l3-8 4 16 3-8h4'],
  tasks: ['M9 6h11', 'M9 12h11', 'M9 18h11', 'M4 5.5l1 1 2-2', 'M4 11.5l1 1 2-2', 'M4 17.5l1 1 2-2'],
  memories: ['M4 5a2 2 0 0 1 2-2h6v18H6a2 2 0 0 1-2-2z', 'M12 3h6a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-6', 'M8 8h1', 'M8 12h1'],
  relations: ['M9 6a3 3 0 1 0 0.01 0', 'M17 16a3 3 0 1 0 0.01 0', 'M11 8l4 6', 'M6 8.5l6 6'],
  platform: ['M9 2v6', 'M15 2v6', 'M7 8h10v4a5 5 0 0 1-10 0z', 'M12 17v5'],
  models: ['M7 7h10v10H7z', 'M10 10h4v4h-4z', 'M12 2v3', 'M12 19v3', 'M2 12h3', 'M19 12h3'],
  personality: ['M12 3a7 7 0 0 1 7 7c0 2-1 3.5-2 5-.6 1-1 2-1 4H8c0-2-.4-3-1-4-1-1.5-2-3-2-5a7 7 0 0 1 7-7z', 'M9 12c1 1 5 1 6 0', 'M9 9h.01', 'M15 9h.01'],
  meme: ['M4 4h16v16H4z', 'M8.5 10a1.5 1.5 0 1 0 .01 0', 'M4 16l5-5 4 4 3-3 4 4'],
  kb: ['M6 3h9l5 5v13H6z', 'M15 3v5h5', 'M9 13h7', 'M9 16h7'],
  backup: ['M4 8a8 4.5 0 0 1 16 0v8a8 4.5 0 0 1-16 0z', 'M4 8v4a8 4.5 0 0 0 16 0V8'],
  spark: ['M12 3v3', 'M12 18v3', 'M3 12h3', 'M18 12h3', 'M5.6 5.6l2.1 2.1', 'M16.3 16.3l2.1 2.1', 'M5.6 18.4l2.1-2.1', 'M16.3 7.7l2.1-2.1'],
  check: ['M4 12.5l5 5L20 6.5'],
  chat: ['M4 5h16v10H8l-4 4z', 'M8 9h8', 'M8 12h5'],
  tune: ['M4 7h16', 'M4 17h16', 'M9 7a2.5 2.5 0 1 0 .01 0', 'M15 17a2.5 2.5 0 1 0 .01 0'],
}

const props = defineProps({
  name: { type: String, required: true },
  size: { type: [Number, String], default: 18 },
})

const paths = computed(() => ICONS[props.name] || ICONS.spark)
</script>
