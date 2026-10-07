<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">插件</span>
      <span class="yt-page-sub">Tools · Skills · MCP（编译期加载，启禁需重启）</span>
      <span class="spacer" />
      <n-button size="tiny" secondary @click="load">刷新</n-button>
    </div>

    <n-alert v-if="dirty.size" type="warning" :bordered="false" style="margin-bottom: 14px">
      {{ dirty.size }} 个插件的启禁状态待重启后生效：{{ [...dirty].join('、') }}
    </n-alert>

    <div v-if="plugins.length" class="cap-grid">
      <PluginCard
        v-for="p in plugins"
        :key="p.name"
        :plugin="p"
        :dirty="dirty.has(p.name)"
        @toggle="toggle"
        @open="openDetail"
      />
    </div>
    <n-card v-else size="small">
      <empty-state title="没有插件"
                   hint="在 plugins/<name>/ 下加一个独立 cargo crate，参考 plugins/README.md 与 plugins/hello/" />
    </n-card>

    <!-- 详情 modal -->
    <n-modal v-model:show="showDetail" preset="card" :title="detail?.name" style="max-width: 640px">
      <template #header-extra>
        <n-tag size="tiny" round :bordered="false" type="info" v-if="detail?.version">v{{ detail.version }}</n-tag>
      </template>
      <div v-if="detail" class="detail">
        <div class="detail-row">
          <span class="k">状态</span>
          <span class="v">
            <n-tag size="tiny" round :bordered="false" type="success" v-if="detail.enabled && detail.loaded">已加载</n-tag>
            <n-tag size="tiny" round :bordered="false" type="warning" v-else-if="detail.enabled && !detail.loaded">已启用·待重启</n-tag>
            <n-tag size="tiny" round :bordered="false" v-else>已禁用</n-tag>
          </span>
        </div>
        <div class="detail-row">
          <span class="k">描述</span>
          <span class="v">{{ detail.description || '（无）' }}</span>
        </div>
        <div class="detail-row" v-if="detail.data_dir">
          <span class="k">数据目录</span>
          <span class="v mono">{{ detail.data_dir }}</span>
        </div>
        <div class="detail-block" v-if="(detail.skills || []).length">
          <div class="cap-block-label">Skills（{{ detail.skills.length }}）</div>
          <div class="cap-block-list">
            <div v-for="s in detail.skills" :key="s.name" class="cap-block-item">
              <span class="chip skill mono">⚡ {{ s.name }}</span>
              <span class="cap-block-desc">{{ s.description }}</span>
            </div>
          </div>
        </div>
        <div class="detail-block" v-if="(detail.tools || []).length">
          <div class="cap-block-label">Tools（{{ detail.tools.length }}）</div>
          <div class="cap-block-list">
            <div v-for="t in detail.tools" :key="t.name" class="cap-block-item">
              <span class="chip tool mono">🔧 {{ t.name }}</span>
            </div>
          </div>
        </div>
      </div>
    </n-modal>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'
import PluginCard from '../components/PluginCard.vue'

const plugins = ref([])
const dirty = ref(new Set())
const detail = ref(null)
const showDetail = ref(false)

async function load() {
  const { data } = await api.get('/plugins/list')
  plugins.value = data.plugins || []
  dirty.value = new Set(plugins.value.filter(p => p.enabled !== p.loaded).map(p => p.name))
}

async function toggle(p, v) {
  try {
    await api.post('/plugins/toggle', { name: p.name, enabled: v })
    p.enabled = v
    if (v !== p.loaded) dirty.value.add(p.name)
    else dirty.value.delete(p.name)
    dirty.value = new Set(dirty.value)
  } catch (e) {
    console.error('toggle 失败', e)
  }
}

function openDetail(p) {
  detail.value = p
  showDetail.value = true
}

onMounted(load)
</script>

<style scoped>
.detail { display: flex; flex-direction: column; gap: 12px; }
.detail-row { display: flex; gap: 14px; align-items: baseline; }
.detail-row .k {
  flex: none; width: 72px;
  font-size: 12px; color: var(--yt-ink-3);
}
.detail-row .v { font-size: 13px; color: var(--yt-ink-1); }
.detail-block {
  border-top: 1px dashed var(--yt-card-border);
  padding-top: 10px;
}
.cap-block-label {
  font-size: 11px; letter-spacing: 0.06em;
  color: var(--yt-ink-3);
  margin-bottom: 8px;
  font-weight: 600;
}
.cap-block-list { display: flex; flex-direction: column; gap: 8px; }
.cap-block-item { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
.cap-block-desc { font-size: 12px; color: var(--yt-ink-2); }
.mono { font-family: var(--yt-mono); }
</style>
