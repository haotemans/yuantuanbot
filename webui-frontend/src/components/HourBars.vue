<template>
  <div class="hourbars">
    <svg :viewBox="`0 0 ${W} ${H}`" class="hourbars-svg" role="img" aria-label="今日消息趋势，收到和发送按小时并排显示；展开下方数据表可查看具体数量">
      <g v-for="tick in ticks" :key="tick.value">
        <line :x1="PAD" :x2="W - 12" :y1="tick.y" :y2="tick.y" class="hb-grid" />
        <text :x="PAD - 8" :y="tick.y + 4" class="hb-tick" text-anchor="end">{{ tick.value }}</text>
      </g>
      <g v-for="(c, i) in cols" :key="i">
        <rect :x="c.x" :y="BASE - c.inH" :width="BAR_W" :height="c.inH" class="hb-in" rx="2" />
        <rect :x="c.x + BAR_W + 2" :y="BASE - c.outH" :width="BAR_W" :height="c.outH" class="hb-out" rx="2" />
        <text v-if="i % 3 === 0 || i === 23" :x="c.x + BAR_W" :y="H - 6" class="hb-tick" text-anchor="middle">{{ String(i).padStart(2, '0') }}时</text>
        <title>{{ i }} 时：收到 {{ c.in }}，发送 {{ c.out }}</title>
      </g>
    </svg>
    <details><summary>查看每小时数据</summary><div class="hour-table"><table><thead><tr><th>时间</th><th>收到</th><th>发送</th></tr></thead><tbody><tr v-for="(c, i) in cols" :key="i"><th>{{ i }}:00</th><td>{{ c.in }}</td><td>{{ c.out }}</td></tr></tbody></table></div></details>
  </div>
</template>
<script setup>
import { computed } from 'vue'
const props = defineProps({ hours: { type: /** @type {import('vue').PropType<{ in: number, out: number }[]>} */ (Array), default: () => [] } })
const W = 760, H = 210, PAD = 44, BASE = 178
const SLOT = (W - PAD - 12) / 24, BAR_W = 9
const ceiling = computed(() => {
  const max = Math.max(1, ...props.hours.flatMap(h => [h.in || 0, h.out || 0]))
  const step = Math.max(1, Math.pow(10, Math.floor(Math.log10(max))) / 2)
  return Math.ceil(max / (step * 4)) * step * 4
})
const ticks = computed(() => [0, 1, 2, 3, 4].map(i => ({ value: Math.round(ceiling.value * i / 4), y: BASE - i / 4 * 156 })))
const cols = computed(() => Array.from({ length: 24 }, (_, i) => {
  const h = props.hours[i] || { in: 0, out: 0 }
  return { in: h.in || 0, out: h.out || 0, inH: (h.in || 0) / ceiling.value * 156, outH: (h.out || 0) / ceiling.value * 156, x: PAD + i * SLOT + (SLOT - BAR_W * 2 - 2) / 2 }
}))
</script>
<style scoped>
.hourbars-svg { width: 100%; height: auto; min-height: 150px; display: block; }
.hb-grid { stroke: var(--yt-tl-line); stroke-width: 1; stroke-dasharray: 3 4; }
.hb-in { fill: var(--yt-chart-in); }
.hb-out { fill: var(--yt-chart-out); }
.hb-tick { font-size: 11px; fill: var(--yt-ink-3); font-family: var(--yt-font); }
summary { width: fit-content; cursor: pointer; color: var(--yt-primary); font-size: 12px; margin-top: 12px; }
.hour-table { max-height: 220px; overflow: auto; margin-top: 12px; }
table { width: 100%; border-collapse: collapse; font-size: 13px; font-variant-numeric: tabular-nums; }
th, td { text-align: left; padding: 6px 12px; border-bottom: 1px solid var(--yt-card-border); }
th { font-weight: 500; color: var(--yt-ink-2); }
</style>
