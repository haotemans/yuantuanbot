<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">平台连接</span>
      <span class="yt-page-sub">NapCat OneBot 11 反向 WS</span>
      <span class="spacer" />
      <span
        class="live-pill"
        :class="{ on: d.adapter_connected }"
        :title="d.adapter_connected ? 'NapCat 已拨入' : '等待 NapCat 拨入'"
      >
        <span class="live-dot" />{{ d.adapter_connected ? 'LIVE' : '离线' }}
      </span>
      <n-button size="tiny" secondary @click="reload">刷新</n-button>
    </div>

    <!-- 状态卡 -->
    <n-grid cols="1 s:2 l:4" :x-gap="14" :y-gap="14" responsive="screen">
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" :style="{ background: d.adapter_connected ? '#16a34a' : '#dc2626' }" />
            连接状态
          </div>
          <div class="stat-foot">
            <span class="stat-num" :style="{ color: d.adapter_connected ? '#16a34a' : '#dc2626' }">
              {{ d.adapter_connected ? '已连接' : '未拨入' }}
            </span>
          </div>
          <div class="stat-hint">{{ d.adapter_connected ? 'NapCat Websockets客户端 已拨入本端' : '等待 NapCat 卡片拨入或填写错 URL' }}</div>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" style="background: #06b6d4" />
            登录身份
          </div>
          <div class="stat-foot">
            <span class="stat-num mono">{{ d.self_qq || '—' }}</span>
          </div>
          <div class="stat-hint">get_login_info 握手拿到的自身 QQ 号</div>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" style="background: #4f46e5" />
            监听地址
          </div>
          <div class="stat-foot">
            <span class="stat-num mono" style="font-size: 18px">{{ form.listen_addr || '127.0.0.1:6199' }}</span>
          </div>
          <div class="stat-hint">NapCat 需拨入 <code>ws://{{ form.listen_addr || '127.0.0.1:6199' }}/ws</code></div>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" style="background: #d97706" />
            Token
          </div>
          <div class="stat-foot">
            <span class="stat-num">{{ hasToken ? '已配置' : '未设置' }}</span>
          </div>
          <div class="stat-hint">鉴权 NapCat 拨入用；改动立即生效</div>
        </n-card>
      </n-gi>
    </n-grid>

    <!-- 配置表单 -->
    <n-card title="连接配置" size="small" style="margin-top: 14px">
      <n-form label-placement="left" label-width="110" style="max-width: 620px">
        <n-form-item label="启用">
          <n-switch v-model:value="form.enabled" />
          <span class="form-hint">关闭后不再接受 NapCat 拨入</span>
        </n-form-item>
        <n-form-item label="监听地址">
          <n-input v-model:value="form.listen_addr" placeholder="127.0.0.1:6199" />
        </n-form-item>
        <n-form-item label="Token">
          <n-input v-model:value="form.token" type="password" show-password-on="click"
                   placeholder="不回读；留空 = 保留后端原值" />
        </n-form-item>
      </n-form>
      <n-space>
        <n-button type="primary" :loading="saving" @click="save(false)">保存连接配置</n-button>
        <n-button type="warning" :loading="saving || restarting" @click="save(true)">保存并重启</n-button>
        <span v-if="msg" :style="{ color: ok ? '#16a34a' : '#dc2626', fontSize: '12px' }">{{ msg }}</span>
      </n-space>
    </n-card>

    <!-- NapCat 卡片填法指导 -->
    <n-card title="NapCat 侧配置" size="small" style="margin-top: 14px">
      <div class="guide">
        <div class="guide-step">
          <span class="step-no">1</span>
          <div>
            <div class="step-title">打开 NapCat WebUI</div>
            <div class="step-desc">
              浏览器前往 <code class="mono">http://127.0.0.1:6099/webui</code>，登录后左侧选「网络配置」。
            </div>
          </div>
        </div>
        <div class="guide-step">
          <span class="step-no">2</span>
          <div>
            <div class="step-title">新建 → Websockets客户端</div>
            <div class="step-desc">
              类型选「Websockets客户端」（不是服务器！）；名称随意；启用打开。
            </div>
          </div>
        </div>
        <div class="guide-step">
          <span class="step-no">3</span>
          <div>
            <div class="step-title">填 URL 与 Token</div>
            <div class="step-desc">
              URL 填 <code class="mono">ws://{{ form.listen_addr || '127.0.0.1:6199' }}/ws</code>（注意结尾 <code>/ws</code>）。
              Token 与本页 Token 字段填<strong>同一个值</strong>。
            </div>
          </div>
        </div>
        <div class="guide-step">
          <span class="step-no">4</span>
          <div>
            <div class="step-title">保存即可</div>
            <div class="step-desc">
              NapCat 卡片的 Websockets客户端是出站连接，保存即生效无需重启 QQ 主进程。
              本页顶部 LIVE 灯变绿即握手成功。
            </div>
          </div>
        </div>
      </div>
    </n-card>

    <n-alert type="info" style="margin-top: 14px">
      Token 改动<strong>立即生效</strong>（下次 NapCat 拨入即用新值）。
      监听地址改动需<strong>重启</strong>：点「保存并重启」面板触发后端重启（约 3 秒自动刷新），无需手动 taskkill。
    </n-alert>
  </div>
</template>

<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import { api } from '../api'
import { subscribeWs } from '../ws'

