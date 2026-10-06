<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">插件</span>
      <span class="yt-page-sub">Tools · Skills · MCP（编译期加载，启禁需重启）</span>
      <span class="spacer" />
      <n-button size="tiny" secondary @click="load">刷新</n-button>
    </div>

    <n-alert v-if="dirty.size" type="warning" :bordered="false" style="margin-bottom: 14px">
      有 {{ dirty.size }} 个插件的启禁状态待重启后生效：{{ [...dirty].join('、') }}
    </n-alert>

    <n-card size="small">
      <div v-if="plugins.length" class="plugin-list">
        <div v-for="p in plugins" :key="p.name" class="plugin-row" :class="{ disabled: !p.enabled }">
          <div class="plugin-icon">
            <span class="dot" :class="{ on: p.enabled && p.loaded, warn: p.enabled && !p.loaded }" />
          </div>
          <div class="plugin-info">
            <div class="plugin-headline">
              <span class="plugin-name mono">{{ p.name }}</span>
              <n-tag size="tiny" round :bordered="false" type="info" v-if="p.version">v{{ p.version }}</n-tag>
              <n-tag size="tiny" round :bordered="false" type="success" v-if="p.enabled && p.loaded">已加载</n-tag>
              <n-tag size="tiny" round :bordered="false" type="warning" v-else-if="p.enabled && !p.loaded">已启用·待重启</n-tag>
              <n-tag size="tiny" round :bordered="false" v-else>已禁用</n-tag>
            </div>
            <div class="plugin-desc">
              {{ p.description || '（无描述；可在 plugins/' + p.name + '/Cargo.toml 的 [package] description 补）' }}
            </div>
            <div class="plugin-meta mono" v-if="p.data_dir">
              data/{{ p.name }}/
            </div>
          </div>
          <div class="plugin-actions">
            <n-switch :value="p.enabled" @update:value="(v) => toggle(p, v)" />
          </div>
        </div>
      </div>
      <empty-state v-else title="没有插件"
                   hint="在 plugins/<name>/ 下加一个独立 cargo crate，参考 plugins/README.md 与 plugins/hello/" />
    </n-card>

    <n-alert type="info" style="margin-top: 14px">
      <b>插件是编译期加载</b>：每个 <code>plugins/&lt;name&gt;/</code> 是独立 cargo crate。启禁开关写 <code>enabled</code> 标记文件，
      <b>必须重启 yuantuan 后端才能生效</b>。要加新插件，参考 <code>plugins/hello/</code> 与 <code>plugins/README.md</code>。
    </n-alert>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'

const plugins = ref([])
const dirty = ref(new Set())

async function load() {
  const { data } = await api.get('/plugins/list')
  plugins.value = data.plugins || []
  // dirty = enabled xor loaded（任一不一致就需要重启）
  dirty.value = new Set(plugins.value.filter(p => p.enabled !== p.loaded).map(p => p.name))
}

async function toggle(p, v) {
  try {
    const { data } = await api.post('/plugins/toggle', { name: p.name, enabled: v })
    p.enabled = v
    if (v !== p.loaded) dirty.value.add(p.name)
    else dirty.value.delete(p.name)
    dirty.value = new Set(dirty.value)  // trigger reactivity
    if (data.message) {
      // 显示提示但不打扰操作
      console.info('[plugins]', data.message)
    }
  } catch (e) {
    console.error('toggle 失败', e)
  }
}

onMounted(load)
</script>

<style scoped>
.plugin-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.plugin-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 14px 18px;
  border: 1px solid var(--yt-card-border);
  border-radius: 12px;
  background: var(--yt-header-bg);
  transition: border-color 0.15s ease, background 0.15s ease;
}
.plugin-row:hover {
  border-color: rgba(79, 70, 229, 0.3);
}
.plugin-row.disabled {
  opacity: 0.6;
}
.plugin-icon {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: var(--yt-active-bg);
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--yt-ink-3);
}
.dot.on { background: #16a34a; box-shadow: 0 0 0 3px rgba(22,163,74,0.18); }
.dot.warn { background: #d97706; box-shadow: 0 0 0 3px rgba(217,119,6,0.18); }

.plugin-info { flex: 1; min-width: 0; }
.plugin-headline {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.plugin-name {
  font-weight: 700;
  font-size: 14px;
  color: var(--yt-ink-1);
}
.plugin-desc {
  margin-top: 3px;
  font-size: 12px;
  color: var(--yt-ink-2);
  line-height: 1.4;
}
.plugin-meta {
  margin-top: 4px;
  font-size: 11.5px;
  color: var(--yt-ink-3);
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.plugin-actions { flex: none; }
</style>
