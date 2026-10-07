<template>
  <div class="cap-card" :class="{ 'is-disabled': !server.enabled, 'is-dirty': dirty }" @click="$emit('open', server)">
    <div class="cap-card-head">
      <div class="cap-card-icon mcp">
        {{ server.name[0].toUpperCase() }}
      </div>
      <div class="cap-card-title">
        <div class="cap-card-name">{{ server.name }}</div>
        <div class="cap-card-status">
          <span v-if="server.running" class="ok">运行中</span>
          <span v-else-if="server.enabled" class="warn">待重启</span>
          <span v-else>已禁用</span>
        </div>
      </div>
    </div>

    <div class="cap-card-body">
      <div class="cap-card-meta">
        {{ server.command }} {{ (server.args || []).join(' ') }}
      </div>
      <div class="cap-card-tags">
        <span v-if="server.running && (server.tools || []).length" class="chip tool">
          🔧 {{ server.tools.length }} tool
        </span>
        <span v-if="(server.env_keys || []).length" class="chip">🔐 {{ server.env_keys.length }} env</span>
      </div>
    </div>

    <div class="cap-card-foot" @click.stop>
      <n-switch
        size="small"
        :value="server.enabled"
        @update:value="(v) => $emit('toggle', server, v)"
      />
      <span class="spacer" />
      <span class="cap-card-open" @click="$emit('open', server)">打开</span>
    </div>
  </div>
</template>

<script setup>
defineProps({
  server: { type: Object, required: true },
  dirty: { type: Boolean, default: false },
})
defineEmits(['toggle', 'open'])
</script>

<style scoped>
.ok { color: var(--yt-ok); font-weight: 600; }
.warn { color: #d97706; font-weight: 600; }
</style>
