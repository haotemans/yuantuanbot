import { defineStore } from 'pinia'

// 顶栏全局态：mood 显示（仪表盘页刷新时同步）、主题
export const useUiStore = defineStore('ui', {
  state: () => ({
    mood: 'calm',
    dark: localStorage.getItem('yt_theme') === 'dark',
  }),
  actions: {
    setMood(m) { if (m) this.mood = m },
    toggleTheme() {
      this.dark = !this.dark
      localStorage.setItem('yt_theme', this.dark ? 'dark' : 'light')
    },
  },
})
