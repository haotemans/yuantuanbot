<template>
  <n-grid :cols="3" :x-gap="12">
    <n-gi span="2">
      <div ref="el" style="height: calc(100vh - 110px); border: 1px solid #e5e7eb; border-radius: 8px"></div>
    </n-gi>
    <n-gi>
      <n-card v-if="sel" size="small" :title="`${sel.id}「${sel.name}」`">
        <div style="font-size: 13px; line-height: 1.9">
          <div>首次见：{{ fmt(sel.first_seen) }}</div>
          <div>最近见：{{ fmt(sel.last_seen) }}</div>
          <n-divider style="margin: 8px 0" />
          <div v-for="e in relatedEdges" :key="e.from + '→' + e.to">
            {{ e.from === sel.id ? '→' : '←' }} {{ e.from === sel.id ? e.to : e.from }}：
            trust {{ e.trust.toFixed(2) }} / familiar {{ e.familiar.toFixed(2) }}
          </div>
          <n-empty v-if="!relatedEdges.length" description="暂无关系边" />
        </div>
      </n-card>
      <n-empty v-else description="点节点看详情" />
    </n-gi>
  </n-grid>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { Network } from 'vis-network/standalone'
import { api } from '../api'

const el = ref(null)
const data = ref({ nodes: [], edges: [] })
const sel = ref(null)

const relatedEdges = computed(() =>
  data.value.edges.filter((e) => sel.value && (e.from === sel.value.id || e.to === sel.value.id))
)

function fmt(ts) { return ts ? new Date(ts * 1000).toLocaleString() : '—' }

onMounted(async () => {
  const { data: d } = await api.get('/relations')
  data.value = d
  const nodes = d.nodes.map((n) => ({
    id: n.id,
    label: n.name || n.id,
    color: n.id === 'self' ? '#f59e0b' : '#60a5fa',
    shape: n.id === 'self' ? 'box' : 'dot',
    _raw: n,
  }))
  const edges = d.edges.map((e) => ({
    from: e.from,
    to: e.to,
    width: 1 + e.familiar * 4,
    color: { color: e.from === 'self' || e.to === 'self' ? '#f59e0b' : '#94a3b8', opacity: 0.8 },
    title: `trust ${e.trust.toFixed(2)} familiar ${e.familiar.toFixed(2)}`,
    arrows: 'to',
  }))
  const net = new Network(el.value, { nodes, edges }, {
    physics: { stabilization: true },
    edges: { smooth: true },
  })
  net.on('click', (p) => {
    sel.value = p.nodes.length ? d.nodes.find((n) => n.id === p.nodes[0]) : null
  })
})
</script>
