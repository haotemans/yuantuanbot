<template>
  <div class="provider-card">
    <div class="pc-head">
      <span class="pc-status" :class="{ ok: p.api_key_present }" :title="p.api_key_present ? 'API key 已就绪' : 'API key 未配置（或端点无需）'" />
      <span class="pc-name mono">{{ name }}</span>
      <template v-if="usage && usage.length">
        <n-tag v-for="r in usage" :key="r" size="tiny" round type="info" class="pc-usage">{{ r }}</n-tag>
      </template>
      <n-popconfirm @positive-click="$emit('delete')">
        <template #trigger>
          <n-button text size="tiny" type="error" class="pc-del">删除</n-button>
        </template>
        删除 provider {{ name }}？使用它的角色会被解绑。
      </n-popconfirm>
    </div>
    <div class="pc-body">
      <div class="pc-field">
        <label class="pc-label">base_url</label>
        <n-input v-model:value="p.base_url" size="small"
                 placeholder="https://api.example.com/v1" />
      </div>
      <div class="pc-field">
        <label class="pc-label">models（逗号分隔）</label>
        <n-input v-model:value="p.modelsText" size="small"
                 placeholder="gpt-4o-mini, gpt-4o" />
      </div>
      <div class="pc-field">
        <label class="pc-label">api_key 环境变量名（密钥不回显）</label>
        <n-input v-model:value="p.api_key_env" size="small"
                 placeholder="OPENAI_API_KEY" />
      </div>
    </div>
  </div>
</template>

<script setup>
// Provider 编辑卡：p 为父级响应式对象引用，v-model 直改嵌套字段即回写父状态
defineProps({
  name: { type: String, required: true },
  p: { type: /** @type {import('vue').PropType<{ base_url: string, modelsText: string, api_key_env: string, api_key_present: boolean }>} */ (Object), required: true },
  usage: { type: /** @type {import('vue').PropType<string[]>} */ (Array), default: () => [] },
})
defineEmits(['delete'])
</script>

<style scoped>
.provider-card {
  border: 1px solid var(--yt-card-border);
  border-radius: 12px;
  padding: 14px;
  background: var(--yt-header-bg);
  backdrop-filter: blur(4px);
  transition: border-color 0.15s ease, box-shadow 0.15s ease, transform 0.15s ease;
}
.provider-card:hover {
  border-color: rgba(79, 70, 229, 0.35);
  box-shadow: var(--yt-card-shadow);
  transform: translateY(-1px);
}
.pc-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
}
.pc-status {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--yt-ink-3);
  flex: none;
}
.pc-status.ok {
  background: #16a34a;
  box-shadow: 0 0 0 3px rgba(22, 163, 74, 0.14);
}
.pc-name {
  font-weight: 700;
  font-size: 13.5px;
  color: var(--yt-ink-1);
}
.pc-usage {
  margin-left: 2px;
}
.pc-del {
  margin-left: auto;
}
.pc-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.pc-field {}
.pc-label {
  display: block;
  font-size: 11.5px;
  color: var(--yt-ink-3);
  margin-bottom: 4px;
}
</style>
