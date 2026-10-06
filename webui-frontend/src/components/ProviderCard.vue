<template>
  <!-- AstrBot 风格列表行 -->
  <div class="pc-row" :class="{ bound: usage && usage.length }">
    <div class="pc-left">
      <span class="pc-name mono">{{ name }}</span>
      <n-tag size="tiny" round :type="p.api_key_present ? 'success' : 'default'" class="pc-status">
        <span class="dot" :class="{ ok: p.api_key_present }" />
        {{ p.api_key_present ? 'key ✓' : '自定义' }}
      </n-tag>
      <n-tag v-for="r in usage" :key="r" size="tiny" round type="info" class="pc-usage">{{ r }}</n-tag>
    </div>
    <div class="pc-actions">
      <n-button size="tiny" secondary @click="showEdit = true">编辑</n-button>
      <n-popconfirm @positive-click="$emit('delete')">
        <template #trigger>
          <n-button size="tiny" text type="error">删除</n-button>
        </template>
        删除 provider {{ name }}？使用它的角色会被解绑。
      </n-popconfirm>
    </div>
  </div>

  <!-- 编辑抽屉 -->
  <n-modal v-model:show="showEdit" preset="card" :title="`编辑 Provider · ${name}`" style="width: 640px" :bordered="false">
    <n-form label-placement="top" size="small">
      <n-form-item label="Provider ID">
        <n-input :value="name" disabled />
        <template #feedback>
          <span class="hint">唯一标识该 provider，不可改；如需改名请删除后重建</span>
        </template>
      </n-form-item>

      <n-form-item label="API 地址">
        <n-input v-model:value="p.base_url" placeholder="https://api.example.com/v1" />
        <template #feedback>
          <span class="hint">OpenAI 兼容端点；末尾不带斜杠</span>
        </template>
      </n-form-item>

      <n-form-item label="API 协议">
        <n-select v-model:value="p.protocol" :options="protocolOptions" />
        <template #feedback>
          <span class="hint">当前仅支持 OpenAI Chat Completions；Anthropic Messages / Gemini 后续按需扩展</span>
        </template>
      </n-form-item>

      <n-form-item label="API 密钥环境变量">
        <n-input v-model:value="p.api_key_env" placeholder="OPENAI_API_KEY" />
        <template #feedback>
          <span class="hint">密钥放服务器环境变量里，不会写进配置也不回显；留空表示无需密钥（本地 mock）</span>
        </template>
      </n-form-item>

      <n-divider style="margin: 12px 0" />

      <div class="models-section">
        <div class="models-head">
          <div class="models-title">模型目录</div>
          <n-button size="tiny" secondary :loading="fetching" :disabled="!p.base_url" @click="fetchModels">
            获取可用模型
          </n-button>
        </div>
        <div class="models-hint">
          保存后这些模型将出现在角色绑定的下拉里；目录外 ID 仍可手动填写直接发送
        </div>
        <div v-if="fetchError" class="fetch-error">{{ fetchError }}</div>
        <div v-if="availableModels.length" class="models-grid">
          <n-tag
            v-for="m in availableModels"
            :key="m"
            size="small"
            :type="isSelected(m) ? 'primary' : 'default'"
            class="model-tag"
            @click="toggleModel(m)"
          >
            {{ m }}
          </n-tag>
        </div>
        <div v-else class="models-empty">
          还没有可用模型。点「获取可用模型」从 provider 拉取，或下方手动添加。
        </div>

        <div class="add-model-row">
          <n-input v-model:value="newModel" size="small" placeholder="手动添加模型 ID" @keyup.enter="addModel" />
          <n-button size="small" secondary @click="addModel">+ 添加模型</n-button>
        </div>

        <div v-if="selectedModels.length" class="selected-list">
          <div class="selected-title">已选（{{ selectedModels.length }}）</div>
          <div class="selected-tags">
            <n-tag
              v-for="m in selectedModels"
              :key="m"
              size="small"
              closable
              @close="removeModel(m)"
            >
              {{ m }}
            </n-tag>
          </div>
        </div>
      </div>
    </n-form>
  </n-modal>
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

