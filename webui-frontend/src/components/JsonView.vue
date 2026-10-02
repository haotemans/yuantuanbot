<template>
  <pre class="json-view"><code><span v-for="(t, i) in tokens" :key="i" :class="t.cls">{{ t.text }}</span></code></pre>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  data: { default: null },
})

// 极简 JSON 语法高亮：字符串/键/数字/布尔/null/标点六类，逐 token 渲染（Vue 自动转义，无 v-html）
const TOKEN_RE = /("(?:\\u[a-fA-F0-9]{4}|\\[^u]|[^\\"])*")(\s*:)?|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|\btrue\b|\bfalse\b|\bnull\b/g

const tokens = computed(() => {
  const src = JSON.stringify(props.data, null, 2) ?? ''
  const out = []
  let last = 0
  for (const m of src.matchAll(TOKEN_RE)) {
    const idx = m.index ?? 0
    if (idx > last) out.push({ cls: 'jk-punc', text: src.slice(last, idx) })
    const [whole, str, colon] = m
    if (str !== undefined) out.push({ cls: colon ? 'jk-key' : 'jk-str', text: whole + (colon || '') })
    else if (whole === 'true' || whole === 'false') out.push({ cls: 'jk-bool', text: whole })
    else if (whole === 'null') out.push({ cls: 'jk-null', text: whole })
    else out.push({ cls: 'jk-num', text: whole })
    last = idx + whole.length
  }
  if (last < src.length) out.push({ cls: 'jk-punc', text: src.slice(last) })
  return out
})
</script>

<style scoped>
.json-view {
  margin: 0;
  padding: 12px 14px;
  font-size: 12px;
  line-height: 1.65;
  overflow: auto;
  max-height: calc(100vh - 260px);
  border-radius: 8px;
  border: 1px solid var(--yt-card-border);
  background: var(--yt-code-bg);
  font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace;
  white-space: pre-wrap;
  word-break: break-all;
}
.jk-key { color: var(--yt-jk-key); }
.jk-str { color: var(--yt-jk-str); }
.jk-num { color: var(--yt-jk-num); }
.jk-bool { color: var(--yt-jk-bool); }
.jk-null { color: var(--yt-jk-null); }
.jk-punc { color: var(--yt-jk-punc); }
</style>
