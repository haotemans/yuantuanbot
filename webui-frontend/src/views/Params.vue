<template>
  <div class="yt-page">
    <n-skeleton v-if="!loaded" text :repeat="10" />
    <template v-else>
      <n-grid cols="1 m:2" :x-gap="14" :y-gap="14" responsive="screen">
        <n-gi>
          <n-card title="夜间归纳" size="small">
            <n-form label-placement="left" label-width="140">
              <n-form-item label="启用">
                <n-switch v-model:value="f.consolidation.enabled" />
              </n-form-item>
              <n-form-item label="每日时刻">
                <n-time-picker :formatted-value="f.consolidation.daily_time" format="HH:mm"
                               value-format="HH:mm" style="width: 140px"
                               @update:formatted-value="(v) => (f.consolidation.daily_time = v || '03:00')" />
              </n-form-item>
              <n-form-item label="启动即跑一次">
                <n-switch v-model:value="f.consolidation.run_on_startup" />
              </n-form-item>
            </n-form>
            <p class="param-hint">调试用开关，生产保持关闭；改时刻会取消旧定时器按新时刻重建（热应用）。</p>
          </n-card>

          <n-card title="上下文预算" size="small" style="margin-top: 14px">
            <n-form label-placement="left" label-width="140">
              <n-form-item label="预算（字符）">
                <n-input-number v-model:value="f.context.budget_chars" :min="4000" :max="200000" :step="1000" style="width: 160px" />
              </n-form-item>
              <n-form-item label="会话窗口 K（条）">
                <n-input-number v-model:value="f.context.k" :min="5" :max="100" style="width: 160px" />
              </n-form-item>
              <n-form-item label="话题记忆条数">
                <n-input-number v-model:value="f.context.roster_mem_per" :min="0" :max="10" style="width: 160px" />
              </n-form-item>
            </n-form>
            <p class="param-hint">按字符控制本轮上下文；超预算先减少近期对话，再减少记忆和人物资料。当前问题与引用优先保留，必要内容放不下时停止生成。</p>
          </n-card>

          <n-card title="节流" size="small" style="margin-top: 14px">
            <n-form label-placement="left" label-width="140">
              <n-form-item label="节流窗口（秒）">
                <n-input-number v-model:value="f.prefilter.window_secs" :min="5" :max="600" style="width: 160px" />
              </n-form-item>
              <n-form-item label="窗口内气泡硬顶">
                <n-input-number v-model:value="f.prefilter.self_msg_cap" :min="1" :max="100" style="width: 160px" />
              </n-form-item>
              <n-form-item label="Decision 成本闸（次/分）">
                <n-input-number v-model:value="f.prefilter.decision_cost_per_min" :min="1" :max="120" style="width: 160px" />
              </n-form-item>
            </n-form>
          </n-card>

          <n-card title="消息管线" size="small" style="margin-top: 14px">
            <n-form label-placement="left" label-width="180">
              <n-form-item label="单 chat 队列容量">
                <n-input-number v-model:value="f.pipeline.per_chat_queue_cap" :min="4" :max="512" style="width: 160px" />
              </n-form-item>
              <n-form-item label="自发消息 ID 缓存">
                <n-input-number v-model:value="f.pipeline.self_msg_ids_cap" :min="32" :max="4096" style="width: 160px" />
              </n-form-item>
              <n-form-item label="Decision 成本闸初始值">
                <n-input-number v-model:value="f.pipeline.decision_cost_per_min_init" :min="1" :max="120" style="width: 160px" />
              </n-form-item>
            </n-form>
            <p class="param-hint">
              Q52 单 chat 洪峰降级阈值（只影响新 worker）· R4「回复我」判定缓存大小 · 成本闸启动值（运行期被「Decision 成本闸」覆盖）。
            </p>
          </n-card>
        </n-gi>

        <n-gi>
          <n-card title="回复形态" size="small">
            <n-form label-placement="left" label-width="160">
              <n-form-item label="泡数封顶">
                <n-input-number v-model:value="f.reply.bubble_cap" :min="1" :max="5" style="width: 160px" />
              </n-form-item>
              <n-form-item label="拆句目标字数">
                <n-input-number v-model:value="f.reply.bubble_char_cap" :min="20" :max="2000" :step="10" style="width: 160px" />
              </n-form-item>
              <n-form-item label="延时系数（ms/字）">
                <n-input-number v-model:value="f.reply.per_char_ms" :min="0" :max="500" style="width: 160px" />
              </n-form-item>
              <n-form-item label="首泡延时下限（ms）">
                <n-input-number v-model:value="f.reply.first_delay_min_ms" :min="0" :max="10000" :step="50" style="width: 160px" />
              </n-form-item>
              <n-form-item label="首泡延时上限（ms）">
                <n-input-number v-model:value="f.reply.first_delay_max_ms" :min="0" :max="30000" :step="50" style="width: 160px" />
              </n-form-item>
              <n-form-item label="单泡延时泡顶（ms）">
                <n-input-number v-model:value="f.reply.max_delay_ms" :min="100" :max="30000" :step="100" style="width: 160px" />
              </n-form-item>
              <n-form-item label="延时总预算（秒）">
                <n-input-number v-model:value="totalBudgetSecs" :min="1" :max="60" style="width: 160px" />
              </n-form-item>
            </n-form>
            <p class="param-hint">打字模拟 = 基线 + 字数 × 系数（带抖动，夹在上下限之间）；总预算是整条回复允许花在延时上的总时长。</p>
          </n-card>

          <n-card title="Meme" size="small" style="margin-top: 14px">
            <n-form label-placement="left" label-width="160">
              <n-form-item label="偷表情包（进群图自动入待审）">
                <n-switch v-model:value="f.meme.steal_enabled" />
              </n-form-item>
            </n-form>
          </n-card>
        </n-gi>
      </n-grid>

      <div class="yt-toolbar" style="margin-top: 14px">
        <n-button type="primary" size="small" :loading="saving" @click="save">保存运行参数</n-button>
        <span v-if="text" :style="{ color: ok ? '#16a34a' : '#dc2626', fontSize: '12.5px' }">{{ text }}</span>
      </div>
      <n-alert v-if="restartList.length" type="warning" style="margin-top: 10px">
        以下项需重启进程生效：{{ restartList.join('、') }}
      </n-alert>
    </template>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'

