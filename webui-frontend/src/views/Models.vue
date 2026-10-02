<template>
  <div style="max-width: 860px">
    <n-card title="LLM Providers" size="small" style="margin-bottom: 12px">
      <n-space vertical>
        <div v-for="(p, name) in providers" :key="name" style="border: 1px solid var(--yt-card-border); border-radius: 10px; padding: 12px">
          <n-grid :cols="4" :x-gap="10">
            <n-gi><n-input :value="name" disabled size="small" /></n-gi>
            <n-gi><n-input v-model:value="p.base_url" size="small" placeholder="base_url" /></n-gi>
            <n-gi><n-input v-model:value="p.modelsText" size="small" placeholder="models（逗号分隔）" /></n-gi>
            <n-gi>
              <n-input v-model:value="p.api_key_env" size="small" placeholder="api_key 环境变量名（不回显）" />
            </n-gi>
          </n-grid>
          <div style="font-size: 12px; color: var(--yt-text-dim); margin-top: 4px">
            密钥放在服务器环境变量里，本页只存变量名；已配置状态：
            <n-tag size="tiny" :type="p.api_key_present ? 'success' : 'default'">{{ p.api_key_present ? '环境变量已就位' : '未配置/无需' }}</n-tag>
            <n-button text size="tiny" type="error" style="float: right" @click="delProvider(name)">删除</n-button>
          </div>
        </div>
        <n-space>
          <n-input v-model:value="newName" size="small" placeholder="新 provider 名" style="width: 180px" />
          <n-button size="small" @click="addProvider">添加</n-button>
        </n-space>
      </n-space>
    </n-card>
    <n-card title="角色绑定（三角色独立，允许绑同一家）" size="small">
      <n-grid :cols="3" :x-gap="10">
        <n-gi v-for="r in roleNames" :key="r">
          <n-form-item :label="r">
            <n-space vertical style="width: 100%">
              <n-select v-model:value="roles[r].provider" :options="providerOptions" size="small" placeholder="provider" />
              <n-input v-model:value="roles[r].model" size="small" placeholder="model" />
            </n-space>
          </n-form-item>
        </n-gi>
      </n-grid>
      <n-button type="primary" size="small" :loading="saving" @click="save">保存（providers.toml 整体写回 + LLM 热重建）</n-button>
      <span v-if="msg" :style="{ color: ok ? '#16a34a' : '#dc2626', fontSize: '12px', marginLeft: '8px' }">{{ msg }}</span>
    </n-card>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'

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

const providerOptions = computed(() => Object.keys(providers.value).map((n) => ({ label: n, value: n })))

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
