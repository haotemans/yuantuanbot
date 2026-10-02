<template>
  <div>
    <n-space style="margin-bottom: 8px">
      <n-select v-model:value="ownerType" :options="typeOptions" clearable placeholder="owner_type" size="small" style="width: 140px" />
      <n-input v-model:value="ownerId" placeholder="owner_id（如 p_2001 / 555666）" size="small" style="width: 220px" @keyup.enter="load" />
      <n-button size="small" @click="load">查询</n-button>
    </n-space>
    <n-tabs type="line" animated>
      <n-tab-pane name="long" tab="长期记忆">
        <n-data-table :columns="memCols" :data="memories" size="small" :pagination="{ pageSize: 20 }" />
      </n-tab-pane>
      <n-tab-pane name="daily" tab="每日摘要">
        <n-data-table :columns="sumCols" :data="summaries" size="small" :pagination="{ pageSize: 20 }" />
      </n-tab-pane>
    </n-tabs>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'

const ownerType = ref(null)
const ownerId = ref('')
const memories = ref([])
const summaries = ref([])
const typeOptions = ['person', 'chat', 'self'].map((s) => ({ label: s, value: s }))

const memCols = [
  { title: 'owner', key: 'owner', render: (r) => `${r.owner_type} / ${r.owner_id}` },
  { title: '内容', key: 'content', ellipsis: { tooltip: true } },
  { title: '来源', key: 'source', width: 130 },
  { title: '更新于', key: 'updated_at', width: 170, render: (r) => fmt(r.updated_at) },
]
const sumCols = [
  { title: 'date', key: 'date', width: 110 },
  { title: 'owner', key: 'owner', width: 220, render: (r) => `${r.owner_type} / ${r.owner_id}` },
  { title: '摘要', key: 'summary', ellipsis: { tooltip: true } },
  { title: '区间', key: 'range', width: 140, render: (r) => (r.msg_id_end ? `${r.msg_id_start}~${r.msg_id_end}` : '—') },
]

function fmt(ts) { return ts ? new Date(ts * 1000).toLocaleString() : '—' }

async function load() {
  const p = new URLSearchParams({ limit: '200' })
  if (ownerType.value) p.set('owner_type', ownerType.value)
  if (ownerId.value) p.set('owner_id', ownerId.value)
  const [m, s] = await Promise.all([
    api.get(`/memories?${p}`), api.get(`/summaries?${p}`),
  ])
  memories.value = m.data.memories
  summaries.value = s.data.summaries
}
onMounted(load)
</script>
