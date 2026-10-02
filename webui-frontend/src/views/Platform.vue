<template>
  <n-card title="平台连接（NapCat）" style="max-width: 560px" size="small">
    <n-form label-placement="left" label-width="110">
      <n-form-item label="启用">
        <n-switch v-model:value="form.enabled" />
      </n-form-item>
      <n-form-item label="WS 地址">
        <n-input v-model:value="form.ws_url" placeholder="ws://127.0.0.1:3001" />
      </n-form-item>
      <n-form-item label="Token">
        <n-input v-model:value="form.token" type="password" show-password-on="click"
                 placeholder="掩码不回读；留空保存=清空" />
      </n-form-item>
    </n-form>
    <n-space>
      <n-button type="primary" size="small" :loading="saving" @click="save">保存连接配置</n-button>
      <span v-if="msg" :style="{ color: ok ? '#16a34a' : '#dc2626', fontSize: '12px' }">{{ msg }}</span>
    </n-space>
    <n-alert type="info" style="margin-top: 10px; font-size: 12px">
      保存立即写回并触发热应用事件；adapter 重连等组件级生效在后续热配单完善（当前 adapter 重启进程生效）。
    </n-alert>
  </n-card>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'

const form = ref({ enabled: true, ws_url: '', token: '' })
const whole = ref({})
const saving = ref(false)
const msg = ref('')
const ok = ref(false)

onMounted(async () => {
  const { data } = await api.get('/config')
  whole.value = data.config
  const n = data.config.napcat || {}
  form.value = { enabled: n.enabled ?? true, ws_url: n.ws_url ?? '', token: '' }
})

async function save() {
  saving.value = true
  msg.value = ''
  try {
    const config = { ...whole.value, napcat: { ...form.value } }
    if (!config.napcat.token) delete config.napcat.token // 留空不写入该键（默认空）
    const { data } = await api.post('/config', { config })
    ok.value = true
    msg.value = `已保存（${(data.applied || []).join('、') || '无热应用项'}）`
    whole.value = config
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}
</script>
