<template>
  <div class="yt-page">
    <div class="yt-page-head"><span class="yt-page-title">关系网</span><span class="yt-page-sub">{{ data.nodes.length }} 个人物，{{ data.edges.length }} 条关系</span><span class="spacer" /><n-select :value="sel?.id" :options="data.nodes.map(n => ({ label: n.name || n.id, value: n.id }))" clearable placeholder="选择人物查看详情" style="width: 220px" @update:value="selectPerson" /><n-button size="small" secondary :disabled="!hasNodes" @click="net?.fit({ animation: false })">适应画布</n-button></div>
    <div class="relations-layout">
    <div class="relations-main">
      <n-card size="small" title="人物关系图">
        <template #header-extra>
          <span style="font-size: 12px; color: var(--yt-text-dim)">悬停查看详情 · 点击节点锁定右侧信息</span>
        </template>
        <div class="graph-legend"><span><i class="self-node" />云团</span><span><i class="peer-node" />群友</span><span>节点大小：关系数量</span><span>线条粗细：熟悉度 · 箭头：关系方向</span></div>
        <div v-show="hasNodes" ref="el" class="graph-canvas" />
        <empty-state v-if="!hasNodes" title="还没有关系数据" hint="云团在群里和大家聊几句之后，第一条关系边就会出现在这里" />
      </n-card>
    </div>
    <div>
      <Transition name="slide-in" mode="out-in">
        <n-card v-if="sel" :key="sel.id" size="small" :title="`${sel.name || sel.id}`">
          <template #header-extra><span class="mono" style="font-size: 12px; color: var(--yt-text-dim)">{{ sel.id }}</span></template>
          <div style="font-size: 13px; line-height: 1.9">
            <div>首次见：{{ fmtTs(sel.first_seen) }}</div>
            <div>最近见：{{ fmtTs(sel.last_seen) }}</div>
            <n-divider style="margin: 8px 0" />
            <div v-for="e in relatedEdges" :key="e.from + '→' + e.to">
              {{ e.from === sel.id ? '→' : '←' }} {{ e.from === sel.id ? e.to : e.from }}：
              信任 {{ e.trust.toFixed(2) }} / 熟悉 {{ e.familiar.toFixed(2) }}
            </div>
            <n-empty v-if="!relatedEdges.length" description="暂无关系边" />
          </div>
        </n-card>
        <empty-state v-else title="未选中节点" hint="点击画布中的任意节点，这里滑入它的关系详情" />
      </Transition>
    </div>
  </div></div>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { DataSet, Network } from 'vis-network/standalone'
import { api } from '../api'
import { fmtTs } from '../fmt'
import { useUiStore } from '../store/ui'
import EmptyState from '../components/EmptyState.vue'

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

const selfColor = () => ui.dark ? '#8bb5ff' : '#285ec8'
const peerColor = () => ui.dark ? '#5a7396' : '#a4b8d8'
/** @type {any} */
let graphNodes = null
/** @type {any} */
let graphEdges = null
function selectPerson(id) {
  sel.value = data.value.nodes.find(n => n.id === id) || null
  if (net) net.selectNodes(id == null ? [] : [id])
}

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
    color: n.id === 'self' ? selfColor() : peerColor(),
    font: { color: n.id === 'self' ? (ui.dark ? '#202d43' : '#ffffff') : labelColor() },
    shape: n.id === 'self' ? 'box' : 'dot',
    size: baseSize(n.id),
    _base: baseSize(n.id),
  })))
  const edges = new DataSet(d.edges.map((e) => ({
    from: e.from,
    to: e.to,
    width: 1 + e.familiar * 4,
    color: { color: e.from === 'self' || e.to === 'self' ? selfColor() : '#94a3b8', opacity: 0.8 },
    title: `trust ${e.trust.toFixed(2)} / familiar ${e.familiar.toFixed(2)}`,
    arrows: 'to',
  })))

  graphNodes = nodes
  graphEdges = edges
  net = new Network(el.value, { nodes, edges }, {
    physics: { stabilization: true },
    edges: { smooth: true },
    interaction: { hover: true, tooltipDelay: 120 },
    nodes: {
      borderWidth: 2,
      font: { face: getComputedStyle(document.documentElement).getPropertyValue('--yt-font'), size: 13, color: labelColor(), strokeWidth: 3, strokeColor: 'rgba(0,0,0,0)' },
      shadow: { enabled: true, color: 'rgba(15,23,42,0.18)', size: 8, x: 0, y: 2 },
    },
  })

  // hover 辉光：节点放大一圈 + 主色光晕
  net.on('hoverNode', (p) => {
    const n = nodes.get(p.node)
    if (n) nodes.update({
      id: p.node, size: (n._base ?? 12) * 1.3, borderWidth: 3,
      shadow: { enabled: true, color: 'rgba(40, 94, 200, 0.2)', size: 22, x: 0, y: 0 },
    })
  })
  net.on('blurNode', (p) => {
    const n = nodes.get(p.node)
    if (n) nodes.update({
      id: p.node, size: n._base ?? 12, borderWidth: 2,
      shadow: { enabled: true, color: 'rgba(15,23,42,0.18)', size: 8, x: 0, y: 2 },
    })
  })
  net.on('click', (p) => {
    sel.value = p.nodes.length ? d.nodes.find((n) => n.id === p.nodes[0]) : null
  })
})

onUnmounted(() => { net?.destroy(); net = null })

// 暗色切换时同步节点文字颜色（画布底色由 CSS 变量接管）
watch(() => ui.dark, () => {
  if (!net) return
  net.setOptions({ nodes: { font: { color: labelColor() } } })
  graphNodes.update(data.value.nodes.map(n => ({ id: n.id, color: n.id === 'self' ? selfColor() : peerColor(), font: { color: n.id === 'self' ? (ui.dark ? '#202d43' : '#ffffff') : labelColor() } })))
  graphEdges.update(graphEdges.get().map(e => ({ id: e.id, color: { color: e.from === 'self' || e.to === 'self' ? selfColor() : '#94a3b8', opacity: 0.8 } })))
})
</script>

<style scoped>
.relations-layout { display: grid; grid-template-columns: minmax(0, 1fr) 320px; gap: 20px; }
.relations-main { min-width: 0; }
.graph-legend { display: flex; flex-wrap: wrap; gap: 12px 18px; align-items: center; font-size: 12px; color: var(--yt-ink-2); margin-bottom: 16px; }
.graph-legend span { display: inline-flex; align-items: center; gap: 6px; }
.graph-legend i { width: 10px; height: 10px; display: inline-block; }
.self-node { background: var(--yt-primary); border-radius: 2px; }
.peer-node { background: var(--yt-chart-in); border-radius: 50%; }
.graph-canvas { min-height: 360px; height: calc(100vh - 290px); }
@media (max-width: 1100px) { .relations-layout { grid-template-columns: 1fr; } }
</style>
