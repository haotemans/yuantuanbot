<template>
  <div class="login-split">
    <section class="login-hero-side">
      <span class="login-hero-glow-b" aria-hidden="true" />
      <div class="login-hero-name yt-anim-fade-down">
        <img :src="mascot" class="yt-brand-logo" alt="云团" />
        云团
      </div>
      <p class="login-hero-slogan yt-anim-fade-up" style="--d: 90ms">
        一个长期运行的 QQ 群聊 Agent——会搭话、有记忆、有情绪。<br />
        这里是它的运维面板。
      </p>
      <div class="login-hero-points">
        <span class="login-hero-point yt-anim-fade-up" style="--d: 160ms">
          <yt-icon name="trace" :size="16" />Decision trace 实时观察
        </span>
        <span class="login-hero-point yt-anim-fade-up" style="--d: 220ms">
          <yt-icon name="memories" :size="16" />记忆与人格版本管理
        </span>
        <span class="login-hero-point yt-anim-fade-up" style="--d: 280ms">
          <yt-icon name="relations" :size="16" />群友关系网可视化
        </span>
      </div>
    </section>
    <section class="login-form-side">
      <n-card class="login-form-card yt-anim-pop" style="--d: 60ms">
        <div style="font-size: 20px; font-weight: 700; letter-spacing: -0.01em; margin-bottom: 4px">
          {{ needSetup === true ? '欢迎使用云团' : '登录' }}
        </div>
        <p class="login-sub">
          <template v-if="needSetup === true">首次使用：输入的密码即为管理员密码（首启引导）</template>
          <template v-else-if="needSetup === false">请输入管理员密码</template>
          <template v-else>&nbsp;</template>
        </p>
        <transition name="yt-field" mode="out-in">
          <n-input v-model:value="pw" type="password" show-password-on="click"
                   :placeholder="needSetup === true ? '设一个管理员密码' : '管理员密码'"
                   size="large" @keyup.enter="doLogin" />
        </transition>
        <n-button type="primary" block size="large" style="margin-top: 14px" :loading="loading" @click="doLogin">
          {{ needSetup === true ? '设置并进入' : '进入面板' }}
        </n-button>
        <transition name="yt-shake-fade">
          <n-alert v-if="msg" type="error" :bordered="false" class="yt-login-err">{{ msg }}</n-alert>
        </transition>
      </n-card>
    </section>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import axios from 'axios'
import { TOKEN_KEY } from '../api'
import { reconnectWs, ensureWs } from '../ws'
import YtIcon from '../components/YtIcon.vue'
import mascot from '../assets/mascot.png'

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

<style scoped>
.login-sub {
  font-size: 13px;
  color: var(--yt-text-dim);
  margin: 0 0 16px;
  min-height: 18px;
}
.yt-login-err {
  margin-top: 14px;
  border-radius: 10px;
}
.login-form-card :deep(.n-card__content) {
  padding: 30px 32px 32px;
}
.login-form-card :deep(.n-input) {
  border-radius: 10px;
}
.login-form-card :deep(.n-button) {
  border-radius: 10px;
  font-weight: 600;
}
</style>
