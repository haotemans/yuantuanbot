<template>
  <n-config-provider :theme="ui.dark ? darkTheme : null"
                     :theme-overrides="ui.dark ? darkOverrides : lightOverrides">
    <n-message-provider>
      <router-view v-if="$route.name === 'login'" />
      <n-layout v-else style="height: 100vh">
        <n-layout-header class="yt-header" style="height: 54px; display: flex; align-items: center; padding: 0 20px; gap: 16px">
          <n-button quaternary circle size="small" @click="ui.toggleSider" class="yt-icon-btn">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M4 6h16M4 12h16M4 18h16" />
            </svg>
          </n-button>
          <div class="yt-brand">
            <img :src="mascot" class="yt-brand-logo" alt="云团" />
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
            <n-switch v-model:value="ui.dark" size="small" @update:value="ui.toggleTheme" class="yt-theme-switch">
              <template #checked>暗</template>
              <template #unchecked>亮</template>
            </n-switch>
            <n-dropdown :options="adminOptions" @select="onAdmin">
              <n-button quaternary size="small">admin</n-button>
            </n-dropdown>
          </div>
        </n-layout-header>
        <n-layout has-sider position="absolute" style="top: 54px">
          <n-layout-sider class="yt-sider" bordered collapse-mode="width" :width="200" :collapsed-width="64"
                          :collapsed="ui.collapsed" :native-scrollbar="false">
            <n-menu :options="menuOptions" :value="String($route.name)" :collapsed="ui.collapsed"
                    :collapsed-icon-size="20" :root-indent="22" :indent="16" style="padding: 4px 8px" @update:value="go" />
          </n-layout-sider>
          <n-layout-content :native-scrollbar="false" content-style="padding: 20px 22px;">
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
import { darkTheme, NTooltip } from 'naive-ui'
import { useUiStore } from './store/ui'
import { lightOverrides, darkOverrides } from './theme'
import { wsConnected, ensureWs } from './ws'
import { TOKEN_KEY, api } from './api'
import YtIcon from './components/YtIcon.vue'
import mascot from './assets/mascot.png'

const ui = useUiStore()
const route = useRoute()
const router = useRouter()

const moodTag = computed(() => ({ happy: 'success', calm: 'default', angry: 'error', down: 'warning' }[ui.mood] || 'default'))

// 菜单：[分组, [路由名, 文案, 图标]]；折叠态图标外包 tooltip 补名字
/** @type {[string, [string, string, string][]][]} */
const groups = [
  ['总览', [['dashboard', '仪表盘', 'dashboard']]],
  ['观察', [
    ['trace', 'Decision trace', 'trace'],
    ['tasks', '任务回放', 'tasks'],
    ['memories', '记忆浏览', 'memories'],
    ['relations', '关系网', 'relations'],
  ]],
  ['配置', [
    ['platform', '平台连接', 'platform'],
    ['models', '模型', 'models'],
    ['params', '运行参数', 'tune'],
    ['personality', '人格', 'personality'],
    ['meme', 'Meme', 'meme'],
    ['kb', '知识库', 'kb'],
  ]],
  ['系统', [['backup', '备份 / 日志', 'backup']]],
]
const iconWithTip = (icon, label) => () =>
  h(NTooltip, { trigger: 'hover', placement: 'right', disabled: !ui.collapsed }, {
    trigger: () => h(YtIcon, { name: icon, size: 20 }),
    default: () => label,
  })
const menuOptions = groups.map(([label, children]) => ({
  type: 'group', label, key: label,
  children: children.map(([key, l, icon]) => ({ label: () => h('span', l), key, icon: iconWithTip(icon, l) })),
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
