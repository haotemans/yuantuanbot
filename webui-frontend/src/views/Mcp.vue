<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">MCP 服务器</span>
      <span class="yt-page-sub">stdio 子进程 · 保存写回 config.toml，重启 yuantuan 后生效</span>
      <span class="spacer" />
      <n-button size="tiny" secondary @click="load">刷新</n-button>
      <n-button size="tiny" type="primary" @click="openAdd">+ 新增 server</n-button>
      <n-button size="tiny" type="primary" :disabled="!dirtyFlag" @click="saveAll" secondary v-if="dirtyFlag">
        保存全部 ●
      </n-button>
    </div>

    <n-alert v-if="dirtyFlag" type="warning" :bordered="false" style="margin-bottom: 14px">
      配置已修改未保存。点右上「保存全部」写回 config.toml；重启 yuantuan 才 spawn / 关停。
    </n-alert>

    <div v-if="servers.length" class="cap-grid">
      <McpServerCard
        v-for="s in servers"
        :key="s.name"
        :server="s"
        :dirty="dirtyFlag"
        @toggle="(sv, v) => { sv.enabled = v; markDirty() }"
        @open="openEdit"
      />
    </div>
    <n-card v-else size="small">
      <empty-state title="没有 MCP server"
                   hint="点右上「+ 新增 server」加一个；推荐 @modelcontextprotocol/server-filesystem 或 uvx mcp-server-fetch" />
    </n-card>

    <!-- 新增/编辑 弹窗 -->
    <n-modal v-model:show="showEdit" preset="card" :title="editForm.isNew ? '新增 MCP server' : `编辑 ${editForm.name}`" style="max-width: 560px">
      <div class="form">
        <div class="form-row">
          <span class="k">name (snake_case)</span>
          <n-input size="small" v-model:value="editForm.name" :disabled="!editForm.isNew" placeholder="fs / web / docs" />
        </div>
        <div class="form-row">
          <span class="k">command</span>
          <n-input size="small" v-model:value="editForm.command" placeholder="npx / uvx / 绝对路径" />
        </div>
        <div class="form-row">
          <span class="k">args（空格分隔）</span>
          <n-input size="small" v-model:value="editForm.argsText" placeholder="-y @modelcontextprotocol/server-filesystem /data" />
        </div>
        <div class="form-row">
          <span class="k">env（KEY=VAL 每行一条）</span>
          <n-input size="small" type="textarea" :autosize="{ minRows: 2, maxRows: 5 }"
                   v-model:value="editForm.envText" placeholder="API_KEY=xxx" />
        </div>
        <div class="form-row">
          <span class="k">启用</span>
          <n-switch v-model:value="editForm.enabled" size="small" />
        </div>
      </div>
      <template #footer>
        <div style="display: flex; gap: 8px; justify-content: space-between; width: 100%">
          <n-button v-if="!editForm.isNew" size="small" type="error" secondary @click="onDelete">删除</n-button>
          <span v-else />
          <div style="display: flex; gap: 8px">
            <n-button size="small" @click="showEdit = false">取消</n-button>
            <n-button size="small" type="primary" :disabled="!formValid" @click="onSubmit">
              {{ editForm.isNew ? '添加' : '保存修改' }}
            </n-button>
          </div>
        </div>
      </template>
    </n-modal>

    <n-card size="small" style="margin-top: 16px">
      <div class="hint">
        常用：filesystem = <code>npx -y @modelcontextprotocol/server-filesystem /path/to/share</code>；
        fetch = <code>uvx mcp-server-fetch</code>；更多见
        <a href="https://github.com/modelcontextprotocol/servers" target="_blank" rel="noreferrer">官方 server 列表</a>
      </div>
    </n-card>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'
import McpServerCard from '../components/McpServerCard.vue'

const servers = ref([])
const dirtyFlag = ref(false)

const showEdit = ref(false)
const editForm = ref(emptyForm())
const editingIndex = ref(-1)

function emptyForm() {
  return { isNew: true, name: '', command: '', argsText: '', envText: '', enabled: true }
}

const formValid = computed(() => editForm.value.name.trim() && editForm.value.command.trim())

function markDirty() { dirtyFlag.value = true }

async function load() {
  const { data } = await api.get('/mcp/list')
  servers.value = (data.servers || []).map(s => ({ ...s, env: {} }))
  dirtyFlag.value = false
}

function openAdd() {
  editForm.value = emptyForm()
  editingIndex.value = -1
  showEdit.value = true
}
function openEdit(s) {
  const idx = servers.value.findIndex(x => x.name === s.name)
  if (idx < 0) return
  editingIndex.value = idx
  const envText = (s.env_keys || []).map(k => `${k}=`).join('\n')
  editForm.value = {
    isNew: false,
    name: s.name,
    command: s.command,
    argsText: (s.args || []).join(' '),
    envText,
    enabled: s.enabled,
  }
  showEdit.value = true
}

function onSubmit() {
  const f = editForm.value
  const args = f.argsText.split(/\s+/).filter(x => x)
  const env = {}
  for (const line of f.envText.split('\n')) {
    const t = line.trim()
    if (!t || t.startsWith('#')) continue
    const eq = t.indexOf('=')
    if (eq < 0) continue
    env[t.slice(0, eq)] = t.slice(eq + 1)
  }
  const next = {
    name: f.name.trim(),
    command: f.command.trim(),
    args,
    env,
    env_keys: Object.keys(env),
    enabled: f.enabled,
    running: false,
    tools: [],
  }
  if (f.isNew) servers.value.push(next)
  else servers.value.splice(editingIndex.value, 1, next)
  dirtyFlag.value = true
  showEdit.value = false
}

function onDelete() {
  if (editingIndex.value >= 0) {
    servers.value.splice(editingIndex.value, 1)
    dirtyFlag.value = true
  }
  showEdit.value = false
}

async function saveAll() {
  try {
    const body = servers.value.map(s => ({
      name: s.name,
      command: s.command,
      args: s.args,
      env: s.env || {},
      enabled: s.enabled,
    }))
    const { data } = await api.post('/mcp/save', { servers: body })
    if (data.ok) {
      dirtyFlag.value = false
      window?.alert?.('已保存；重启 yuantuan 后 MCP server 才会 spawn / 关停')
    }
  } catch (e) {
    console.error('save 失败', e)
    window?.alert?.('保存失败：' + (e?.response?.data?.error || e.message))
  }
}

onMounted(load)
</script>

<style scoped>
.form { display: flex; flex-direction: column; gap: 12px; }
.form-row { display: flex; align-items: center; gap: 12px; }
.form-row .k {
  flex: none; width: 180px;
  font-size: 12px; color: var(--yt-ink-3);
}
.hint { font-size: 12px; color: var(--yt-ink-3); line-height: 1.7; }
.hint code {
  background: var(--yt-active-bg);
  padding: 1px 6px;
  border-radius: 4px;
  font-size: 11.5px;
}
</style>