/** @type {import('vue').Ref<Record<string, any>>} */
const d = ref({ adapter_connected: false, self_qq: 0 })
const form = ref({ enabled: true, listen_addr: '127.0.0.1:6199', token: '' })
const hasToken = ref(false)
/** @type {import('vue').Ref<Record<string, any>>} */
const whole = ref({})
const saving = ref(false)
const restarting = ref(false)
const msg = ref('')
const ok = ref(false)

async function loadDashboard() {
  try {
    const { data } = await api.get('/dashboard')
    d.value.adapter_connected = !!data.adapter_connected
    d.value.self_qq = data.self_qq || 0
  } catch { /* ignore */ }
}

async function loadConfig() {
  const { data } = await api.get('/config')
  whole.value = data.config
  const n = data.config.napcat || {}
  form.value = {
    enabled: n.enabled ?? true,
    listen_addr: n.listen_addr ?? '127.0.0.1:6199',
    token: '',
  }
  hasToken.value = !!(n.token && n.token !== '')
  // 不删 token 字段：保留掩码 "***" 给后端 unmask 还原（用户填新值时会被替换）
}

async function reload() {
  await Promise.all([loadDashboard(), loadConfig()])
}

async function save(andRestart = false) {
  saving.value = true
  msg.value = ''
  try {
    const newNapcat = { ...whole.value.napcat }
    newNapcat.enabled = form.value.enabled
    newNapcat.listen_addr = form.value.listen_addr
    delete newNapcat.ws_url
    if (form.value.token) newNapcat.token = form.value.token
    // 用户留空 = 保留 newNapcat.token 原值(掩码 "***",后端 unmask 还原真值)
    const config = { ...whole.value, napcat: newNapcat }
    const { data } = await api.post('/config', { config })
    whole.value = config
    form.value.token = ''
    ok.value = true
    if (andRestart) {
      restarting.value = true
      msg.value = '已保存,重启中…(页面会自动刷新)'
      setTimeout(() => { window.location.reload() }, 3200)
      try {
        await api.post('/config/restart')
      } catch { /* 重启后旧连接会断,请求可能失败属预期 */ }
      return
    }
    const restarts = (data.requires_restart || []).filter(r => r.includes('napcat'))
    const parts = []
    if ((data.applied || []).some(a => a.includes('token'))) parts.push('token 已热应用')
    if (restarts.length) parts.push('⚠️ listen_addr 改动需重启后端')
    msg.value = `已保存${parts.length ? '：' + parts.join('，') : ''}`
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}

// WS 推送：adapter_connected 掉线/接上实时反映
function onWsEvent(ev) {
  if (!ev || !ev.kind) return
  if (ev.kind === 'ConfigReloaded') loadConfig()
}

let timer = null
let off = null
onMounted(() => {
  reload()
  timer = setInterval(loadDashboard, 8000)
  off = subscribeWs(onWsEvent)
})
onUnmounted(() => { clearInterval(timer); off && off() })
</script>

<style scoped>
.live-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.6px;
  color: var(--yt-ink-3);
  background: var(--yt-card-border);
  transition: color 0.2s ease, background 0.2s ease;
}
.live-pill.on {
  color: #16a34a;
  background: rgba(22, 163, 74, 0.12);
}
.live-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  flex: none;
}
.live-pill.on .live-dot {
  animation: yt-live-pulse 1.6s ease-in-out infinite;
  box-shadow: 0 0 0 0 rgba(22, 163, 74, 0.4);
}
@keyframes yt-live-pulse {
  0%   { box-shadow: 0 0 0 0 rgba(22, 163, 74, 0.4); }
  70%  { box-shadow: 0 0 0 6px rgba(22, 163, 74, 0); }
  100% { box-shadow: 0 0 0 0 rgba(22, 163, 74, 0); }
}

.stat-card { min-height: 108px; }
.stat-label {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--yt-ink-3);
  font-weight: 600;
}
.stat-chip {
  width: 8px;
  height: 8px;
  border-radius: 2px;
  flex: none;
}
.stat-foot {
  margin-top: 8px;
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.stat-num {
  font-size: 22px;
  font-weight: 700;
  color: var(--yt-ink-1);
  line-height: 1;
}
/* .mono, .stat-num 已由 styles.css 全局提供 */
.stat-hint {
  margin-top: 6px;
  font-size: 11.5px;
  color: var(--yt-ink-3);
  line-height: 1.4;
}
.stat-hint code {
  background: var(--yt-code-bg);
  padding: 1px 5px;
  border-radius: 3px;
  font-size: 11px;
}
.form-hint {
  margin-left: 10px;
  font-size: 12px;
  color: var(--yt-ink-3);
}

.guide {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 4px 0;
}
.guide-step {
  display: flex;
  align-items: flex-start;
  gap: 12px;
}
.step-no {
  width: 24px;
  height: 24px;
  border-radius: 50%;
  background: var(--yt-active-bg);
  color: var(--yt-primary);
  font-weight: 700;
  font-size: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  margin-top: 2px;
}
.step-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--yt-ink-1);
}
.step-desc {
  margin-top: 2px;
  font-size: 12px;
  color: var(--yt-ink-2);
  line-height: 1.5;
}
.step-desc code {
  background: var(--yt-code-bg);
  padding: 1px 5px;
  border-radius: 3px;
  font-size: 11px;
}

@media (prefers-reduced-motion: reduce) {
  .live-pill.on .live-dot { animation: none; }
}
</style>
