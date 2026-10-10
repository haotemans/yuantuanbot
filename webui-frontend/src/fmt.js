// 时间/文案小工具：多页共用，语气和格式全站统一
export function fmtTs(ts) {
  return ts ? new Date(ts * 1000).toLocaleString() : '—'
}

export function fmtAgo(ts) {
  if (!ts) return '—'
  const s = Math.max(0, Math.floor(Date.now() / 1000 - ts))
  if (s < 60) return `${s} 秒前`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m} 分钟前`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时前`
  return `${Math.floor(h / 24)} 天前`
}

export function fmtUptime(s) {
  if (s == null) return '—'
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  return h > 0 ? `${h}h${m}m` : `${m}m${s % 60}s`
}

// 事件 kind 主色：trace 时间线、仪表盘迷你流、详情徽标共用一份
export const KIND_COLORS = {
  MessageReceived: 'var(--yt-ink-3)',
  DecisionMade: 'var(--yt-primary)',
  BubbleSent: 'var(--yt-primary)',
  ReplyInterrupted: 'var(--yt-danger)',
  ConsolidationDone: 'var(--yt-primary)',
  MemoryWritten: 'var(--yt-ink-3)',
  MoodChanged: 'var(--yt-ink-3)',
  ConfigReloaded: 'var(--yt-ink-3)',
}
export function kindColor(kind) {
  return KIND_COLORS[kind] || 'var(--yt-ink-3)'
}

// DecisionMade 的 action 语义色：卡片头部色条与徽标共用
export const ACTION_COLORS = {
  reply: 'var(--yt-ok)',
  ignore: 'var(--yt-ink-3)',
  send_meme: 'var(--yt-primary)',
  start_task: 'var(--yt-warning)',
}
export function actionColor(action) {
  return ACTION_COLORS[action] || 'var(--yt-ink-3)'
}

export const KIND_LABELS = { MessageReceived: '收到消息', DecisionMade: '参与决策', BubbleSent: '发送回复', ReplyInterrupted: '回复中断', ConsolidationDone: '归纳完成', MemoryWritten: '写入记忆', MoodChanged: '情绪变化', ConfigReloaded: '配置更新', TaskCreated: '创建任务', TaskFinished: '任务结束' }
export function kindLabel(kind) { return KIND_LABELS[kind] || kind }

// 事件行摘要：trace 列表与仪表盘迷你流统一口径
export function eventSummary(e) {
  const p = e.payload || {}
  if (e.kind === 'DecisionMade') return `${p.action} / ${p.mood} / ${p.reason ?? ''}`
  if (p.text) return String(p.text)
  if (p.chat_id) return String(p.chat_id)
  return e.kind
}
