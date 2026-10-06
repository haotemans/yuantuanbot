<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">仪表盘</span>
      <span class="yt-page-sub">3 秒判生死</span>
      <span class="spacer" />
      <span class="live-pill" :class="{ on: wsConnected }" :title="wsConnected ? '实时推送已连接' : '实时推送断开（轮询兜底中）'">
        <span class="live-dot" />LIVE
      </span>
      <n-tag :type="d.adapter_connected ? 'success' : 'error'" round size="small">
        适配器 · {{ d.adapter_connected ? '已连接' : '离线' }}
      </n-tag>
      <n-tag round size="small">mood · {{ d.mood ?? '—' }}</n-tag>
      <n-tag round size="small" class="mono">运行 · {{ fmtUptime(d.uptime_secs) }}</n-tag>
      <n-button size="tiny" secondary @click="load">刷新</n-button>
    </div>

    <n-grid cols="1 s:2 l:4" :x-gap="14" :y-gap="14" responsive="screen">
      <n-gi v-for="s in statCards" :key="s.label">
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" :style="{ background: s.color }" />
            {{ s.label }}
          </div>
          <div class="stat-foot">
            <span class="stat-num">
              <CountUp :value="s.value" />
            </span>
            <Sparkline :points="s.points" :color="s.color" />
          </div>
        </n-card>
      </n-gi>
    </n-grid>

    <n-card title="今日消息趋势" size="small" style="margin-top: 14px">
      <template #header-extra>
        <span class="stat-hint">
          <span style="color:var(--yt-grad-from)">■</span> 收
          &nbsp;<span style="color:var(--yt-primary)">■</span> 发 · 按小时聚合最近事件
        </span>
      </template>
      <template v-if="loadingFirst">
        <yt-skeleton :height="132" />
      </template>
      <template v-else>
        <HourBars v-if="hasHourData" :hours="hourSeries" />
        <empty-state v-else title="今天还没有消息样本" hint="群里有人说话后，这里会按小时画出收发趋势" />
      </template>
    </n-card>

    <n-card title="最近事件" size="small" style="margin-top: 14px">
      <template v-if="loadingFirst">
        <div style="display: flex; flex-direction: column; gap: 10px">
          <yt-skeleton v-for="i in 5" :key="i" :height="18" />
        </div>
      </template>
      <template v-else>
        <div v-if="recent.length">
          <transition-group name="yt-fade">
            <div v-for="e in recent" :key="e.id ?? e._k" class="mini-event">
              <span class="mini-dot" :style="{ background: kindColor(e.kind) }" />
              <span class="mini-kind" :style="{ color: kindColor(e.kind) }">{{ e.kind }}</span>
              <span class="mini-text">{{ eventSummary(e) }}</span>
              <span class="mini-time">{{ fmtAgo(e.ts) }}</span>
            </div>
          </transition-group>
        </div>
        <empty-state v-else title="还没有事件" hint="等群里有人说话，第一条 MessageReceived 就会出现在这里" />
      </template>
    </n-card>
  </div>
</template>

<script>
// 会话级采样历史（模块作用域，切页不丢）：四个指标的近实时走势，30s 一个点
const TREND_CAP = 40
const trend = { inToday: [], outToday: [], decision: [], tasks: [] }
</script>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { useUiStore } from '../store/ui'
import { kindColor, eventSummary, fmtAgo, fmtUptime } from '../fmt'
import Sparkline from '../components/Sparkline.vue'
import HourBars from '../components/HourBars.vue'
import EmptyState from '../components/EmptyState.vue'
import CountUp from '../components/CountUp.vue'
import YtSkeleton from '../components/YtSkeleton.vue'
import { subscribeWs, wsConnected } from '../ws'

/** @type {import('vue').Ref<Record<string, any>>} */
const d = ref({})
/** @type {import('vue').Ref<any[]>} */
const recent = ref([])
/** @type {import('vue').Ref<{ in: number, out: number }[]>} */
const hourSeries = ref(Array.from({ length: 24 }, () => ({ in: 0, out: 0 })))
const hasHourData = ref(false)
const loadingFirst = ref(true)
const ui = useUiStore()
let timer = null

function sample(key, v) {
  if (typeof v !== 'number') return
  const arr = trend[key]
  arr.push(v)
  if (arr.length > TREND_CAP) arr.shift()
}

const statCards = computed(() => [
  { label: '今日收', value: d.value.messages_in_today, color: '#06b6d4', points: trend.inToday },
  { label: '今日发', value: d.value.messages_out_today, color: '#4f46e5', points: trend.outToday },
  { label: '今日 Decision 调用', value: d.value.decision_calls_today, color: '#d97706', points: trend.decision },
  { label: '活跃任务', value: d.value.active_tasks, color: '#7c3aed', points: trend.tasks },
])

