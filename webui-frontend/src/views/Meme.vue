<template>
  <div>
    <n-tabs type="line" animated @update:value="load">
      <n-tab-pane name="pending" :tab="`待审队列（${pending.length}）`">
        <empty-state v-if="!pending.length" title="待审队列已清空" hint="群里再偷到新图时会先进这里，等你收编" />
        <n-grid v-else :cols="5" :x-gap="10" :y-gap="10">
          <n-gi v-for="m in pending" :key="m.id">
            <n-card size="small">
              <meme-img :id="m.id" height="110px" />
              <n-input v-model:value="m._cat" size="tiny" placeholder="类别（如 开心）" style="margin: 6px 0" />
              <n-space size="small">
                <n-button size="tiny" type="success" @click="approve(m)">收编</n-button>
                <n-popconfirm @positive-click="reject(m)">
                  <template #trigger><n-button size="tiny" type="error">删除</n-button></template>
                  删文件+删行，确定？
                </n-popconfirm>
              </n-space>
            </n-card>
          </n-gi>
        </n-grid>
      </n-tab-pane>
      <n-tab-pane name="active" :tab="`库（${active.length}）`">
        <n-space style="margin-bottom: 8px">
          <n-select v-model:value="cat" :options="catOptions" clearable placeholder="类别筛选" size="small" style="width: 160px" />
          <n-button size="small" @click="load">刷新图库</n-button>
        </n-space>
        <empty-state v-if="!shownActive.length" title="该分类下还没有表情包" hint="去「待审队列」收编几张，或换个分类筛选" />
        <n-grid v-else :cols="5" :x-gap="10" :y-gap="10">
          <n-gi v-for="m in shownActive" :key="m.id">
            <n-card size="small">
              <meme-img :id="m.id" height="110px" />
              <div style="font-size: 12px; margin-top: 4px">
                <n-tag size="tiny" round>{{ m.category }}</n-tag>
                用过 {{ m.use_count }} 次
              </div>
            </n-card>
          </n-gi>
        </n-grid>
      </n-tab-pane>
    </n-tabs>
  </div>
</template>

<script setup>
import { computed, h, onMounted, ref } from 'vue'
import { NImage } from 'naive-ui'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'

// 图片走 Bearer 头像不可直链 → fetch blob 转 objectURL
const MemeImg = (props) => {
  const src = ref('')
  api.get(`/meme-file/${props.id}`, { responseType: 'blob' }).then((r) => {
    src.value = URL.createObjectURL(r.data)
  })
  return () => h(NImage, { src: src.value, style: `height:${props.height}; object-fit: contain; width: 100%`, previewDisabled: !src.value })
}

const pending = ref([])
const active = ref([])
const cat = ref(null)

const catOptions = computed(() => [...new Set(active.value.map((m) => m.category))].map((c) => ({ label: c, value: c })))
const shownActive = computed(() => (cat.value ? active.value.filter((m) => m.category === cat.value) : active.value))

async function load() {
  const [p, a] = await Promise.all([
    api.get('/memes?status=pending'), api.get('/memes?status=active'),
  ])
  pending.value = p.data.memes.map((m) => ({ ...m, _cat: '' }))
  active.value = a.data.memes
}
async function approve(m) {
  await api.post(`/memes/${m.id}/approve`, { category: m._cat || undefined })
  await load()
}
async function reject(m) {
  await api.post(`/memes/${m.id}/reject`)
  await load()
}
onMounted(load)
</script>
