<template>
  <span class="mono">{{ display }}</span>
</template>

<script setup>
// 数字滚动：状态变化时在老值 ↔ 新值之间做 350ms 插值，避免硬跳
import { onUnmounted, ref, watch } from 'vue'

const props = defineProps({
  value: { type: Number, default: null },
  duration: { type: Number, default: 350 },
  fallback: { type: String, default: '—' },
})

const display = ref(props.fallback)
let raf = null
let current = null

function animateTo(target) {
  if (raf) cancelAnimationFrame(raf)
  if (current == null || !Number.isFinite(current)) {
    current = target
    display.value = String(Math.round(target))
    return
  }
  const from = current
  const t0 = performance.now()
  const step = (now) => {
    const k = Math.min((now - t0) / props.duration, 1)
    const eased = 1 - Math.pow(1 - k, 3)
    const v = from + (target - from) * eased
    display.value = String(Math.round(v))
    if (k < 1) {
      raf = requestAnimationFrame(step)
    } else {
      current = target
      raf = null
    }
  }
  raf = requestAnimationFrame(step)
}

watch(() => props.value, (v) => {
  if (v == null || Number.isNaN(v)) {
    display.value = props.fallback
    return
  }
  animateTo(v)
}, { immediate: true })

onUnmounted(() => { if (raf) cancelAnimationFrame(raf) })
</script>
