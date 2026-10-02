<template>
  <div class="yt-page">
    <n-grid cols="1 s:2 l:4" :x-gap="14" :y-gap="14" responsive="screen">
      <n-gi v-for="s in statCards" :key="s.label">
        <n-card class="stat-card" size="small">
          <div class="stat-label"><span class="stat-chip" :style="{ background: s.color }" />{{ s.label }}</div>
          <div class="stat-foot">
            <span class="stat-num">{{ s.value ?? '—' }}</span>
            <Sparkline :points="s.points" :color="s.color" />
          </div>
        </n-card>
      </n-gi>
    </n-grid>

    <n-card size="small" style="margin-top: 14px">
      <n-space align="center" :size="12" style="flex-wrap: wrap">
        <n-tag :type="d.adapter_connected ? 'success' : 'error'" round>
          适配器 · {{ d.adapter_connected ? '已连接' : '离线（>30s 无活性）' }}
        </n-tag>
        <n-tag round>mood · {{ d.mood }}</n-tag>
        <n-tag round>运行时长 · {{ fmtUptime(d.uptime_secs) }}</n-tag>
        <n-button size="tiny" secondary style="margin-left: auto" @click="load">刷新</n-button>
      </n-space>
    </n-card>

    <n-card title="最近事件" size="small" style="margin-top: 14px">
      <div v-if="recent.length">
        <div v-for="(e, i) in recent" :key="e.id ?? i" class="mini-event">
          <span class="mini-dot" :style="{ background: kindColor(e.kind) }" />
          <span class="mini-kind" :style="{ color: kindColor(e.kind) }">{{ e.kind }}</span>
          <span class="mini-text">{{ eventSummary(e) }}</span>
          <span class="mini-time">{{ fmtAgo(e.ts) }}</span>
        </div>
      </div>
      <n-empty v-else class="yt-empty" description="还没有事件，等群里有人说话就有了" />
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

/** @type {import('vue').Ref<Record<string, any>>} */
const d = ref({})
/** @type {import('vue').Ref<any[]>} */
const recent = ref([])
const ui = useUiStore()
let timer = null

function sample(key, v) {
  if (typeof v !== 'number') return
  const arr = trend[key]
  arr.push(v)
  if (arr.length > TREND_CAP) arr.shift()
}

const statCards = computed(() => [
  { label: '今日收', value: d.value.messages_in_today, color: '#2563eb', points: trend.inToday },
  { label: '今日发', value: d.value.messages_out_today, color: '#0ea5e9', points: trend.outToday },
  { label: '今日 Decision 调用（成本）', value: d.value.decision_calls_today, color: '#d97706', points: trend.decision },
  { label: '活跃任务', value: d.value.active_tasks, color: '#7c3aed', points: trend.tasks },
])

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
    const { data } = await api.get('/events?limit=8')
    recent.value = data.events
  } catch { /* 同上 */ }
}

onMounted(() => { load(); timer = setInterval(load, 30000) })
onUnmounted(() => clearInterval(timer))
</script>
