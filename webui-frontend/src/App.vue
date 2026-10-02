<template>
  <n-config-provider :theme="ui.dark ? darkTheme : null">
    <n-message-provider>
      <router-view v-if="$route.name === 'login'" />
      <n-layout v-else style="height: 100vh">
        <n-layout-header bordered style="height: 48px; display: flex; align-items: center; padding: 0 16px; gap: 12px">
          <span style="font-weight: 600">云团 WebUI</span>
          <n-tooltip>
            <template #trigger>
              <n-badge :type="wsConnected ? 'success' : 'error'" dot processing />
            </template>
            {{ wsConnected ? 'NapCat/事件流 已连接' : '事件流断开（重连中）' }}
          </n-tooltip>
          <n-tag size="small" :type="moodTag">mood: {{ ui.mood }}</n-tag>
          <div style="margin-left: auto; display: flex; align-items: center; gap: 10px">
            <n-switch v-model:value="ui.dark" size="small" @update:value="ui.toggleTheme">
              <template #checked>暗</template>
              <template #unchecked>亮</template>
            </n-switch>
            <n-dropdown :options="adminOptions" @select="onAdmin">
              <n-button quaternary size="small">admin</n-button>
            </n-dropdown>
          </div>
        </n-layout-header>
        <n-layout has-sider position="absolute" style="top: 48px">
          <n-layout-sider bordered width="176" collapse-mode="width" :native-scrollbar="false">
            <n-menu :options="menuOptions" :value="String($route.name)" @update:value="go" />
          </n-layout-sider>
          <n-layout-content :native-scrollbar="false" content-style="padding: 16px;">
            <router-view />
          </n-layout-content>
        </n-layout>
      </n-layout>
    </n-message-provider>
  </n-config-provider>
</template>

<script setup>
import { computed, h, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { darkTheme } from 'naive-ui'
import { useUiStore } from './store/ui'
import { wsConnected, ensureWs } from './ws'
import { TOKEN_KEY, api } from './api'

const ui = useUiStore()
const route = useRoute()
const router = useRouter()

const moodTag = computed(() => ({ happy: 'success', calm: 'default', angry: 'error', down: 'warning' }[ui.mood] || 'default'))

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

onMounted(async () => {
  ensureWs()
  // 顶栏 mood 初值
  try {
    const { data } = await api.get('/dashboard')
    ui.setMood(data.mood)
  } catch { /* 登录态由拦截器处理 */ }
})
</script>
