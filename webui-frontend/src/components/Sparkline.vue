<template>
  <svg :width="width" :height="height" :viewBox="`0 0 ${width} ${height}`" class="sparkline" aria-hidden="true">
    <defs>
      <linearGradient :id="gid" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" :stop-color="color" stop-opacity="0.25" />
        <stop offset="100%" :stop-color="color" stop-opacity="0.02" />
      </linearGradient>
    </defs>
    <path :d="areaPath" :fill="`url(#${gid})`" />
    <polyline :points="linePoints" fill="none" :stroke="color" stroke-width="1.8"
              stroke-linecap="round" stroke-linejoin="round" />
    <circle v-if="lastPt" :cx="lastPt[0]" :cy="lastPt[1]" r="2.4" :fill="color" />
  </svg>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  points: { type: /** @type {import('vue').PropType<number[]>} */ (Array), default: () => [] },
  color: { type: String, default: '#2563eb' },
  width: { type: Number, default: 116 },
  height: { type: Number, default: 34 },
})

let seq = 0
const gid = `sg-${++seq}`

// 纯 SVG 手写迷你趋势：归一化到画布，首尾留 1px 呼吸边；单点/全等退化为平线
const coords = computed(() => {
  const raw = props.points.length ? props.points : [0, 0]
  const vals = raw.length === 1 ? [raw[0], raw[0]] : raw
  const lo = Math.min(...vals)
  const hi = Math.max(...vals)
  const span = hi - lo || 1
  const n = Math.max(vals.length, 2)
  const pad = 1
  return vals.map((v, i) => {
    const x = pad + (i / (n - 1)) * (props.width - pad * 2)
    const y = props.height - pad - ((v - lo) / span) * (props.height - pad * 2 - 4) - 2
    return [Number(x.toFixed(2)), Number(y.toFixed(2))]
  })
})

const linePoints = computed(() => coords.value.map((p) => p.join(',')).join(' '))
const lastPt = computed(() => (coords.value.length ? coords.value[coords.value.length - 1] : null))
const areaPath = computed(() => {
  const cs = coords.value
  if (!cs.length) return ''
  const first = cs[0]
  const last = cs[cs.length - 1]
  const mids = cs.map((p) => `L${p[0]},${p[1]}`).join(' ')
  return `M${first[0]},${props.height - 1} ${mids} L${last[0]},${props.height - 1} Z`
})
</script>

<style scoped>
.sparkline {
  display: block;
}
</style>
