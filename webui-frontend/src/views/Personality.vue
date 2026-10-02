<template>
  <n-grid :cols="2" :x-gap="12">
    <n-gi>
      <n-card title="人格编辑器" size="small">
        <n-input v-model:value="draft" type="textarea" :rows="16" placeholder="人设提示词全文" />
        <n-input v-model:value="note" size="small" placeholder="修改说明（note）" style="margin: 8px 0" />
        <n-space>
          <n-button type="primary" size="small" :loading="saving" @click="saveNew">保存为新版本</n-button>
          <span v-if="msg" :style="{ color: ok ? '#16a34a' : '#dc2626', fontSize: '12px' }">{{ msg }}</span>
        </n-space>
      </n-card>
      <n-card v-if="diffLines" title="版本对照（左旧右新）" size="small" style="margin-top: 12px">
        <div style="font-family: monospace; font-size: 12px; max-height: 320px; overflow: auto">
          <div v-for="(l, i) in diffLines" :key="i" :style="{ background: l.bg, whiteSpace: 'pre-wrap' }">
            <span style="color: var(--yt-text-dim)">{{ l.tag }}</span> {{ l.text }}
          </div>
        </div>
      </n-card>
    </n-gi>
    <n-gi>
      <n-card title="版本时间线" size="small">
        <n-timeline>
          <n-timeline-item v-for="v in versions" :key="v.version_no"
            :type="v.active ? 'success' : 'default'"
            :title="`v${v.version_no}${v.active ? '（active）' : ''}`"
            :content="`${v.note || '—'} · ${fmt(v.created_at)} · ${v.content_len} 字`">
            <n-space size="small" style="margin-top: 4px">
              <n-checkbox :checked="diffSel.includes(v.version_no)" @update:checked="(c) => toggleDiff(v.version_no, c)">参比</n-checkbox>
              <n-button size="tiny" @click="loadOne(v.version_no)">载入此版本编辑</n-button>
              <n-popconfirm @positive-click="rollback(v.version_no)">
                <template #trigger><n-button size="tiny" type="warning">回滚到此版</n-button></template>
                生成新版本（内容=v{{ v.version_no }}），历史线性向前，确定？
              </n-popconfirm>
            </n-space>
          </n-timeline-item>
        </n-timeline>
      </n-card>
    </n-gi>
  </n-grid>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'

const versions = ref([])
const draft = ref('')
const note = ref('')
const saving = ref(false)
const msg = ref('')
const ok = ref(false)
const diffSel = ref([])
const diffLines = ref(null)
const contents = ref({})

function fmt(ts) { return ts ? new Date(ts * 1000).toLocaleString() : '—' }

async function load() {
  const { data } = await api.get('/personality/versions')
  versions.value = data.versions
  const active = data.versions.find((v) => v.active)
  if (active && !draft.value) loadOne(active.version_no)
}
async function loadOne(no) {
  const { data } = await api.get(`/personality/versions/${no}`)
  draft.value = data.content
  contents.value[no] = data.content
}
async function saveNew() {
  if (!draft.value.trim()) { ok.value = false; msg.value = '内容为空'; return }
  saving.value = true
  try {
    const { data } = await api.post('/personality/versions', { content: draft.value, note: note.value })
    ok.value = true
    msg.value = `已保存 v${data.version_no} 并激活`
    note.value = ''
    await load()
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}
async function rollback(no) {
  try {
    const { data } = await api.post(`/personality/rollback/${no}`)
    ok.value = true
    msg.value = `已回滚生成 v${data.version_no}`
    await load()
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '回滚失败'
  }
}
async function toggleDiff(no, checked) {
  const s = diffSel.value.filter((x) => x !== no)
  if (checked) s.push(no)
  diffSel.value = s.slice(-2)
  if (diffSel.value.length === 2) {
    const [a, b] = [...diffSel.value].sort((x, y) => x - y)
    for (const n of [a, b]) if (!contents.value[n]) {
      const { data } = await api.get(`/personality/versions/${n}`)
      contents.value[n] = data.content
    }
    diffLines.value = lineDiff(contents.value[a], contents.value[b])
  } else {
    diffLines.value = null
  }
}

// 简单行级对照（同位行比对，行数不等补标 + 新增/缺失）
function lineDiff(a, b) {
  const A = a.split('\n'); const B = b.split('\n')
  const n = Math.max(A.length, B.length)
  const out = []
  for (let i = 0; i < n; i++) {
    if (A[i] === B[i]) out.push({ tag: ' ', text: A[i] ?? '', bg: 'transparent' })
    else if (A[i] !== undefined && B[i] !== undefined) out.push({ tag: '~', text: `- ${A[i]}  →  + ${B[i]}`, bg: 'rgba(245,158,11,0.15)' })
    else if (A[i] !== undefined) out.push({ tag: '-', text: A[i], bg: 'rgba(220,38,38,0.12)' })
    else out.push({ tag: '+', text: B[i], bg: 'rgba(22,163,74,0.12)' })
  }
  return out
}

onMounted(load)
</script>
