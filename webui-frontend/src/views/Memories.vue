<template>
  <div class="yt-page">
    <div class="yt-page-head"><span class="yt-page-title">记忆浏览</span><span class="yt-page-sub">按人物或群聊查找长期记忆与每日摘要</span></div>
    <n-space style="margin-bottom: 8px">
      <n-select v-model:value="ownerType" :options="typeOptions" clearable placeholder="记忆归属类型" size="small" style="width: 140px" />
      <n-input v-model:value="ownerId" placeholder="owner_id（如 p_2001 / 555666）" size="small" style="width: 220px" @keyup.enter="load" />
      <n-button size="small" type="primary" secondary @click="load">查询记忆</n-button>
    </n-space>
    <n-tabs type="line" animated>
      <n-tab-pane name="long" tab="长期记忆">
        <n-skeleton v-if="!firstLoaded" text :repeat="8" style="margin-top: 6px" />
        <n-data-table v-else :columns="memCols" :data="memories" size="small" :loading="loading" :pagination="{ pageSize: 20 }" />
      </n-tab-pane>
      <n-tab-pane name="daily" tab="每日摘要">
        <n-skeleton v-if="!firstLoaded" text :repeat="8" style="margin-top: 6px" />
        <n-data-table v-else :columns="sumCols" :data="summaries" size="small" :loading="loading" :pagination="{ pageSize: 20 }" />
      </n-tab-pane>
    </n-tabs>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'

const ownerType = ref(null)
const ownerId = ref('')
/** @type {import('vue').Ref<any[]>} */
const memories = ref([])
/** @type {import('vue').Ref<any[]>} */
const summaries = ref([])
const loading = ref(false)
const firstLoaded = ref(false)
const typeOptions = ['person', 'chat', 'self'].map((s) => ({ label: s, value: s }))

const memCols = [
  { title: '记忆归属', key: 'owner', render: (r) => `${r.owner_type} / ${r.owner_id}` },
  { title: '内容', key: 'content', ellipsis: { tooltip: true } },
  { title: '来源', key: 'source', width: 130 },
  { title: '更新于', key: 'updated_at', width: 170, render: (r) => fmt(r.updated_at) },
]
const sumCols = [
  { title: '日期', key: 'date', width: 110 },
  { title: '记忆归属', key: 'owner', width: 220, render: (r) => `${r.owner_type} / ${r.owner_id}` },
  { title: '摘要', key: 'summary', ellipsis: { tooltip: true } },
  { title: '区间', key: 'range', width: 140, render: (r) => (r.msg_id_end ? `${r.msg_id_start}~${r.msg_id_end}` : '—') },
]

function fmt(ts) { return ts ? new Date(ts * 1000).toLocaleString() : '—' }

async function load() {
  loading.value = true
  try {
    const p = new URLSearchParams({ limit: '200' })
    if (ownerType.value) p.set('owner_type', ownerType.value)
    if (ownerId.value) p.set('owner_id', ownerId.value)
    const [m, s] = await Promise.all([
      api.get(`/memories?${p}`), api.get(`/summaries?${p}`),
    ])
    memories.value = m.data.memories
    summaries.value = s.data.summaries
  } finally {
    loading.value = false
    firstLoaded.value = true
  }
}
onMounted(load)
</script>
