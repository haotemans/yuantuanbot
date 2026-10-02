<template>
  <div class="yt-page">
    <n-card title="LLM Providers" size="small" style="margin-bottom: 14px">
      <n-grid cols="1 m:2" :x-gap="12" :y-gap="12" responsive="screen">
        <n-gi v-for="item in providerList" :key="item.name">
          <provider-card :name="item.name" :p="item.p" @delete="delProvider(item.name)" />
        </n-gi>
      </n-grid>
      <empty-state v-if="!Object.keys(providers).length" title="还没有 Provider"
                   hint="添加一个 OpenAI 兼容端点（base_url + 环境变量名），密钥放服务器环境变量里" />
      <n-space style="margin-top: 12px">
        <n-input v-model:value="newName" size="small" placeholder="新 provider 名" style="width: 180px" @keyup.enter="addProvider" />
        <n-button size="small" @click="addProvider">添加 Provider</n-button>
      </n-space>
    </n-card>

    <n-card title="角色绑定（三角色独立，允许绑同一家）" size="small">
      <n-grid cols="1 m:3" :x-gap="12" :y-gap="12" responsive="screen">
        <n-gi v-for="r in roleNames" :key="r">
          <div class="provider-card">
            <div style="font-weight: 700; margin-bottom: 10px">{{ r }}</div>
            <n-space vertical :size="8" style="width: 100%">
              <n-select v-model:value="roles[r].provider" :options="providerOptions" size="small" placeholder="provider" />
              <n-input v-model:value="roles[r].model" size="small" placeholder="model" />
              <div style="display: flex; align-items: center; gap: 8px; min-height: 24px">
                <n-button size="tiny" secondary :loading="tests[r].state === 'loading'" @click="testRole(r)">测试连通</n-button>
                <span v-if="tests[r].state === 'ok'" class="test-ok mono">
                  {{ tests[r].latency }}ms · {{ tests[r].model }}
                </span>
                <span v-else-if="tests[r].state === 'fail'" class="test-fail">{{ tests[r].error }}</span>
              </div>
            </n-space>
          </div>
        </n-gi>
      </n-grid>
      <n-button type="primary" size="small" style="margin-top: 14px" :loading="saving" @click="save">保存模型配置并热重建</n-button>
      <span v-if="msg" :style="{ color: ok ? '#16a34a' : '#dc2626', fontSize: '12px', marginLeft: '8px' }">{{ msg }}</span>
    </n-card>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'
import ProviderCard from '../components/ProviderCard.vue'
import axios from 'axios'

/** @type {import('vue').Ref<Record<string, { base_url: string, modelsText: string, api_key_env: string, api_key_present: boolean }>>} */
const providers = ref({})
const roles = ref({
  decision: { provider: null, model: '' },
  bot_chat: { provider: null, model: '' },
  agent_exec: { provider: null, model: '' },
})
const roleNames = ['decision', 'bot_chat', 'agent_exec']
const newName = ref('')
const saving = ref(false)
const msg = ref('')
const ok = ref(false)
/** @type {import('vue').Ref<Record<string, { state: string, latency: number|null, model: string, error: string }>>} */
const tests = ref({
  decision: { state: 'idle', latency: null, model: '', error: '' },
  bot_chat: { state: 'idle', latency: null, model: '', error: '' },
  agent_exec: { state: 'idle', latency: null, model: '', error: '' },
})

const providerOptions = computed(() => Object.keys(providers.value).map((n) => ({ label: n, value: n })))
// 面板渲染列表：{name, p} 包装保留原对象引用（v-model 直改嵌套字段仍走响应式）
const providerList = computed(() => Object.entries(providers.value).map(([name, p]) => ({ name, p })))

onMounted(async () => {
  const { data } = await api.get('/config')
  const ps = data.providers?.provider || {}
  for (const [name, p] of Object.entries(ps)) {
    providers.value[name] = {
      base_url: p.base_url || '',
      modelsText: (p.models || []).join(','),
      api_key_env: '',
      api_key_present: !!p.api_key_present,
    }
  }
  const rs = data.providers?.roles || {}
  for (const r of roleNames) {
    if (rs[r]) roles.value[r] = { provider: rs[r].provider, model: rs[r].model }
  }
})

async function testRole(r) {
  tests.value[r] = { state: 'loading', latency: null, model: '', error: '' }
  try {
    const { data } = await api.post('/llm/test', { role: r })
    tests.value[r] = { state: data.ok ? 'ok' : 'fail', latency: data.latency_ms ?? null, model: data.model || '', error: data.error || '' }
  } catch (e) {
    const err = axios.isAxiosError(e) ? (e.response?.data?.error || '请求失败') : '请求失败'
    tests.value[r] = { state: 'fail', latency: null, model: '', error: err }
  }
}

function addProvider() {
  const n = newName.value.trim()
  if (!n || providers.value[n]) return
  providers.value[n] = { base_url: '', modelsText: '', api_key_env: '', api_key_present: false }
  newName.value = ''
}
function delProvider(n) { delete providers.value[n] }

async function save() {
  saving.value = true
  msg.value = ''
  try {
    const provider = {}
    for (const [name, p] of Object.entries(providers.value)) {
      provider[name] = {
        base_url: p.base_url,
        api_key_env: p.api_key_env,
        models: p.modelsText.split(',').map((s) => s.trim()).filter(Boolean),
      }
    }
    const rolesOut = {}
    for (const r of roleNames) {
      if (roles.value[r].provider) rolesOut[r] = { provider: roles.value[r].provider, model: roles.value[r].model }
    }
    const { data } = await api.post('/config', { providers: { provider, roles: rolesOut } })
    ok.value = true
    msg.value = `已保存（${(data.applied || []).join('、')}）`
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}
</script>

<style scoped>
.provider-card {
  border: 1px solid var(--yt-card-border);
  border-radius: 12px;
  padding: 14px;
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}
.provider-card:hover {
  border-color: rgba(79, 70, 229, 0.35);
  box-shadow: var(--yt-card-shadow);
}
.test-ok {
  font-size: 12px;
  color: #16a34a;
}
.test-fail {
  font-size: 12px;
  color: #dc2626;
  line-height: 1.5;
}
</style>
