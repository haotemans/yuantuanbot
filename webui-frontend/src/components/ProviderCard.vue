<template>
  <div class="provider-card">
    <div style="display: flex; align-items: center; gap: 8px; margin-bottom: 10px">
      <n-badge :type="p.api_key_present ? 'success' : 'default'" dot processing />
      <span style="font-weight: 700">{{ name }}</span>
      <n-tag size="tiny" :type="p.api_key_present ? 'success' : 'default'" round>
        {{ p.api_key_present ? 'key 就绪' : 'key 未配置 / 无需' }}
      </n-tag>
      <n-button text size="tiny" type="error" style="margin-left: auto" @click="$emit('delete')">删除</n-button>
    </div>
    <n-space vertical :size="8">
      <n-input v-model:value="p.base_url" size="small" placeholder="base_url（OpenAI 兼容，如 https://api.example.com/v1）" />
      <n-input v-model:value="p.modelsText" size="small" placeholder="models（逗号分隔）" />
      <n-input v-model:value="p.api_key_env" size="small" placeholder="api_key 环境变量名（密钥不回显，留在服务器环境里）" />
    </n-space>
  </div>
</template>

<script setup>
// Provider 编辑卡：p 为父级响应式对象引用，v-model 直改嵌套字段即回写父状态
defineProps({
  name: { type: String, required: true },
  p: { type: /** @type {import('vue').PropType<{ base_url: string, modelsText: string, api_key_env: string, api_key_present: boolean }>} */ (Object), required: true },
})
defineEmits(['delete'])
</script>

<style scoped>
.provider-card {
  border: 1px solid var(--yt-card-border);
  border-radius: 12px;
  padding: 14px;
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}
.provider-card:hover {
  border-color: rgba(79, 70, 229, 0.35);
  box-shadow: var(--yt-card-shadow);
}
</style>
