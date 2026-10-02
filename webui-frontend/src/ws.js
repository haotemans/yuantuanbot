import { ref } from 'vue'
import { TOKEN_KEY } from './api'

// 全局单条 /ws：顶栏连接灯 + 各页订阅（带 token query，浏览器 WS 无自定义头）
export const wsConnected = ref(false)
const listeners = new Set()

let ws = null
let retryTimer = null

function connect() {
  const token = localStorage.getItem(TOKEN_KEY)
  if (!token) return
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  ws = new WebSocket(`${proto}://${location.host}/ws?token=${encodeURIComponent(token)}`)
  ws.onopen = () => { wsConnected.value = true }
  ws.onclose = () => {
    wsConnected.value = false
    ws = null
    if (!retryTimer) retryTimer = setTimeout(() => { retryTimer = null; connect() }, 3000)
  }
  ws.onerror = () => { try { ws?.close() } catch { /* ignore */ } }
  ws.onmessage = (e) => {
    let ev
    try { ev = JSON.parse(e.data) } catch { return }
    for (const fn of listeners) fn(ev)
  }
}

export function ensureWs() {
  if (!ws && localStorage.getItem(TOKEN_KEY)) connect()
}

export function subscribeWs(fn) {
  listeners.add(fn)
  ensureWs()
  return () => listeners.delete(fn)
}

export function reconnectWs() {
  try { ws?.close() } catch { /* ignore */ }
  ws = null
  if (!retryTimer) retryTimer = setTimeout(() => { retryTimer = null; connect() }, 200)
}
