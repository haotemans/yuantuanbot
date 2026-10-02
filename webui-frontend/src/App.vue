<template>
  <n-config-provider :theme="ui.dark ? darkTheme : null"
                     :theme-overrides="ui.dark ? darkOverrides : lightOverrides">
    <n-message-provider>
      <router-view v-if="$route.name === 'login'" />
      <n-layout v-else style="height: 100vh">
        <n-layout-header class="yt-header" style="height: 54px; display: flex; align-items: center; padding: 0 18px; gap: 14px">
          <div class="yt-brand">
            <span class="yt-brand-logo">云</span>
            <span class="yt-brand-name">云团</span>
            <span class="yt-brand-sub">群聊 Agent 运维面板</span>
          </div>
          <n-tooltip>
            <template #trigger>
              <n-badge :type="wsConnected ? 'success' : 'error'" dot processing />
            </template>
            {{ wsConnected ? '事件流已连接' : '事件流断开（自动重连中）' }}
          </n-tooltip>
          <n-tag size="small" :type="moodTag" round>mood · {{ ui.mood }}</n-tag>
          <div style="margin-left: auto; display: flex; align-items: center; gap: 12px">
            <n-switch v-model:value="ui.dark" size="small" @update:value="ui.toggleTheme">
              <template #checked>暗</template>
              <template #unchecked>亮</template>
            </n-switch>
            <n-dropdown :options="adminOptions" @select="onAdmin">
              <n-button quaternary size="small">admin</n-button>
            </n-dropdown>
          </div>
        </n-layout-header>
        <n-layout has-sider position="absolute" style="top: 54px">
          <n-layout-sider bordered width="184" collapse-mode="width" :native-scrollbar="false">
            <n-menu :options="menuOptions" :value="String($route.name)" style="padding: 6px 0" @update:value="go" />
          </n-layout-sider>
          <n-layout-content :native-scrollbar="false" content-style="padding: 18px 20px;">
            <router-view v-slot="{ Component }">
              <transition name="fade" mode="out-in">
                <component :is="Component" />
              </transition>
            </router-view>
          </n-layout-content>
        </n-layout>
      </n-layout>
    </n-message-provider>
  </n-config-provider>
</template>

<script setup>
import { computed, h, onMounted, watchEffect } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { darkTheme } from 'naive-ui'
import { useUiStore } from './store/ui'
import { lightOverrides, darkOverrides } from './theme'
import { wsConnected, ensureWs } from './ws'
import { TOKEN_KEY, api } from './api'

const ui = useUiStore()
const route = useRoute()
const router = useRouter()

const moodTag = computed(() => ({ happy: 'success', calm: 'default', angry: 'error', down: 'warning' }[ui.mood] || 'default'))

/** @type {[string, [string, string][]][]} */
const groups = [
  ['总览', [['dashboard', '仪表盘']]],
  ['观察', [['trace', 'Decision trace'], ['tasks', '任务回放'], ['memories', '记忆浏览'], ['relations', '关系网']]],
  ['配置', [['platform', '平台连接'], ['models', '模型'], ['personality', '人格'], ['meme', 'Meme'], ['kb', '知识库']]],
  ['系统', [['backup', '备份 / 日志']]],
]
const menuOptions = groups.map(([label, children]) => ({
  type: 'group', label, key: label,
  children: children.map(([key, l]) => ({ label: () => h('span', l), key })),
}))

function go(key) { router.push({ name: key }) }

const adminOptions = [{ label: '退出登录', key: 'logout' }]
function onAdmin(key) {
  if (key === 'logout') {
    localStorage.removeItem(TOKEN_KEY)
    router.push({ name: 'login' })
  }
}

// 暗色开关同步到 <html>，供全局 CSS 变量双套切换
watchEffect(() => {
  document.documentElement.classList.toggle('dark', ui.dark)
})

onMounted(async () => {
  ensureWs()
  // 顶栏 mood 初值
  try {
    const { data } = await api.get('/dashboard')
    ui.setMood(data.mood)
  } catch { /* 登录态由拦截器处理 */ }
})
</script>
