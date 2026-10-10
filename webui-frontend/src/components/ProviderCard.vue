<template>
  <div class="pc-wrap">
    <!-- AstrBot 风格一行：左名称 + 中标签 + 右编辑/删除 -->
    <div class="pc-row" :class="{ active: expanded }" @click="expanded = !expanded">
      <div class="pc-icon">
        <span class="pc-status-dot" :class="{ ok: p.api_key_present }" />
      </div>
      <div class="pc-info">
        <div class="pc-headline">
          <span class="pc-name mono">{{ name }}</span>
          <n-tag size="tiny" round :bordered="false" class="pc-tint">
            {{ p.protocol === 'openai_chat' ? 'OpenAI 兼容' : p.protocol }}
          </n-tag>
          <n-tag v-for="r in usage" :key="r" size="tiny" round type="info" :bordered="false">{{ r }}</n-tag>
        </div>
        <div class="pc-sub mono">{{ p.base_url || '未填 base_url' }}</div>
      </div>
      <div class="pc-actions" @click.stop>
        <n-button size="small" secondary round @click="expanded = !expanded">
          {{ expanded ? '收起' : '编辑' }}
        </n-button>
        <n-popconfirm @positive-click="$emit('delete')">
          <template #trigger>
            <n-button size="small" text type="error" round>删除</n-button>
          </template>
          删除 provider {{ name }}？使用它的角色会被解绑。
        </n-popconfirm>
      </div>
    </div>

    <!-- 展开的编辑面板（嵌入式，无 modal） -->
    <n-collapse-transition :show="expanded">
      <div class="pc-edit">
        <div class="edit-grid">
          <div class="field-block">
            <label class="field-label">Provider ID</label>
            <n-input :value="name" disabled size="medium" />
            <div class="field-hint">Provider 的唯一标识，不可改</div>
          </div>
          <div class="field-block">
            <label class="field-label">API 协议</label>
            <n-select v-model:value="p.protocol" :options="protocolOptions" size="medium" />
            <div class="field-hint">当前仅支持 OpenAI Chat Completions</div>
          </div>
          <div class="field-block field-span-2">
            <label class="field-label">API 地址</label>
            <n-input v-model:value="p.base_url" placeholder="https://api.example.com/v1" size="medium" />
            <div class="field-hint">OpenAI 兼容端点，末尾不带斜杠</div>
          </div>
          <div class="field-block field-span-2">
            <label class="field-label">API 密钥</label>
            <n-input v-model:value="p.api_key_env" placeholder="sk-xxxx 直接粘密钥;或环境变量名如 OPENAI_API_KEY" size="medium" />
            <div class="field-hint">直接粘 sk-xxxx 密钥;或填环境变量名(服务器端 export);留空 = 无需密钥(本地 mock)</div>
          </div>
        </div>

        <div class="models-block">
          <div class="models-head">
            <div>
              <div class="models-title">模型目录</div>
              <div class="models-sub">保存后这些模型出现在角色绑定下拉；目录外 ID 仍可手动填写</div>
            </div>
            <n-button size="small" secondary :loading="fetching" :disabled="!p.base_url" @click.stop="fetchModels">
              {{ fetching ? '拉取中…' : '获取可用模型' }}
            </n-button>
          </div>

          <div v-if="fetchError" class="fetch-msg fail">{{ fetchError }}</div>
          <div v-else-if="fetchOk" class="fetch-msg ok">已拉取 {{ availableModels.length }} 个模型（点击任意一个加入目录）</div>

          <div v-if="availableModels.length" class="models-cloud">
            <span
              v-for="m in availableModels"
              :key="m"
              class="model-chip"
              :class="{ on: isSelected(m) }"
              @click.stop="toggleModel(m)"
            >{{ m }}</span>
          </div>

          <div class="models-selected">
            <div class="sel-title">已选模型（{{ selectedModels.length }}）</div>
            <div v-if="selectedModels.length" class="sel-cloud">
              <span v-for="m in selectedModels" :key="m" class="model-chip sel mono" @click.stop="removeModel(m)">
                {{ m }} <span class="x">×</span>
              </span>
            </div>
            <div v-else class="sel-empty">尚未添加模型；从上方拉取点选，或手动加：</div>
            <div class="sel-add">
              <n-input v-model:value="newModel" size="small" placeholder="手动添加模型 ID 回车确认" @keyup.enter="addModel" />
              <n-button size="small" secondary round @click.stop="addModel">+ 添加</n-button>
            </div>
          </div>
        </div>
      </div>
    </n-collapse-transition>
  </div>
