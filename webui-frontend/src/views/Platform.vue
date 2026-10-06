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
                 placeholder="不回读；留空 = 保留后端原值" />
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
/** @type {import('vue').Ref<Record<string, any>>} */
const whole = ref({})
const saving = ref(false)
const msg = ref('')
const ok = ref(false)

onMounted(async () => {
  const { data } = await api.get('/config')
  whole.value = data.config
  const n = data.config.napcat || {}
  // token 不回读（后端掩码返回），只让用户输入新值；whole 里清掉避免保存时把掩码回写覆盖真 token
  form.value = { enabled: n.enabled ?? true, ws_url: n.ws_url ?? '', token: '' }
  if (whole.value.napcat) whole.value.napcat = { ...whole.value.napcat, token: undefined }
})

async function save() {
  saving.value = true
  msg.value = ''
  try {
    const newNapcat = { ...whole.value.napcat }
    newNapcat.enabled = form.value.enabled
    newNapcat.ws_url = form.value.ws_url
    // 只在用户显式输入了新 token 时才写入；否则保留后端原值（不写回掩码）
    if (form.value.token) newNapcat.token = form.value.token
    else delete newNapcat.token
    const config = { ...whole.value, napcat: newNapcat }
    const { data } = await api.post('/config', { config })
    ok.value = true
    msg.value = `已保存（${(data.applied || []).join('、') || '无热应用项'}）`
    whole.value = config
    form.value.token = '' // 提交成功后清空，避免下次保存又把新 token 当旧值
  } catch (e) {
    ok.value = false
    msg.value = e.response?.data?.error || '保存失败'
  } finally {
    saving.value = false
  }
}
</script>
