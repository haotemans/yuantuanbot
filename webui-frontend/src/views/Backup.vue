<template>
  <div class="yt-page">
    <div class="yt-page-head">
      <span class="yt-page-title">备份 / 恢复</span>
      <span class="yt-page-sub">GitHub 私有仓 + 本地 tar.gz</span>
      <span class="spacer" />
      <n-button size="tiny" secondary @click="reload">刷新</n-button>
    </div>

    <!-- 状态卡 -->
    <n-grid cols="1 s:2 l:4" :x-gap="14" :y-gap="14" responsive="screen">
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" :style="{ background: cfg.enabled && cfg.repo_url ? '#16a34a' : '#d97706' }" />
            状态
          </div>
          <div class="stat-foot">
            <span class="stat-num">{{ cfg.enabled && cfg.repo_url ? '已启用' : '未配置' }}</span>
          </div>
          <div class="stat-hint">{{ cfg.enabled ? '每日 ' + cfg.daily_time + ' 自动备份' : '配置后才启用推送' }}</div>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" style="background: #06b6d4" />
            本地备份点
          </div>
          <div class="stat-foot">
            <span class="stat-num">{{ backups.length }}</span>
          </div>
          <div class="stat-hint">保留最近 {{ cfg.keep_days }} 天</div>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" style="background: #4f46e5" />
            总大小
          </div>
          <div class="stat-foot">
            <span class="stat-num">{{ totalSize }}</span>
          </div>
          <div class="stat-hint">所有 tar.gz 合计</div>
        </n-card>
      </n-gi>
      <n-gi>
        <n-card class="stat-card" size="small">
          <div class="stat-label">
            <span class="stat-chip" style="background: #d97706" />
            最近备份
          </div>
          <div class="stat-foot">
            <span class="stat-num" style="font-size: 14px">{{ lastBackupAgo }}</span>
          </div>
          <div class="stat-hint">{{ lastBackupName || '还没有备份' }}</div>
        </n-card>
      </n-gi>
    </n-grid>

    <!-- 操作条 -->
    <n-card size="small" style="margin-top: 14px">
      <div class="action-row">
        <n-button type="primary" :loading="running" @click="runNow">
          {{ running ? '备份中…' : '立即备份' }}
        </n-button>
        <span v-if="lastRun" class="run-msg" :class="{ ok: lastRunOk, fail: !lastRunOk }">{{ lastRun }}</span>
      </div>
    </n-card>

    <!-- 配置 -->
    <n-card title="GitHub 推送" size="small" style="margin-top: 14px">
      <n-form label-placement="left" label-width="120" style="max-width: 720px">
        <n-form-item label="启用">
          <n-switch v-model:value="cfg.enabled" />
          <span class="form-hint">开启后每日 {{ cfg.daily_time }} 自动备份并推送</span>
        </n-form-item>
        <n-form-item label="仓库 URL">
          <n-input v-model:value="cfg.repo_url" placeholder="https://github.com/<you>/yuantuan-backup.git" />
        </n-form-item>
        <n-form-item label="PAT 环境变量">
          <n-input v-model:value="cfg.pat_env" placeholder="YUANTUAN_BACKUP_PAT" />
          <span class="form-hint">服务器上需 export 同名变量为 fine-grained PAT（contents:write）</span>
        </n-form-item>
        <n-form-item label="每日时间">
          <n-input v-model:value="cfg.daily_time" placeholder="03:00" style="width: 140px" />
        </n-form-item>
        <n-form-item label="本地保留">
          <n-input-number v-model:value="cfg.keep_days" :min="1" :max="90" style="width: 140px" />
          <span class="form-hint">天；超期自动删除本地 tar.gz</span>
        </n-form-item>
      </n-form>
      <n-space>
        <n-button type="primary" secondary :loading="saving" @click="save">保存配置（热应用）</n-button>
        <span v-if="saveMsg" :style="{ color: saveOk ? '#16a34a' : '#dc2626', fontSize: '12px' }">{{ saveMsg }}</span>
      </n-space>
    </n-card>

    <!-- 历史备份 -->
    <n-card size="small" style="margin-top: 14px">
      <template #header>
        <div class="sec-head">
          <span class="sec-title">历史备份</span>
          <span class="sec-sub">{{ backups.length }} 个备份点</span>
        </div>
      </template>
      <n-data-table
        v-if="backups.length"
        :columns="cols"
        :data="backups"
        size="small"
        :pagination="{ pageSize: 10 }"
      />
      <empty-state v-else title="还没有备份" hint="点上方「立即备份」跑一次，或开启每日定时" />
    </n-card>

    <!-- 恢复说明 -->
    <n-alert type="info" style="margin-top: 14px">
      <b>恢复流程</b>：选备份点 → 点「恢复」→ 后端写恢复标记 → <b>手动重启 yuantuan 进程</b> → 启动时自动解压覆盖 data/
      → 启动完成就是恢复后的状态。当前进程不会被强杀。
    </n-alert>
  </div>
</template>

<script setup>
import { computed, h, onMounted, ref } from 'vue'
import { NButton, NPopconfirm, NTag } from 'naive-ui'
import { api } from '../api'
import EmptyState from '../components/EmptyState.vue'

const cfg = ref({ enabled: false, repo_url: '', pat_env: 'YUANTUAN_BACKUP_PAT', daily_time: '03:00', keep_days: 7 })
const backups = ref([])
const running = ref(false)
const saving = ref(false)
const lastRun = ref('')
const lastRunOk = ref(false)
const saveMsg = ref('')
const saveOk = ref(false)

