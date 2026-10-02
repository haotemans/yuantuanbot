<template>
  <div style="min-height: 100vh; display: flex; align-items: center; justify-content: center">
    <n-card title="云团 WebUI 登录" style="width: 360px">
      <p style="color: #888; font-size: 13px">首次使用：输入的密码即为管理员密码（首启引导）</p>
      <n-input v-model:value="pw" type="password" show-password-on="click" placeholder="管理员密码" @keyup.enter="doLogin" />
      <n-button type="primary" block style="margin-top: 12px" :loading="loading" @click="doLogin">登录</n-button>
      <n-alert v-if="msg" type="error" style="margin-top: 10px">{{ msg }}</n-alert>
    </n-card>
  </div>
</template>

<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import axios from 'axios'
import { TOKEN_KEY } from '../api'
import { reconnectWs, ensureWs } from '../ws'

const pw = ref('')
const msg = ref('')
const loading = ref(false)
const router = useRouter()

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
