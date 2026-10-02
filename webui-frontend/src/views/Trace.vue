<template>
  <n-layout has-sider style="height: calc(100vh - 90px)">
    <n-layout-content content-style="padding: 8px 12px 8px 8px;">
      <n-space style="margin-bottom: 10px" :size="10">
        <n-select v-model:value="kind" :options="kindOptions" clearable placeholder="kind" size="small" style="width: 200px" @update:value="reset" />
        <n-input v-model:value="chat" placeholder="chat_id 过滤" size="small" style="width: 160px" @keyup.enter="reset" />
        <n-button size="small" secondary @click="reset">查询</n-button>
        <n-tag size="small" :type="wsConnected ? 'success' : 'error'" round>ws · {{ wsConnected ? '实时' : '断开' }}</n-tag>
      </n-space>
      <div v-if="items.length" class="tl">
        <div v-for="e in items" :key="e.id ?? e._k" class="tl-item" :class="{ active: sel === e }" @click="sel = e">
          <div class="tl-rail"><span class="tl-dot" :style="{ background: kindColor(e.kind) }" /></div>
          <div class="tl-card">
            <div class="tl-head">
              <span class="tl-badge" :style="badgeStyle(e.kind)">{{ e.kind }}</span>
              <span class="tl-time">{{ fmtAgo(e.ts) }}</span>
            </div>
            <div class="tl-text">{{ eventSummary(e) }}</div>
            <div v-if="e.payload?.chat_id" class="tl-chat">{{ e.payload.chat_id }}</div>
          </div>
        </div>
      </div>
      <n-empty v-else class="yt-empty" description="没有匹配的事件，换个筛选条件试试" />
      <n-button block size="small" secondary style="margin-top: 8px" :loading="loading" @click="loadMore">加载更早</n-button>
    </n-layout-content>
    <n-layout-sider width="430" bordered content-style="padding: 12px;">
      <template v-if="sel">
        <n-space size="small" style="margin-bottom: 8px">
          <n-tag size="small" round :color="badgeStyleObj(sel.kind)">{{ sel.kind }}</n-tag>
          <span v-if="sel.ts" class="tl-chat" style="margin-top: 0">{{ fmtTs(sel.ts) }}</span>
        </n-space>
        <template v-if="sel.kind === 'DecisionMade'">
          <n-space size="small" style="margin-bottom: 8px">
            <n-tag size="small" round :type="sel.payload.action === 'ignore' ? 'default' : 'success'">action · {{ sel.payload.action }}</n-tag>
            <n-tag size="small" round>mood · {{ sel.payload.mood }}</n-tag>
            <n-tag v-if="sel.payload.fallback" size="small" round type="error">fallback</n-tag>
          </n-space>
          <n-alert type="info" style="margin-bottom: 10px">reason · {{ sel.payload.reason }}</n-alert>
        </template>
        <json-view :data="sel.payload" />
      </template>
      <n-empty v-else class="yt-empty" description="点击左侧事件查看详情" />
    </n-layout-sider>
  </n-layout>
</template>

<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { subscribeWs, wsConnected } from '../ws'
import { kindColor, eventSummary, fmtAgo, fmtTs } from '../fmt'
import JsonView from '../components/JsonView.vue'

/** @type {import('vue').Ref<any[]>} */
const items = ref([])
/** @type {import('vue').Ref<any>} */
const sel = ref(null)
const kind = ref(null)
const chat = ref('')
const loading = ref(false)

const kindOptions = [
  'MessageReceived', 'DecisionMade', 'BubbleSent', 'ReplyInterrupted',
  'ConsolidationDone', 'MemoryWritten', 'MoodChanged', 'ConfigReloaded',
].map((k) => ({ label: k, value: k }))

// kind 彩色徽标：主色 10% 底 + 主色字/边
function badgeStyle(k) {
  const c = kindColor(k)
  return { color: c, background: `${c}1a`, border: `1px solid ${c}55` }
}
function badgeStyleObj(k) {
  const c = kindColor(k)
  return { color: `${c}1a`, textColor: c, borderColor: `${c}55` }
}

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