const totalSize = computed(() => {
  const sum = backups.value.reduce((a, b) => a + (b.size_bytes || 0), 0)
  return fmtSize(sum)
})
const lastBackupName = computed(() => backups.value[0]?.name || '')
const lastBackupAgo = computed(() => {
  const t = backups.value[0]?.created_at
  if (!t) return '—'
  const diff = Math.floor(Date.now() / 1000) - t
  if (diff < 60) return `${diff}s 前`
  if (diff < 3600) return `${Math.floor(diff / 60)}m 前`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h 前`
  return `${Math.floor(diff / 86400)}d 前`
})

const cols = [
  { title: '文件', key: 'name', render: (r) => h('code', { style: 'font-size: 12px' }, r.name) },
  { title: '大小', key: 'size_bytes', width: 110, render: (r) => fmtSize(r.size_bytes) },
  { title: '时间', key: 'created_at', width: 170, render: (r) => new Date(r.created_at * 1000).toLocaleString() },
  {
    title: '操作',
    key: 'op',
    width: 200,
    render: (r) =>
      h('div', { style: 'display: flex; gap: 8px' }, [
        h(NButton, { size: 'tiny', secondary: true, onClick: () => downloadFile(r.name) }, () => '下载'),
        h(
          NPopconfirm,
          { onPositiveClick: () => restore(r.name) },
          {
            trigger: () => h(NButton, { size: 'tiny', type: 'warning', secondary: true }, () => '恢复'),
            default: () => `用 ${r.name} 覆盖 data/？需要手动重启 yuantuan`,
          }
        ),
      ]),
  },
]

function fmtSize(n) {
  if (!n) return '—'
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`
}

async function loadConfig() {
  const { data } = await api.get('/config')
  const b = data.config.backup || {}
  cfg.value = {
    enabled: !!b.enabled,
    repo_url: b.repo_url || '',
    pat_env: b.pat_env || 'YUANTUAN_BACKUP_PAT',
    daily_time: b.daily_time || '03:00',
    keep_days: b.keep_days ?? 7,
  }
}

async function loadBackups() {
  try {
    const { data } = await api.get('/backup/list')
    backups.value = data.backups || []
  } catch { /* ignore */ }
}

async function reload() {
  await Promise.all([loadConfig(), loadBackups()])
}

async function runNow() {
  running.value = true
  lastRun.value = ''
  try {
    const { data } = await api.post('/backup/run')
    lastRunOk.value = true
    const pushed = data.pushed ? ' · 已推送 GitHub' : (data.push_error ? ` · 推送失败：${data.push_error}` : ' · 仅本地')
    lastRun.value = `✓ ${data.file} · ${fmtSize(data.size_bytes)} · ${data.elapsed_ms}ms${pushed}`
    await loadBackups()
  } catch (e) {
    lastRunOk.value = false
    lastRun.value = `✗ ${e.response?.data?.error || '备份失败'}`
  } finally {
    running.value = false
  }
}

async function save() {
  saving.value = true
  saveMsg.value = ''
  try {
    const whole = (await api.get('/config')).data.config
    whole.backup = { ...whole.backup, ...cfg.value }
    const { data } = await api.post('/config', { config: whole })
    saveOk.value = true
    saveMsg.value = `已保存（${(data.applied || []).join('、') || '无热应用项'}）`
  } catch (e) {
    saveOk.value = false
    saveMsg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}

function downloadFile(name) {
  // 直接打开新窗触发浏览器下载（带 token 走 query 不行，借 axios 拉 blob）
  api.get(`/backup/file/${name}`, { responseType: 'blob' }).then((r) => {
    const url = URL.createObjectURL(r.data)
    const a = document.createElement('a')
    a.href = url
    a.download = name
    a.click()
    URL.revokeObjectURL(url)
  })
}

async function restore(name) {
  try {
    const { data } = await api.post('/backup/restore', { file: name })
    lastRunOk.value = true
    lastRun.value = `✓ ${data.message}`
  } catch (e) {
    lastRunOk.value = false
    lastRun.value = `✗ ${e.response?.data?.error || '恢复标记写入失败'}`
  }
}

onMounted(reload)
</script>

<style scoped>
.stat-card { min-height: 108px; }
.stat-label {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--yt-ink-3);
  font-weight: 600;
}
.stat-chip {
  width: 8px;
  height: 8px;
  border-radius: 2px;
  flex: none;
}
.stat-foot { margin-top: 8px; }
.stat-num { font-size: 22px; font-weight: 700; color: var(--yt-ink-1); line-height: 1; }
.stat-hint { margin-top: 6px; font-size: 11.5px; color: var(--yt-ink-3); line-height: 1.4; }

.sec-head { display: flex; align-items: baseline; gap: 10px; }
.sec-title { font-weight: 700; font-size: 14px; color: var(--yt-ink-1); }
.sec-sub { font-size: 12px; color: var(--yt-ink-3); }

.action-row {
  display: flex;
  align-items: center;
  gap: 12px;
}
.run-msg { font-size: 12.5px; }
.run-msg.ok { color: #16a34a; }
.run-msg.fail { color: #dc2626; }

.form-hint {
  margin-left: 10px;
  font-size: 12px;
  color: var(--yt-ink-3);
}
</style>