// 前端聚合：拉最近事件样本，按今天 0 点起分小时桶（收=MessageReceived，发=BubbleSent）
function aggregateHours(events) {
  const midnight = new Date()
  midnight.setHours(0, 0, 0, 0)
  const t0 = Math.floor(midnight.getTime() / 1000)
  const buckets = Array.from({ length: 24 }, () => ({ in: 0, out: 0 }))
  let any = false
  for (const e of events) {
    if (!e.ts || e.ts < t0) continue
    const h = new Date(e.ts * 1000).getHours()
    if (e.kind === 'MessageReceived') { buckets[h].in++; any = true }
    else if (e.kind === 'BubbleSent') { buckets[h].out++; any = true }
  }
  return { buckets, any }
}

async function load() {
  try {
    const { data } = await api.get('/dashboard')
    d.value = data
    ui.setMood(data.mood)
    sample('inToday', data.messages_in_today)
    sample('outToday', data.messages_out_today)
    sample('decision', data.decision_calls_today)
    sample('tasks', data.active_tasks)
  } catch { /* 401 由拦截器处理 */ }
  try {
    const { data } = await api.get('/events?limit=500')
    recent.value = data.events.slice(0, 8)
    const { buckets, any } = aggregateHours(data.events)
    hourSeries.value = buckets
    hasHourData.value = any
  } catch { /* 同上 */ }
  loadingFirst.value = false
}

// WS 实时增量：事件到 → 对应卡片 +1，事件流前插；不再等 30s 轮询
function onWsEvent(ev) {
  if (!ev || !ev.kind) return
  const now = Math.floor(Date.now() / 1000)
  if (ev.kind === 'MessageReceived') {
    d.value.messages_in_today = (d.value.messages_in_today ?? 0) + 1
    sample('inToday', d.value.messages_in_today)
  } else if (ev.kind === 'BubbleSent') {
    d.value.messages_out_today = (d.value.messages_out_today ?? 0) + 1
    sample('outToday', d.value.messages_out_today)
  } else if (ev.kind === 'DecisionMade') {
    d.value.decision_calls_today = (d.value.decision_calls_today ?? 0) + 1
    sample('decision', d.value.decision_calls_today)
    if (ev.payload?.mood) ui.setMood(ev.payload.mood)
  } else if (ev.kind === 'TaskCreated') {
    d.value.active_tasks = (d.value.active_tasks ?? 0) + 1
    sample('tasks', d.value.active_tasks)
  } else if (ev.kind === 'TaskFinished') {
    d.value.active_tasks = Math.max(0, (d.value.active_tasks ?? 0) - 1)
    sample('tasks', d.value.active_tasks)
  }
  // 事件流前插（去重 by 时间+kind 粗略）
  const item = { _k: Math.random(), kind: ev.kind, payload: ev.payload ?? ev, ts: ev.ts ?? now }
  recent.value = [item, ...recent.value].slice(0, 8)
  // 小时桶同步 +1（仅今天时段）
  const midnight = new Date(); midnight.setHours(0, 0, 0, 0)
  const t0 = Math.floor(midnight.getTime() / 1000)
  if (item.ts >= t0) {
    const h = new Date(item.ts * 1000).getHours()
    if (ev.kind === 'MessageReceived') { hourSeries.value[h].in++; hasHourData.value = true }
    else if (ev.kind === 'BubbleSent') { hourSeries.value[h].out++; hasHourData.value = true }
  }
}

let off = null
onMounted(() => {
  load()
  // 拉长到 60s：WS 主驱动，HTTP 仅作漂移校准
  timer = setInterval(load, 60000)
  off = subscribeWs(onWsEvent)
})
onUnmounted(() => { clearInterval(timer); off && off() })
</script>

<style scoped>
/* 事件行入场：新事件从顶部插入时轻微下落 */
.yt-fade-enter-active { transition: opacity 0.3s ease, transform 0.3s ease; }
.yt-fade-enter-from { opacity: 0; transform: translateY(-4px); }
.yt-fade-leave-active { transition: opacity 0.15s ease; position: absolute; }
.yt-fade-leave-to { opacity: 0; }

/* LIVE 实时标记：绿点 + 慢脉冲；断开时灰显 */
.live-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.6px;
  color: var(--yt-ink-3);
  background: var(--yt-card-border);
  transition: color 0.2s ease, background 0.2s ease;
}
.live-pill.on {
  color: #16a34a;
  background: rgba(22, 163, 74, 0.12);
}
.live-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  flex: none;
}
.live-pill.on .live-dot {
  animation: yt-live-pulse 1.6s ease-in-out infinite;
  box-shadow: 0 0 0 0 rgba(22, 163, 74, 0.4);
}
@keyframes yt-live-pulse {
  0%   { box-shadow: 0 0 0 0 rgba(22, 163, 74, 0.4); }
  70%  { box-shadow: 0 0 0 6px rgba(22, 163, 74, 0); }
  100% { box-shadow: 0 0 0 0 rgba(22, 163, 74, 0); }
}
@media (prefers-reduced-motion: reduce) {
  .live-pill.on .live-dot { animation: none; }
}
</style>
