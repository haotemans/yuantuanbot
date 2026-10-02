import axios from 'axios'
import router from './router'

export const TOKEN_KEY = 'yt_token'

export const api = axios.create({ baseURL: '/api', timeout: 15000 })

api.interceptors.request.use((cfg) => {
  const t = localStorage.getItem(TOKEN_KEY)
  if (t) cfg.headers.Authorization = `Bearer ${t}`
  return cfg
})

// 401 全局登出跳登录
api.interceptors.response.use(
  (r) => r,
  (err) => {
    if (err.response?.status === 401) {
      localStorage.removeItem(TOKEN_KEY)
      if (router.currentRoute.value.name !== 'login') router.push({ name: 'login' })
    }
    return Promise.reject(err)
  }
)

export function isLoggedIn() {
  return !!localStorage.getItem(TOKEN_KEY)
}
