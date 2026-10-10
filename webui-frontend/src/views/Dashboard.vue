<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">仪表盘</span>
      <span class="yt-page-sub">群聊活动与运行状态</span>
      <span class="spacer" />
      <span class="live-pill" :class="{ on: wsConnected }">{{ wsConnected ? '实时更新已连接' : '定时刷新中' }}</span>
      <n-button size="small" secondary :loading="refreshing" @click="load">刷新数据</n-button>
    </div>
    <n-alert v-if="loadError" type="warning" style="margin-bottom: 16px">{{ loadError }}</n-alert>
    <div class="dashboard-status">
      <span class="status-indicator" :class="{ connected: d.adapter_connected }" />
      <div><strong>{{ d.adapter_connected == null ? '连接状态待确认' : d.adapter_connected ? 'QQ 适配器已连接' : 'QQ 适配器离线' }}</strong><span class="status-note">{{ d.adapter_connected ? '消息通道可用' : '请在平台连接页检查接入状态' }}</span></div>
      <div class="status-meta">运行时长 <strong>{{ fmtUptime(d.uptime_secs) }}</strong></div>
      <div class="status-meta">当前情绪 <strong>{{ d.mood ?? '—' }}</strong></div>
      <router-link to="/platform">查看连接</router-link>
    </div>
    <n-grid cols="1 s:2 l:4" :x-gap="16" :y-gap="16" responsive="screen">
      <n-gi v-for="s in statCards" :key="s.label">
        <n-card class="stat-card" size="small">
          <div class="stat-label">{{ s.label }}</div>
          <div class="stat-foot"><span class="stat-num"><CountUp v-if="s.value != null" :value="s.value" /><template v-else>—</template></span><Sparkline v-if="s.points.length > 1" :points="s.points" color="var(--yt-primary)" /></div>
          <div class="stat-sub">{{ s.hint }}</div>
        </n-card>
      </n-gi>
    </n-grid>
    <div class="dashboard-columns">
      <div class="dashboard-main">
        <n-card title="消息收发趋势" size="small">
          <template #header-extra><span class="chart-legend"><span><i class="legend-in" />收到</span><span><i class="legend-out" />发送</span></span></template>
          <p class="panel-note">按本地时间分小时统计，仅覆盖最近 500 条事件与本页收到的实时事件。</p>
          <yt-skeleton v-if="loadingFirst" :height="210" />
          <HourBars v-else-if="hasHourData" :hours="hourSeries" />
          <empty-state v-else title="暂无消息趋势" hint="收到消息后，这里会显示每小时的收发数量。" />
        </n-card>
        <n-card title="最近事件" size="small">
          <template #header-extra><router-link to="/trace">查看决策追踪</router-link></template>
          <yt-skeleton v-if="loadingFirst" :height="180" />
          <template v-else>
            <div v-if="recent.length">
              <div v-for="e in recent" :key="e.id ?? e._k" class="mini-event">
                <span class="mini-dot" :style="{ background: kindColor(e.kind) }" />
                <span class="mini-kind">{{ kindLabel(e.kind) }}</span>
                <span class="mini-text" :title="eventSummary(e)">{{ eventSummary(e) }}</span>
                <span class="mini-time">{{ fmtAgo(e.ts) }}</span>
              </div>
            </div>
            <empty-state v-else title="暂无事件" hint="消息、决策与任务变化会按时间出现在这里。" />
          </template>
        </n-card>
      </div>
      <n-card title="运行资源" class="resource-card" size="small">
        <div class="resource-block"><div class="resource-label">CPU 使用率<n-tag size="small" :type="cpuType">{{ d.cpu_percent == null ? '待采集' : cpuType === 'error' ? '高负载' : cpuType === 'warning' ? '需关注' : '正常' }}</n-tag></div><div class="resource-value">{{ d.cpu_percent == null ? '—' : fmtCpu(d.cpu_percent) }}</div><div class="resource-meter" role="img" :aria-label="d.cpu_percent == null ? 'CPU 使用率待采集' : `CPU 使用率 ${fmtCpu(d.cpu_percent)}`"><span :style="{ width: `${Math.min(100, Math.max(0, d.cpu_percent ?? 0))}%`, background: cpuType === 'error' ? 'var(--yt-danger)' : cpuType === 'warning' ? 'var(--yt-warning)' : 'var(--yt-primary)' }" /></div></div>
        <div class="resource-block"><div class="resource-label">进程内存</div><div class="resource-value">{{ d.mem_rss_bytes == null ? '—' : memMb }} <small>MB</small></div><p class="panel-note">系统总内存 {{ d.mem_total_bytes == null ? '—' : fmtNum(Math.round(d.mem_total_bytes / 1048576)) }} MB</p></div>
        <div class="resource-block"><div class="resource-label">今日模型用量</div><div class="resource-value">{{ d.tokens_today == null ? '—' : fmtNum(d.tokens_today) }} <small>token</small></div><dl class="resource-details"><div><dt>输入</dt><dd>{{ d.tokens_in_today == null ? '—' : fmtNum(d.tokens_in_today) }}</dd></div><div><dt>输出</dt><dd>{{ d.tokens_out_today == null ? '—' : fmtNum(d.tokens_out_today) }}</dd></div><div><dt>调用次数</dt><dd>{{ d.llm_calls_today ?? '—' }}</dd></div></dl></div>
        <p class="panel-note">事件实时更新；运行资源与用量每 60 秒校准。</p>
      </n-card>
    </div>
  </div>
</template>

