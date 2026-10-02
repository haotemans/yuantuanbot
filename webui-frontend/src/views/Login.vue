<template>
  <div class="login-hero">
    <div class="login-grid">
      <section class="login-brand">
        <div class="login-brand-name">
          <span class="yt-brand-logo">云</span>
          云团
        </div>
        <p class="login-brand-tag">
          一个长期运行的 QQ 群聊 Agent——会搭话、有记忆、有情绪。<br />
          这里是它的运维面板。
        </p>
        <div class="login-brand-points">
          <span class="login-point">Decision trace 实时观察</span>
          <span class="login-point">记忆与人格管理</span>
          <span class="login-point">关系网可视化</span>
        </div>
      </section>
      <n-card style="width: 100%">
        <div style="font-size: 18px; font-weight: 700; margin-bottom: 4px">登录</div>
        <p v-if="needSetup === true" style="color: var(--yt-text-dim); font-size: 13px; margin: 0 0 14px">
          首次使用：输入的密码即为管理员密码（首启引导）
        </p>
        <p v-else-if="needSetup === false" style="color: var(--yt-text-dim); font-size: 13px; margin: 0 0 14px">
          请输入管理员密码
        </p>
        <div v-else style="margin-bottom: 14px" />
        <n-input v-model:value="pw" type="password" show-password-on="click"
                 placeholder="管理员密码" size="large" @keyup.enter="doLogin" />
        <n-button type="primary" block size="large" style="margin-top: 14px" :loading="loading" @click="doLogin">
          进入面板
        </n-button>
        <n-alert v-if="msg" type="error" style="margin-top: 12px">{{ msg }}</n-alert>
      </n-card>
    </div>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import axios from 'axios'
import { TOKEN_KEY } from '../api'
import { reconnectWs, ensureWs } from '../ws'

const pw = ref('')
const msg = ref('')
const loading = ref(false)
const router = useRouter()
// null=未知（接口失败静默退化，不显示提示以免误导）
const needSetup = ref(null)

onMounted(async () => {
  try {
    const { data } = await axios.get('/api/auth/status')
    needSetup.value = !!data.need_setup
  } catch { /* 静默退化：不显示任何提示 */ }
})

async function doLogin() {
  msg.value = ''
  if (!pw.value) { msg.value = '请输入密码'; return }
  loading.value = true
  try {
    const { data } = await axios.post('/api/auth/login', { password: pw.value })
    localStorage.setItem(TOKEN_KEY, data.token)
    reconnectWs(); ensureWs()
    router.push({ name: 'dashboard' })
  } catch (e) {
    msg.value = e.response?.data?.error || '登录失败'
  } finally {
    loading.value = false
  }
}
</script>
