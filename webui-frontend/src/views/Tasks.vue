<template>
  <n-grid :cols="2" :x-gap="12">
    <n-gi>
      <n-space style="margin-bottom: 8px">
        <n-select v-model:value="state" :options="stateOptions" clearable placeholder="state" size="small" style="width: 140px" @update:value="load" />
        <n-button size="small" @click="load">刷新任务</n-button>
      </n-space>
      <n-skeleton v-if="!firstLoaded" text :repeat="6" />
      <n-list v-else-if="list.length" bordered hoverable clickable style="max-height: calc(100vh - 130px); overflow: auto; border-radius: 10px">
        <n-list-item v-for="t in list" :key="t.task_id" @click="open(t)">
          <n-space size="small" align="center">
            <n-tag size="tiny" :type="stateTag(t.state)" round>{{ t.state }}</n-tag>
            <span>{{ t.goal }}</span>
            <span class="mono" style="color: var(--yt-text-dim); font-size: 12px">{{ t.used_calls }}/{{ t.budget_max_calls }}次</span>
          </n-space>
        </n-list-item>
      </n-list>
      <empty-state v-else title="暂无任务" hint="云团决定 start_task 时任务会出现在这里；也可以换个状态筛选看看" />
    </n-gi>
    <n-gi>
      <n-card v-if="cur" :title="`${cur.task_id} · ${cur.goal}`" size="small">
        <n-timeline>
          <n-timeline-item v-for="e in events" :key="e.id" :title="`#${e.seq} ${e.kind}`" :content="textOf(e)" :time="fmtTs(e.ts)" />
        </n-timeline>
        <n-empty v-if="!events.length" description="无流水" />
      </n-card>
      <empty-state v-else title="未选中任务" hint="点击左侧任意任务，这里回放它的执行时间轴" />
    </n-gi>
  </n-grid>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'

const list = ref([])
const cur = ref(null)
const events = ref([])
const state = ref(null)
const firstLoaded = ref(false)
const stateOptions = ['pending', 'running', 'finished', 'failed', 'archived'].map((s) => ({ label: s, value: s }))

function stateTag(s) {
  return ({ running: 'success', finished: 'info', failed: 'error', archived: 'default', pending: 'warning' })[s] || 'default'
}
function fmtTs(ts) { return new Date(ts * 1000).toLocaleString() }
function textOf(e) {
  if (e.payload == null) return ''
  return typeof e.payload === 'string' ? e.payload : JSON.stringify(e.payload)
}

async function load() {
  const params = new URLSearchParams({ limit: '100' })
  if (state.value) params.set('state', state.value)
  const { data } = await api.get(`/tasks?${params}`)
  list.value = data.tasks
  firstLoaded.value = true
}
async function open(t) {
  cur.value = t
  const { data } = await api.get(`/tasks/${t.task_id}/events`)
  events.value = data.events
}
onMounted(load)
</script>