</template>

<script setup>
import { computed, ref } from 'vue'
import axios from 'axios'
import { api } from '../api'

const props = defineProps({
  name: { type: String, required: true },
  p: { type: /** @type {import('vue').PropType<{ base_url: string, modelsText: string, api_key_env: string, api_key_present: boolean, protocol: string }>} */ (Object), required: true },
  usage: { type: /** @type {import('vue').PropType<string[]>} */ (Array), default: () => [] },
})
defineEmits(['delete'])

const expanded = ref(false)
const fetching = ref(false)
const fetchError = ref('')
const fetchOk = ref(false)
const newModel = ref('')
const availableModels = ref([])

// 协议下拉：OpenAI 当前实现可用；Anthropic/Gemini 暂为占位提示
const protocolOptions = [
  { label: 'OpenAI Chat Completions', value: 'openai_chat' },
  { label: 'Anthropic Messages（暂未实现）', value: 'anthropic', disabled: true },
  { label: 'Google Gemini（暂未实现）', value: 'gemini', disabled: true },
]

const selectedModels = computed(() =>
  props.p.modelsText.split(',').map(s => s.trim()).filter(Boolean)
)
function isSelected(m) { return selectedModels.value.includes(m) }
function toggleModel(m) {
  const cur = new Set(selectedModels.value)
  if (cur.has(m)) cur.delete(m); else cur.add(m)
  props.p.modelsText = [...cur].join(', ')
}
function addModel() {
  const m = newModel.value.trim()
  if (!m) return
  if (!selectedModels.value.includes(m)) {
    props.p.modelsText = [...selectedModels.value, m].join(', ')
  }
  newModel.value = ''
}
function removeModel(m) {
  props.p.modelsText = selectedModels.value.filter(x => x !== m).join(', ')
}

async function fetchModels() {
  fetching.value = true
  fetchError.value = ''
  fetchOk.value = false
  try {
    // 用 probe：直接用当前表单的 base_url + api_key_env，无需先保存进 providers.toml
    const { data } = await api.post('/llm/models/probe', {
      base_url: props.p.base_url,
      api_key_env: props.p.api_key_env || '',
    })
    availableModels.value = data.models || []
    fetchOk.value = true
    if (!availableModels.value.length) fetchError.value = 'provider 返回了空模型列表'
  } catch (e) {
    fetchError.value = axios.isAxiosError(e) ? (e.response?.data?.error || '请求失败') : '请求失败'
    availableModels.value = []
  } finally {
    fetching.value = false
  }
}
</script>

<style scoped>
/* AstrBot 风格行：圆角大、悬停浅染色 */
.pc-wrap {
  border: 1px solid var(--yt-card-border);
  border-radius: 14px;
  background: var(--yt-header-bg);
  backdrop-filter: blur(4px);
  overflow: hidden;
  transition: border-color 0.15s ease, background 0.15s ease, box-shadow 0.2s ease;
}
.pc-wrap:hover {
  border-color: rgba(79, 70, 229, 0.32);
  box-shadow: var(--yt-card-shadow);
}

