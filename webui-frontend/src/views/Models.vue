<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">模型</span>
      <span class="yt-page-sub">OpenAI 兼容端点 · 三角色独立绑定 · 保存即热重建</span>
      <span class="spacer" />
      <n-button size="tiny" secondary @click="load">刷新</n-button>
    </div>

    <!-- ============ 三角色概览（最优先看到） ============ -->
    <div class="role-summary">
      <div v-for="r in roleNames" :key="r" class="role-summary-card" :class="{ bound: roles[r].provider }">
        <div class="role-summary-icon">
          <yt-icon :name="roleIcons[r]" :size="18" />
        </div>
        <div class="role-summary-text">
          <div class="role-summary-name">{{ r }}</div>
          <div class="role-summary-desc">{{ roleDescs[r] }}</div>
          <div class="role-summary-target mono" v-if="roles[r].provider">
            {{ roles[r].provider }}<span class="dim"> / </span>{{ roles[r].model || '—' }}
          </div>
          <div class="role-summary-target unbound" v-else>未绑定</div>
        </div>
        <div class="role-summary-test">
          <template v-if="tests[r].state === 'loading'">
            <n-spin size="small" />
          </template>
          <template v-else-if="tests[r].state === 'ok'">
            <span class="test-pill ok mono" :title="tests[r].model">{{ tests[r].latency }}ms</span>
          </template>
          <template v-else-if="tests[r].state === 'fail'">
            <span class="test-pill fail" :title="tests[r].error">fail</span>
          </template>
        </div>
      </div>
    </div>

    <!-- ============ 角色绑定（详细配置） ============ -->
    <n-card size="small" style="margin-top: 14px">
      <template #header>
        <div class="sec-head">
          <span class="sec-title">角色绑定</span>
          <span class="sec-sub">三角色相互独立，允许绑同一家 provider</span>
        </div>
      </template>
      <n-grid cols="1 m:3" :x-gap="12" :y-gap="12" responsive="screen">
        <n-gi v-for="r in roleNames" :key="r">
          <div class="role-bind-card">
            <div class="role-bind-head">
              <yt-icon :name="roleIcons[r]" :size="16" />
              <span class="role-bind-name">{{ r }}</span>
              <n-tag size="tiny" round :type="roles[r].provider ? 'success' : 'default'">
                {{ roles[r].provider ? '已绑定' : '未绑定' }}
              </n-tag>
            </div>
            <n-space vertical :size="10" style="width: 100%">
              <div>
                <label class="field-label">Provider</label>
                <n-select v-model:value="roles[r].provider" :options="providerOptions" size="small"
                          placeholder="选择 provider" clearable />
              </div>
              <div>
                <label class="field-label">Model</label>
                <n-input v-model:value="roles[r].model" size="small" placeholder="如 gpt-4o-mini / deepseek-chat" />
              </div>
              <div class="test-row">
                <n-button size="tiny" secondary :loading="tests[r].state === 'loading'"
                          :disabled="!roles[r].provider" @click="testRole(r)">
                  测试连通
                </n-button>
                <transition name="yt-field" mode="out-in">
                  <span v-if="tests[r].state === 'ok'" key="ok" class="test-ok mono">
                    ✓ {{ tests[r].latency }}ms · {{ tests[r].model }}
                  </span>
                  <span v-else-if="tests[r].state === 'fail'" key="fail" class="test-fail" :title="tests[r].error">
                    ✗ {{ tests[r].error }}
                  </span>
                </transition>
              </div>
            </n-space>
          </div>
        </n-gi>
      </n-grid>

      <div class="save-bar">
        <transition name="yt-field" mode="out-in">
          <span v-if="msg" :class="['save-msg', ok ? 'ok' : 'fail']" :key="msg">{{ msg }}</span>
        </transition>
        <n-button type="primary" size="medium" :loading="saving" @click="save">
          保存模型配置并热重建
        </n-button>
      </div>
    </n-card>

    <!-- ============ Provider 列表 ============ -->
    <n-card size="small" style="margin-top: 14px">
      <template #header>
        <div class="sec-head">
          <span class="sec-title">Providers</span>
          <span class="sec-sub">{{ Object.keys(providers).length }} 个端点 · OpenAI 兼容</span>
        </div>
      </template>
      <template #header-extra>
        <n-input v-model:value="newName" size="small" placeholder="新 provider 名"
                 style="width: 200px" @keyup.enter="addProvider" />
        <n-button size="small" type="primary" secondary @click="addProvider" style="margin-left: 8px">
          + 添加 Provider
        </n-button>
      </template>
      <div v-if="providerList.length" class="provider-list">
        <provider-card v-for="item in providerList" :key="item.name" :name="item.name" :p="item.p"
                       :usage="providerUsage(item.name)" @delete="delProvider(item.name)" />
      </div>
      <empty-state v-else title="还没有 Provider"
                   hint="添加一个 OpenAI 兼容端点（base_url + 环境变量名）；密钥放服务器环境变量里" />
    </n-card>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'
