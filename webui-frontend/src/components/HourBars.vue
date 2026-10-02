<template>
  <svg :viewBox="`0 0 ${W} ${H}`" class="hourbars-svg" role="img" aria-label="今日消息趋势（按小时）">
    <line :x1="PAD" :x2="W - PAD" :y1="BASE" :y2="BASE" class="hb-axis" />
    <g v-for="(c, i) in cols" :key="i">
      <rect v-if="c.inH > 0" :x="c.x" :y="BASE - c.inH" :width="BAR_W" :height="c.inH" class="hb-in" rx="2" />
      <rect v-if="c.outH > 0" :x="c.x" :y="BASE - c.inH - c.outH - (c.inH > 0 ? 1 : 0)" :width="BAR_W" :height="c.outH" class="hb-out" rx="2" />
      <text v-if="i % 6 === 0" :x="c.x + BAR_W / 2" :y="H - 2" class="hb-tick" text-anchor="middle">{{ i }}</text>
      <title>{{ i }} 时 · 收 {{ c.in }} / 发 {{ c.out }}</title>
    </g>
  </svg>
</template>

<script setup>
import { computed } from 'vue'

// 今日消息趋势：24 根堆叠柱（收=青蓝，发=靛蓝），手写 SVG；
// 数据源由父组件从 /api/events 前端聚合：hours[24] = { in, out }
const props = defineProps({
  hours: { type: /** @type {import('vue').PropType<{ in: number, out: number }[]>} */ (Array), default: () => [] },
})

const W = 720
const H = 140
const PAD = 8
const BASE = 116
const SLOT = (W - PAD * 2) / 24
const BAR_W = Math.min(18, SLOT * 0.6)

const cols = computed(() => {
  const max = Math.max(1, ...props.hours.map((h) => (h.in || 0) + (h.out || 0)))
  const top = BASE - 14
  return props.hours.map((h, i) => ({
    in: h.in || 0,
    out: h.out || 0,
    inH: Math.round(((h.in || 0) / max) * top),
    outH: Math.round(((h.out || 0) / max) * top),
    x: PAD + i * SLOT + (SLOT - BAR_W) / 2,
  }))
})
</script>

<style scoped>
.hourbars-svg {
  width: 100%;
  height: 140px;
  display: block;
}
.hb-axis {
  stroke: var(--yt-tl-line);
  stroke-width: 1;
}
.hb-in {
  fill: #06b6d4;
  opacity: 0.9;
}
.hb-out {
  fill: #4f46e5;
  opacity: 0.78;
}
.hb-in:hover,
.hb-out:hover {
  opacity: 1;
}
.hb-tick {
  font-size: 10px;
  fill: var(--yt-text-dim);
  font-family: var(--yt-mono);
}
</style>