.pc-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 14px 18px;
  cursor: pointer;
  transition: background 0.15s ease;
  user-select: none;
}
.pc-row:hover {
  background: rgba(79, 70, 229, 0.04);
}
.pc-row.active {
  border-bottom: 1px solid var(--yt-card-border);
  background: var(--yt-soft-bg);
}

.pc-icon {
  width: 32px;
  height: 32px;
  flex: none;
  border-radius: 8px;
  background: var(--yt-active-bg);
  display: flex;
  align-items: center;
  justify-content: center;
}
.pc-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--yt-ink-3);
}
.pc-status-dot.ok {
  background: var(--yt-ok);
  box-shadow: 0 0 0 3px rgba(22, 163, 74, 0.18);
}

.pc-info { flex: 1; min-width: 0; }
.pc-headline {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.pc-name {
  font-weight: 700;
  font-size: 14.5px;
  color: var(--yt-ink-1);
}
.pc-tint {
  background: rgba(99, 102, 241, 0.1);
  color: var(--yt-primary);
}
.pc-sub {
  margin-top: 3px;
  font-size: 12px;
  color: var(--yt-ink-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pc-actions {
  display: flex;
  gap: 6px;
  flex: none;
}

/* 展开编辑面板 */
.pc-edit {
  padding: 18px 22px 20px;
  background: var(--yt-soft-bg);
  border-top: 1px solid var(--yt-card-border);
}
.edit-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 14px 18px;
  margin-bottom: 18px;
}
.field-block.field-span-2 { grid-column: 1 / -1; }
.field-label {
  display: block;
  font-size: 12px;
  font-weight: 600;
  color: var(--yt-ink-2);
  margin-bottom: 5px;
}
.field-hint {
  margin-top: 4px;
  font-size: 11.5px;
  color: var(--yt-ink-3);
}

.models-block {
  padding-top: 14px;
  border-top: 1px dashed var(--yt-card-border);
}
.models-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
  margin-bottom: 10px;
}
.models-title {
  font-size: 13.5px;
  font-weight: 700;
  color: var(--yt-ink-1);
}
.models-sub {
  font-size: 11.5px;
  color: var(--yt-ink-3);
  margin-top: 2px;
}
.fetch-msg {
  padding: 8px 12px;
  border-radius: 8px;
  font-size: 12px;
  margin-bottom: 10px;
}
.fetch-msg.ok { background: rgba(22, 163, 74, 0.08); color: var(--yt-ok); }
.fetch-msg.fail { background: rgba(220, 38, 38, 0.08); color: var(--yt-danger); }

.models-cloud {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 12px;
  background: var(--yt-header-bg);
  border: 1px solid var(--yt-card-border);
  border-radius: 10px;
  max-height: 180px;
  overflow-y: auto;
  margin-bottom: 14px;
}
.model-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 10px;
  background: var(--yt-code-bg);
  border: 1px solid var(--yt-card-border);
  border-radius: 999px;
  font-size: 12px;
  color: var(--yt-ink-2);
  cursor: pointer;
  transition: all 0.12s ease;
  user-select: none;
}
.model-chip:hover {
  border-color: var(--yt-primary);
  color: var(--yt-primary);
}
.model-chip.on {
  background: var(--yt-primary);
  color: white;
  border-color: var(--yt-primary);
}

.models-selected {
  padding-top: 12px;
  border-top: 1px dashed var(--yt-card-border);
}
.sel-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--yt-ink-2);
  margin-bottom: 8px;
}
.sel-cloud {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 10px;
}
.model-chip.sel {
  background: var(--yt-primary);
  color: white;
  border-color: var(--yt-primary);
  font-weight: 600;
}
.model-chip.sel .x {
  font-weight: 700;
  margin-left: 2px;
  opacity: 0.7;
}
.model-chip.sel:hover .x { opacity: 1; }

.sel-empty {
  font-size: 12px;
  color: var(--yt-ink-3);
  padding: 10px 0;
}
.sel-add {
  display: flex;
  gap: 8px;
}
</style>