<script>
// 会话级采样历史（模块作用域，切页不丢）：四个指标的近实时走势，30s 一个点
const TREND_CAP = 40
const trend = { inToday: [], outToday: [], decision: [], tasks: [], tokens: [], mem: [] }
</script>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { useUiStore } from '../store/ui'
import { kindColor, kindLabel, eventSummary, fmtAgo, fmtUptime } from '../fmt'
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
const refreshing = ref(false)
const loadError = ref('')
const ui = useUiStore()
let timer = null

function sample(key, v) {
  if (typeof v !== 'number') return
  const arr = trend[key]
  arr.push(v)
  if (arr.length > TREND_CAP) arr.shift()
}

const statCards = computed(() => [
  { label: '今日收到消息', value: d.value.messages_in_today, points: trend.inToday, hint: '收到的群聊与私聊消息' },
  { label: '今日发送回复', value: d.value.messages_out_today, points: trend.outToday, hint: '实际发送的回复气泡' },
  { label: '今日参与决策', value: d.value.decision_calls_today, points: trend.decision, hint: '模型判断是否参与的次数' },
  { label: '活跃任务', value: d.value.active_tasks, points: trend.tasks, hint: '当前尚未结束的任务' },
])

const memMb = computed(() => Math.round((d.value.mem_rss_bytes ?? 0) / 1048576))
const cpuType = computed(() => {
  const c = d.value.cpu_percent ?? 0
  if (c >= 80) return 'error'
  if (c >= 50) return 'warning'
  return 'default'
})
function fmtNum(n) {
  if (n == null) return '0'
  if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M'
  if (n >= 1e3) return (n / 1e3).toFixed(1) + 'k'
  return String(n)
}
function fmtCpu(c) {
  return (c ?? 0).toFixed(1) + '%'
}

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
  if (refreshing.value) return
  refreshing.value = true
  loadError.value = ''
  try {
    const { data } = await api.get('/dashboard')
    d.value = data
    ui.setMood(data.mood)
    sample('inToday', data.messages_in_today)
    sample('outToday', data.messages_out_today)
    sample('decision', data.decision_calls_today)
    sample('tasks', data.active_tasks)
    sample('tokens', data.tokens_today)
    sample('mem', Math.round((data.mem_rss_bytes ?? 0) / 1048576))
  } catch { loadError.value = d.value.uptime_secs == null ? '运行数据获取失败，请检查服务后刷新。' : '运行数据获取失败。显示的是上次成功加载的数据，请检查服务后刷新。' }
  try {
    const { data } = await api.get('/events?limit=500')
    recent.value = data.events.slice(0, 8)
    const { buckets, any } = aggregateHours(data.events)
    hourSeries.value = buckets
    hasHourData.value = any
  } catch { loadError.value = '事件数据获取失败，请检查服务后刷新。' }
  loadingFirst.value = false
  refreshing.value = false
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
.dashboard-status { display: flex; gap: 20px; align-items: center; padding: 20px 24px; margin-bottom: 20px; background: var(--yt-surface); border: 1px solid var(--yt-card-border); border-left: 4px solid var(--yt-primary); border-radius: 10px; flex-wrap: wrap; }
.status-indicator { width: 10px; height: 10px; border-radius: 50%; background: var(--yt-ink-3); flex: none; }
.status-indicator.connected { background: var(--yt-ok); }
.status-note { display: block; color: var(--yt-ink-3); font-size: 12px; }
.status-meta { color: var(--yt-ink-3); font-size: 12px; margin-left: auto; }
.status-meta strong { display: block; font-size: 14px; color: var(--yt-ink-1); font-weight: 500; }
a { color: var(--yt-primary); text-decoration: none; font-size: 13px; }
a:hover { text-decoration: underline; }
.dashboard-columns { display: grid; grid-template-columns: minmax(0, 1fr) 280px; gap: 20px; margin-top: 20px; align-items: start; }
.dashboard-main { display: grid; gap: 20px; min-width: 0; }
.live-pill { color: var(--yt-ink-3); font-size: 12px; }
.live-pill.on { color: var(--yt-ok); }
.chart-legend, .chart-legend > span { display: inline-flex; align-items: center; gap: 8px; font-size: 12px; color: var(--yt-ink-2); }
.chart-legend { gap: 18px; }
.chart-legend i { width: 12px; height: 12px; border-radius: 3px; }
.legend-in { background: var(--yt-chart-in); }
.legend-out { background: var(--yt-chart-out); }
.panel-note { margin: 0 0 16px; color: var(--yt-ink-3); font-size: 12px; line-height: 1.7; }
.resource-block { padding: 4px 0 22px; margin-bottom: 20px; border-bottom: 1px solid var(--yt-card-border); }
.resource-label { display: flex; justify-content: space-between; align-items: center; color: var(--yt-ink-2); font-size: 13px; }
.resource-value { margin: 12px 0; font-size: 28px; font-weight: 600; font-variant-numeric: tabular-nums; }
.resource-value small { font-size: 13px; font-weight: 400; color: var(--yt-ink-3); }
.resource-meter { height: 6px; border-radius: 3px; overflow: hidden; background: var(--yt-soft-bg); }
.resource-meter span { display: block; height: 100%; }
.resource-details { margin: 0; font-size: 13px; }
.resource-details > div { display: flex; justify-content: space-between; margin-top: 8px; }
.resource-details dt { color: var(--yt-ink-3); }
.resource-details dd { margin: 0; font-variant-numeric: tabular-nums; }
@media (max-width: 1100px) { .dashboard-columns { grid-template-columns: 1fr; } }
@media (max-width: 800px) { .dashboard-status { padding: 16px; gap: 12px; } .status-meta { margin-left: 0; } }
</style>