import ProviderCard from '../components/ProviderCard.vue'
import YtIcon from '../components/YtIcon.vue'
import axios from 'axios'

/** @type {import('vue').Ref<Record<string, { base_url: string, modelsText: string, api_key_env: string, api_key_present: boolean, protocol: string }>>} */
const providers = ref({})
const roles = ref({
  decision: { provider: null, model: '' },
  bot_chat: { provider: null, model: '' },
  agent_exec: { provider: null, model: '' },
})
const roleNames = ['decision', 'bot_chat', 'agent_exec']
const roleIcons = { decision: 'trace', bot_chat: 'chat', agent_exec: 'tasks' }
const roleDescs = {
  decision: '决策小脑 · 便宜快速',
  bot_chat: '人格回复 · 中高档',
  agent_exec: '任务执行 · 强模型',
}
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
const providerList = computed(() => Object.entries(providers.value).map(([name, p]) => ({ name, p })))

// 每个 provider 被哪些角色使用（卡片角标显示）
function providerUsage(name) {
  return roleNames.filter((r) => roles.value[r].provider === name)
}

async function load() {
  try {
    const { data } = await api.get('/config')
    const ps = data.providers?.provider || {}
    // 保留已输入但未保存的字段（避免刷新丢草稿）；刷新只补新发现的 provider
    /** @type {Record<string, { base_url: string, modelsText: string, api_key_env: string, api_key_present: boolean, protocol: string }>} */
    const next = {}
    for (const [name, p] of Object.entries(ps)) {
      next[name] = providers.value[name] || {
        base_url: p.base_url || '',
        modelsText: (p.models || []).join(', '),
        api_key_env: '',
        api_key_present: !!p.api_key_present,
        protocol: 'openai_chat',
      }
      next[name].api_key_present = !!p.api_key_present
      if (!next[name].protocol) next[name].protocol = 'openai_chat'
    }
    providers.value = next
    const rs = data.providers?.roles || {}
    for (const r of roleNames) {
      if (rs[r]) roles.value[r] = { provider: rs[r].provider, model: rs[r].model }
    }
  } catch { /* 401 拦截器兜底 */ }
}

onMounted(load)

async function testRole(r) {
  tests.value[r] = { state: 'loading', latency: null, model: '', error: '' }
  try {
    const { data } = await api.post('/llm/test', { role: r })
    tests.value[r] = {
      state: data.ok ? 'ok' : 'fail',
      latency: data.latency_ms ?? null,
      model: data.model || '',
      error: data.error || '',
    }
  } catch (e) {
    const err = axios.isAxiosError(e) ? (e.response?.data?.error || '请求失败') : '请求失败'
    tests.value[r] = { state: 'fail', latency: null, model: '', error: err }
  }
}