const loaded = ref(false)
const saving = ref(false)
const text = ref('')
const ok = ref(false)
const restartList = ref([])

// 与后端出厂默认对齐；GET 回读覆盖
const f = ref({
  consolidation: { enabled: true, daily_time: '03:00', run_on_startup: false },
  context: { budget_chars: 40000, k: 20, roster_mem_per: 3 },
  reply: {
    first_delay_min_ms: 300, first_delay_max_ms: 800, base_delay_ms: 600,
    per_char_ms: 40, jitter_ratio: 0.3, min_delay_ms: 800, max_delay_ms: 4000,
    total_budget_ms: 8000, bubble_cap: 3, bubble_char_cap: 500,
  },
  prefilter: { window_secs: 60, self_msg_cap: 12, decision_cost_per_min: 30 },
  pipeline: {
    per_chat_queue_cap: 32,
    self_msg_ids_cap: 512,
    decision_cost_per_min_init: 30,
  },
  meme: { steal_enabled: true },
})
/** 原始 config 全量（GET 快照，保存时合并回写，避免吞掉 napcat/webui 等面板外的节） */
const whole = ref({})

// 秒 ↔ 毫秒桥（总预算在面板以秒呈现，落盘毫秒）
const totalBudgetSecs = computed({
  get: () => Math.round(f.value.reply.total_budget_ms / 1000),
  set: (v) => { f.value.reply.total_budget_ms = Math.max(1, v || 1) * 1000 },
})

onMounted(async () => {
  const { data } = await api.get('/config')
  const c = data.config || {}
  whole.value = c
  for (const k of ['consolidation', 'context', 'reply', 'prefilter', 'pipeline', 'meme']) {
    if (c[k]) f.value[k] = { ...f.value[k], ...c[k] }
  }
  loaded.value = true
})

async function save() {
  saving.value = true
  text.value = ''
  restartList.value = []
  try {
    const config = {
      ...whole.value,
      consolidation: { ...f.value.consolidation },
      context: { ...f.value.context },
      reply: { ...f.value.reply },
      prefilter: { ...f.value.prefilter },
      pipeline: { ...f.value.pipeline },
      meme: { ...f.value.meme },
    }
    const { data } = await api.post('/config', { config })
    ok.value = true
    whole.value = config
    text.value = `已保存并热应用：${(data.applied || []).join('、') || '无'}`
    restartList.value = data.requires_restart || []
  } catch (e) {
    ok.value = false
    text.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}
</script>

<style scoped>
.param-hint {
  margin: 10px 0 0;
  font-size: 12px;
  color: var(--yt-text-dim);
  line-height: 1.7;
}
</style>
