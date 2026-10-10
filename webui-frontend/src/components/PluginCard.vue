<template>
  <div class="cap-card" :class="{ 'is-disabled': !plugin.enabled, 'is-dirty': dirty }" @click="$emit('open', plugin)">
    <div class="cap-card-head">
      <div class="cap-card-icon plugin">
        {{ plugin.name[0].toUpperCase() }}
      </div>
      <div class="cap-card-title">
        <div class="cap-card-name">{{ plugin.name }}</div>
        <div class="cap-card-status">
          <span v-if="plugin.version" class="mono">v{{ plugin.version }} · </span>
          <span v-if="plugin.enabled && plugin.loaded" class="ok">已加载</span>
          <span v-else-if="plugin.enabled && !plugin.loaded" class="warn">待重启</span>
          <span v-else>已禁用</span>
        </div>
      </div>
    </div>

    <div class="cap-card-body">
      <div class="cap-card-desc">
        {{ plugin.description || '无描述；点开查看 skills/tools。' }}
      </div>
      <div class="cap-card-tags">
        <span v-if="(plugin.skills || []).length" class="chip skill">
          ⚡ {{ plugin.skills.length }} skill
        </span>
        <span v-if="(plugin.tools || []).length" class="chip tool">
          🔧 {{ plugin.tools.length }} tool
        </span>
        <span v-if="plugin.data_dir" class="chip">📁 data</span>
      </div>
    </div>

    <div class="cap-card-foot" @click.stop>
      <n-switch
        size="small"
        :value="plugin.enabled"
        @update:value="(v) => $emit('toggle', plugin, v)"
      />
      <span class="spacer" />
      <span class="cap-card-open" @click="$emit('open', plugin)">打开</span>
    </div>
  </div>
</template>

<script setup>
const props = defineProps({
  plugin: { type: Object, required: true },
  dirty: { type: Boolean, default: false },
})
defineEmits(['toggle', 'open'])
</script>

<style scoped>
.ok { color: var(--yt-ok); font-weight: 600; }
.warn { color: var(--yt-warning); font-weight: 600; }
</style>
