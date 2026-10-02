<template>
  <n-grid :cols="3" :x-gap="14">
    <n-gi span="2">
      <n-card size="small" title="群友关系网">
        <template #header-extra>
          <span style="font-size: 12px; color: var(--yt-text-dim)">悬停查看详情 · 点击节点锁定右侧信息</span>
        </template>
        <div v-show="hasNodes" ref="el" class="graph-canvas" />
        <n-empty v-if="!hasNodes" class="yt-empty" description="还没有关系数据，先去群里聊几句吧" />
      </n-card>
    </n-gi>
    <n-gi>
      <n-card v-if="sel" size="small" :title="`${sel.id}「${sel.name}」`">
        <div style="font-size: 13px; line-height: 1.9">
          <div>首次见：{{ fmtTs(sel.first_seen) }}</div>
          <div>最近见：{{ fmtTs(sel.last_seen) }}</div>
          <n-divider style="margin: 8px 0" />
          <div v-for="e in relatedEdges" :key="e.from + '→' + e.to">
            {{ e.from === sel.id ? '→' : '←' }} {{ e.from === sel.id ? e.to : e.from }}：
            trust {{ e.trust.toFixed(2) }} / familiar {{ e.familiar.toFixed(2) }}
          </div>
          <n-empty v-if="!relatedEdges.length" description="暂无关系边" />
        </div>
      </n-card>
      <n-empty v-else class="yt-empty" description="点击节点查看详情" />
    </n-gi>
  </n-grid>
</template>

<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { DataSet, Network } from 'vis-network/standalone'
import { api } from '../api'
import { fmtTs } from '../fmt'
import { useUiStore } from '../store/ui'

const el = ref(null)
const hasNodes = ref(true)
/** @type {import('vue').Ref<any>} */
const sel = ref(null)
/** @type {import('vue').Ref<{ nodes: any[], edges: any[] }>} */
const data = ref({ nodes: [], edges: [] })
const ui = useUiStore()
/** @type {any} */
let net = null

const relatedEdges = computed(() =>
  data.value.edges.filter((e) => sel.value && (e.from === sel.value.id || e.to === sel.value.id))
)

const SELF_COLOR = '#f59e0b'
const PEER_COLOR = '#60a5fa'

// QQ 昵称是外部输入：tooltip 走 HTML 必须先转义
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c])

function labelColor() {
  return ui.dark ? '#cbd5e1' : '#334155'
}

onMounted(async () => {
  const { data: d } = await api.get('/relations')
  data.value = d
  hasNodes.value = d.nodes.length > 0
  if (!hasNodes.value) return

  const degree = {}
  for (const e of d.edges) {
    degree[e.from] = (degree[e.from] || 0) + 1
    degree[e.to] = (degree[e.to] || 0) + 1
  }
  const baseSize = (id) => 12 + Math.min(18, (degree[id] || 0) * 3)
  const tip = (n) =>
    `<b>${esc(n.name || n.id)}</b><br/>id：${esc(n.id)}<br/>首次见：${fmtTs(n.first_seen)}<br/>最近见：${fmtTs(n.last_seen)}`

  /** @type {any} vis DataSet 泛型默认成 id-only，此处字段远超其推断 */
  const nodes = new DataSet(d.nodes.map((n) => ({
    id: n.id,
    label: n.name || n.id,
    title: tip(n),
    color: n.id === 'self' ? SELF_COLOR : PEER_COLOR,
    shape: n.id === 'self' ? 'box' : 'dot',
    size: baseSize(n.id),
    _base: baseSize(n.id),
  })))
  const edges = new DataSet(d.edges.map((e) => ({
    from: e.from,
    to: e.to,
    width: 1 + e.familiar * 4,
    color: { color: e.from === 'self' || e.to === 'self' ? SELF_COLOR : '#94a3b8', opacity: 0.8 },
    title: `trust ${e.trust.toFixed(2)} / familiar ${e.familiar.toFixed(2)}`,
    arrows: 'to',
  })))

  net = new Network(el.value, { nodes, edges }, {
    physics: { stabilization: true },
    edges: { smooth: true },
    interaction: { hover: true, tooltipDelay: 120 },
    nodes: {
      borderWidth: 2,
      font: { size: 13, color: labelColor(), strokeWidth: 3, strokeColor: 'rgba(0,0,0,0)' },
      shadow: { enabled: true, color: 'rgba(15,23,42,0.18)', size: 8, x: 0, y: 2 },
    },
  })

  net.on('hoverNode', (p) => {
    const n = nodes.get(p.node)
    if (n) nodes.update({ id: p.node, size: (n._base ?? 12) * 1.35, borderWidth: 3 })
  })
  net.on('blurNode', (p) => {
    const n = nodes.get(p.node)
    if (n) nodes.update({ id: p.node, size: n._base ?? 12, borderWidth: 2 })
  })
  net.on('click', (p) => {
    sel.value = p.nodes.length ? d.nodes.find((n) => n.id === p.nodes[0]) : null
  })
})

// 暗色切换时同步节点文字颜色（画布底色由 CSS 变量接管）
watch(() => ui.dark, () => {
  if (net) net.setOptions({ nodes: { font: { color: labelColor() } } })
})
</script>