const showEdit = ref(false)
const fetching = ref(false)
const fetchError = ref('')
const newModel = ref('')
const availableModels = ref([])

const protocolOptions = [
  { label: 'OpenAI Chat Completions', value: 'openai_chat' },
  // 后续再扩展：{ label: 'Anthropic Messages', value: 'anthropic' },
  // { label: 'Google Gemini', value: 'gemini' },
]

const selectedModels = computed(() =>
  props.p.modelsText.split(',').map(s => s.trim()).filter(Boolean)
)

function isSelected(m) {
  return selectedModels.value.includes(m)
}

function toggleModel(m) {
  const cur = new Set(selectedModels.value)
  if (cur.has(m)) cur.delete(m)
  else cur.add(m)
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
  try {
    const { data } = await api.get('/llm/models', { params: { provider: props.name } })
    availableModels.value = data.models || []
    if (!availableModels.value.length) {
      fetchError.value = 'provider 返回了空模型列表'
    }
  } catch (e) {
    fetchError.value = axios.isAxiosError(e) ? (e.response?.data?.error || '请求失败') : '请求失败'
    availableModels.value = []
  } finally {
    fetching.value = false
  }
}
</script>

<style scoped>
/* 列表行 */
.pc-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 18px;
  border: 1px solid var(--yt-card-border);
  border-radius: 12px;
  background: var(--yt-header-bg);
  backdrop-filter: blur(4px);
  transition: border-color 0.15s ease, box-shadow 0.15s ease, transform 0.15s ease;
}
.pc-row:hover {
  border-color: rgba(79, 70, 229, 0.35);
  box-shadow: var(--yt-card-shadow);
  transform: translateY(-1px);
}
.pc-row.bound {
  background: linear-gradient(135deg, var(--yt-header-bg), var(--yt-soft-bg));
}
.pc-left {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 1;
  min-width: 0;
}
.pc-name {
  font-weight: 700;
  font-size: 14px;
  color: var(--yt-ink-1);
}
.pc-status {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.pc-status .dot {
  display: inline-block;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--yt-ink-3);
}
.pc-status .dot.ok {
  background: #16a34a;
  box-shadow: 0 0 0 3px rgba(22, 163, 74, 0.18);
}
.pc-usage { margin-left: 2px; }
.pc-actions {
  display: flex;
  gap: 6px;
  align-items: center;
}

.hint {
  font-size: 11.5px;
  color: var(--yt-ink-3);
}

/* 模型目录 */
.models-section { padding: 4px 0; }
.models-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 4px;
}
.models-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--yt-ink-1);
}
.models-hint {
  font-size: 11.5px;
  color: var(--yt-ink-3);
  margin-bottom: 8px;
}
.fetch-error {
  margin: 8px 0;
  padding: 8px 10px;
  background: rgba(220, 38, 38, 0.08);
  border: 1px solid rgba(220, 38, 38, 0.25);
  border-radius: 6px;
  font-size: 12px;
  color: #dc2626;
}
.models-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 10px;
  background: var(--yt-soft-bg);
  border: 1px dashed var(--yt-card-border);
  border-radius: 8px;
  max-height: 220px;
  overflow-y: auto;
}
.model-tag {
  cursor: pointer;
  user-select: none;
  transition: transform 0.1s ease;
}
.model-tag:hover {
  transform: translateY(-1px);
}
.models-empty {
  padding: 24px;
  text-align: center;
  color: var(--yt-ink-3);
  font-size: 12.5px;
  background: var(--yt-soft-bg);
  border: 1px dashed var(--yt-card-border);
  border-radius: 8px;
}
.add-model-row {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
.selected-list {
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px dashed var(--yt-card-border);
}
.selected-title {
  font-size: 12px;
  color: var(--yt-ink-3);
  margin-bottom: 8px;
}
.selected-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
</style>
