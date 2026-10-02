<template>
  <n-layout has-sider style="height: calc(100vh - 80px)">
    <n-layout-content content-style="padding: 8px;">
      <n-space style="margin-bottom: 8px">
        <n-select v-model:value="kind" :options="kindOptions" clearable placeholder="kind" size="small" style="width: 200px" @update:value="reset" />
        <n-input v-model:value="chat" placeholder="chat_id 过滤" size="small" style="width: 160px" @keyup.enter="reset" />
        <n-button size="small" @click="reset">查询</n-button>
        <n-tag size="small" :type="wsConnected ? 'success' : 'error'">ws {{ wsConnected ? '实时' : '断开' }}</n-tag>
      </n-space>
      <n-list bordered hoverable clickable style="max-height: calc(100vh - 140px); overflow: auto">
        <n-list-item v-for="e in items" :key="e.id ?? e._k" @click="sel = e">
          <n-space size="small" align="center">
            <n-tag size="tiny" :type="kindTag(e.kind)">{{ e.kind }}</n-tag>
            <span v-if="e.payload?.chat_id" style="color: #888">{{ e.payload.chat_id }}</span>
            <span v-if="e.kind === 'DecisionMade'" style="font-size: 12px">
              <b :style="{ color: e.payload.action === 'ignore' ? '#999' : '#16a34a' }">{{ e.payload.action }}</b>
              / {{ e.payload.mood }} / {{ e.payload.reason }}
            </span>
            <span v-else-if="e.payload?.text" style="font-size: 12px">{{ e.payload.text }}</span>
          </n-space>
        </n-list-item>
      </n-list>
      <n-button block size="small" style="margin-top: 6px" :loading="loading" @click="loadMore">加载更早</n-button>
    </n-layout-content>
    <n-layout-sider width="420" bordered content-style="padding: 8px;">
      <template v-if="sel">
        <n-space size="small" style="margin-bottom: 6px">
          <n-tag :type="kindTag(sel.kind)">{{ sel.kind }}</n-tag>
          <span v-if="sel.ts" style="color: #888; font-size: 12px">{{ fmtTs(sel.ts) }}</span>
        </n-space>
        <template v-if="sel.kind === 'DecisionMade'">
          <n-space size="small" style="margin-bottom: 6px">
            <n-tag :type="sel.payload.action === 'ignore' ? 'default' : 'success'">action: {{ sel.payload.action }}</n-tag>
            <n-tag>mood: {{ sel.payload.mood }}</n-tag>
            <n-tag v-if="sel.payload.fallback" type="error">fallback</n-tag>
          </n-space>
          <n-alert type="info" style="margin-bottom: 8px">reason：{{ sel.payload.reason }}</n-alert>
        </template>
        <n-code :code="pretty(sel.payload)" language="json" word-wrap style="font-size: 12px" />
      </template>
      <n-empty v-else description="点一条事件看详情" />
    </n-layout-sider>
  </n-layout>
</template>

<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { subscribeWs, wsConnected } from '../ws'

const items = ref([])
const sel = ref(null)
const kind = ref(null)
const chat = ref('')
const loading = ref(false)

const kindOptions = [
  'MessageReceived', 'DecisionMade', 'BubbleSent', 'ReplyInterrupted',
  'ConsolidationDone', 'MemoryWritten', 'MoodChanged', 'ConfigReloaded',
].map((k) => ({ label: k, value: k }))

function kindTag(k) {
  return ({ DecisionMade: 'success', MessageReceived: 'info', BubbleSent: 'warning', ReplyInterrupted: 'error' })[k] || 'default'
}
function fmtTs(ts) { return new Date(ts * 1000).toLocaleString() }
function pretty(v) { return JSON.stringify(v, null, 2) }

async function fetchPage(beforeId) {
  loading.value = true
  try {
    const params = new URLSearchParams({ limit: '50' })
    if (kind.value) params.set('kind', kind.value)
    if (chat.value) params.set('chat_id', chat.value)
    if (beforeId) params.set('before_id', String(beforeId))
    const { data } = await api.get(`/events?${params}`)
    return data.events
  } finally {
    loading.value = false
  }
}

async function reset() {
  items.value = await fetchPage()
}
async function loadMore() {
  const oldest = items.value[items.value.length - 1]
  if (!oldest?.id) return
  items.value = items.value.concat(await fetchPage(oldest.id))
}

let off = null
onMounted(async () => {
  await reset()
  off = subscribeWs((ev) => {
    if (kind.value && ev.kind !== kind.value) return
    if (chat.value && ev.chat_id !== chat.value && ev.payload?.chat_id !== chat.value) return
    items.value.unshift({ _k: Math.random(), kind: ev.kind, payload: ev, ts: Math.floor(Date.now() / 1000) })
    if (items.value.length > 300) items.value.pop()
  })
})
onUnmounted(() => off && off())
</script>
