<template>
  <n-card title="知识库" style="max-width: 520px" size="small">
    <n-alert type="warning">功能默认关闭，二期开放（架构文档九章 + 十五章扩展模型：sqlite-vec 可插拔）。</n-alert>
    <n-descriptions :column="1" size="small" style="margin-top: 12px">
      <n-descriptions-item label="enabled">{{ s.enabled }}</n-descriptions-item>
      <n-descriptions-item label="docs">{{ s.docs }}</n-descriptions-item>
    </n-descriptions>
  </n-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'
const s = ref({ enabled: false, docs: 0 })
onMounted(async () => { s.value = (await api.get('/kb/status')).data })
</script>
