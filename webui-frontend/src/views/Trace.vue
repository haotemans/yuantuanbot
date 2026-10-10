<template>
  <div>
    <div class="yt-page-head"><span class="yt-page-title">决策追踪</span><span class="yt-page-sub">查看事件、模型建议与最终动作</span></div>
    <div class="yt-toolbar">
      <n-select v-model:value="kind" :options="kindOptions" clearable placeholder="选择事件类型" size="small" style="width: 200px" @update:value="reset" />
      <n-input v-model:value="chat" placeholder="按会话 ID 筛选" size="small" style="width: 180px" @keyup.enter="reset" />
      <n-button size="small" type="primary" secondary @click="reset">筛选事件</n-button>
      <span class="spacer" />
      <n-tag size="small" :type="wsConnected ? 'success' : 'error'" round>ws · {{ wsConnected ? '实时' : '断开' }}</n-tag>
    </div>
    <div class="trace-workspace">
      <section class="trace-events">
        <div v-if="items.length" class="tl">
          <div v-for="e in items" :key="e.id ?? e._k" class="tl-item" :class="{ active: sel === e }" @click="sel = e">
            <div class="tl-rail"><span class="tl-dot" :style="{ background: kindColor(e.kind) }" /></div>
            <div class="tl-card" :class="{ 'tl-decision': e.kind === 'DecisionMade' }"
                 :style="e.kind === 'DecisionMade' ? { '--action-c': actionColor(e.payload?.action) } : {}">
              <div class="tl-head">
                <span class="tl-badge" :style="badgeStyle(e.kind)">{{ e.kind }}</span>
                <span v-if="e.kind === 'DecisionMade'" class="tl-badge"
                      :style="badgeStyleFor(e.payload?.action, actionColor(e.payload?.action))">
                  {{ e.payload?.action }}
                </span>
                <span class="tl-time">{{ fmtAgo(e.ts) }}</span>
              </div>
              <div class="tl-text">{{ eventSummary(e) }}</div>
              <div v-if="e.payload?.chat_id" class="tl-chat">{{ e.payload.chat_id }}</div>
            </div>
          </div>
        </div>
        <empty-state v-else title="没有匹配的事件" hint="试着放宽筛选条件，或在群里发一条消息触发一条事件" />
        <n-button block size="small" secondary style="margin-top: 8px" :loading="loading" @click="loadMore">加载更早的事件</n-button>
      </section>
      <aside class="trace-detail">
        <template v-if="sel">
          <n-space size="small" style="margin-bottom: 8px">
            <n-tag size="small" round :color="badgeStyleObj(sel.kind)">{{ sel.kind }}</n-tag>
            <span v-if="sel.ts" class="tl-chat" style="margin-top: 0">{{ fmtTs(sel.ts) }}</span>
          </n-space>
          <template v-if="sel.kind === 'DecisionMade'">
            <n-space size="small" style="margin-bottom: 8px">
              <n-tag size="small" round :color="actionTagColor(sel.payload.action)">action · {{ sel.payload.action }}</n-tag>
              <n-tag size="small" round>mood · {{ sel.payload.mood }}</n-tag>
              <n-tag v-if="sel.payload.fallback" size="small" round type="error">fallback</n-tag>
            </n-space>
            <n-alert type="info" style="margin-bottom: 10px">reason · {{ sel.payload.reason }}</n-alert>
            <template v-if="sel.payload.policy">
              <n-space size="small" style="margin-bottom: 8px">
                <n-tag size="small">模型建议 · {{ sel.payload.policy.suggested_action }}</n-tag>
                <n-tag size="small">回复方式 · {{ sel.payload.reply_mode }}</n-tag>
                <n-tag v-if="sel.payload.policy.evidence_valid === false" size="small" type="warning">消息依据未通过</n-tag>
              </n-space>
              <n-alert v-if="sel.payload.policy.notes?.length" type="warning" style="margin-bottom: 10px">{{ sel.payload.policy.notes.join('；') }}</n-alert>
            </template>
          </template>
          <json-view :data="sel.payload" />
        </template>
        <empty-state v-else title="未选中事件" hint="点击左侧任意一条事件，这里展示它的完整 payload 与语法高亮" />
      </aside>
    </div>
  </div>
</template>

<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { subscribeWs, wsConnected } from '../ws'
import { kindColor, actionColor, eventSummary, fmtAgo, fmtTs } from '../fmt'
import JsonView from '../components/JsonView.vue'
import EmptyState from '../components/EmptyState.vue'

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
  return badgeStyleFor(k, kindColor(k))
}
function badgeStyleFor(_k, c) {
  return { color: c, background: `color-mix(in srgb, ${c} 10%, transparent)`, border: `1px solid color-mix(in srgb, ${c} 30%, transparent)` }
}
function badgeStyleObj(k) {
  const c = kindColor(k)
  return { color: `color-mix(in srgb, ${c} 10%, transparent)`, textColor: c, borderColor: `color-mix(in srgb, ${c} 30%, transparent)` }
}
function actionTagColor(a) {
  const c = actionColor(a)
  return { color: `color-mix(in srgb, ${c} 10%, transparent)`, textColor: c, borderColor: `color-mix(in srgb, ${c} 30%, transparent)` }
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

<style scoped>
.trace-workspace { display: grid; grid-template-columns: minmax(0, 1fr) 420px; gap: 20px; align-items: start; }
.trace-events { min-width: 0; }
.trace-detail { min-width: 0; background: var(--yt-surface); border: 1px solid var(--yt-card-border); border-radius: 12px; padding: 20px; }
.tl { max-height: calc(100vh - 255px); }
.tl-head { flex-wrap: wrap; }
@media (max-width: 1100px) { .trace-workspace { grid-template-columns: 1fr; } .tl { max-height: 420px; } }
</style>