function addProvider() {
  const n = newName.value.trim()
  if (!n || providers.value[n]) return
  providers.value[n] = { base_url: '', modelsText: '', api_key_env: '', api_key_present: false, protocol: 'openai_chat' }
  newName.value = ''
}
function delProvider(n) {
  delete providers.value[n]
  // 同步解绑使用它的角色
  for (const r of roleNames) {
    if (roles.value[r].provider === n) roles.value[r].provider = null
  }
}

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
    msg.value = `已保存（${(data.applied || []).join('、') || '无变更'}）`
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}
</script>

<style scoped>
/* ===== 三角色概览条 ===== */
.role-summary {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
  gap: 12px;
}
.role-summary-card {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  border-radius: 12px;
  border: 1px solid var(--yt-card-border);
  background: var(--yt-header-bg);
  backdrop-filter: blur(6px);
  transition: border-color 0.15s ease, box-shadow 0.15s ease, transform 0.15s ease;
}
.role-summary-card:hover {
  transform: translateY(-1px);
  box-shadow: var(--yt-card-shadow);
}
.role-summary-card.bound {
  border-color: rgba(79, 70, 229, 0.25);
  background: linear-gradient(135deg, var(--yt-header-bg), var(--yt-soft-bg));
}
.role-summary-icon {
  width: 36px;
  height: 36px;
  flex: none;
  border-radius: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--yt-primary);
  background: var(--yt-active-bg);
}
.role-summary-text { flex: 1; min-width: 0; }
.role-summary-name {
  font-weight: 700;
  font-size: 13.5px;
  font-family: var(--yt-mono);
  color: var(--yt-ink-1);
}
.role-summary-desc {
  font-size: 11.5px;
  color: var(--yt-ink-3);
  margin-top: 1px;
}
.role-summary-target {
  font-size: 12px;
  margin-top: 4px;
  color: var(--yt-ink-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.role-summary-target .dim { color: var(--yt-ink-3); }
.role-summary-target.unbound { color: var(--yt-ink-3); font-style: italic; }
.role-summary-test { flex: none; }
.test-pill {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11.5px;
  font-weight: 600;
}
.test-pill.ok { background: rgba(22, 163, 74, 0.12); color: #16a34a; }
.test-pill.fail { background: rgba(220, 38, 38, 0.12); color: #dc2626; }

/* ===== 通用 section head ===== */
.sec-head { display: flex; align-items: baseline; gap: 10px; }
.sec-title { font-weight: 700; font-size: 14px; color: var(--yt-ink-1); }
.sec-sub { font-size: 12px; color: var(--yt-ink-3); }

/* ===== 角色绑定卡片 ===== */
.role-bind-card {
  border: 1px solid var(--yt-card-border);
  border-radius: 12px;
  padding: 14px;
  background: var(--yt-soft-bg);
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}
.role-bind-card:hover {
  border-color: rgba(79, 70, 229, 0.35);
  box-shadow: var(--yt-card-shadow);
}
.role-bind-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
  color: var(--yt-primary);
}
.role-bind-name {
  font-weight: 700;
  font-family: var(--yt-mono);
  font-size: 13px;
  color: var(--yt-ink-1);
  flex: 1;
}
.field-label {
  display: block;
  font-size: 11.5px;
  color: var(--yt-ink-3);
  margin-bottom: 4px;
}
.test-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 26px;
}
.test-ok { font-size: 12px; color: #16a34a; }
.test-fail { font-size: 12px; color: #dc2626; }

/* ===== 保存条 ===== */
.save-bar {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  margin-top: 16px;
  padding-top: 14px;
  border-top: 1px dashed var(--yt-card-border);
}
.save-msg { font-size: 12.5px; }
.save-msg.ok { color: #16a34a; }
.save-msg.fail { color: #dc2626; }

/* ===== Provider 列表 ===== */
.provider-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
</style>
