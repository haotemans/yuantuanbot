<template>
  <div>
    <n-grid :cols="4" :x-gap="12" style="max-width: 960px">
      <n-gi><n-card><n-statistic label="今日收" :value="d.messages_in_today ?? '—'" /></n-card></n-gi>
      <n-gi><n-card><n-statistic label="今日发" :value="d.messages_out_today ?? '—'" /></n-card></n-gi>
      <n-gi><n-card><n-statistic label="今日 Decision 调用（成本）" :value="d.decision_calls_today ?? '—'" /></n-card></n-gi>
      <n-gi><n-card><n-statistic label="活跃任务" :value="d.active_tasks ?? '—'" /></n-card></n-gi>
    </n-grid>
    <n-card style="max-width: 960px; margin-top: 12px">
      <n-space align="center">
        <n-tag :type="d.adapter_connected ? 'success' : 'error'">
          适配器：{{ d.adapter_connected ? '已连接' : '离线（>30s 无活性）' }}
        </n-tag>
        <n-tag>mood：{{ d.mood }}</n-tag>
        <n-tag>运行时长：{{ fmtUptime(d.uptime_secs) }}</n-tag>
        <n-button size="tiny" @click="load">刷新</n-button>
      </n-space>
    </n-card>
  </div>
</template>

<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { useUiStore } from '../store/ui'

const d = ref({})
const ui = useUiStore()
let timer = null

function fmtUptime(s) {
  if (s == null) return '—'
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60)
  return h > 0 ? `${h}h${m}m` : `${m}m${s % 60}s`
}

async function load() {
  try {
    const { data } = await api.get('/dashboard')
    d.value = data
    ui.setMood(data.mood)
  } catch { /* 401 由拦截器处理 */ }
}

onMounted(() => { load(); timer = setInterval(load, 30000) })
onUnmounted(() => clearInterval(timer))
</script>
