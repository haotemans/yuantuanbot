<template>
  <div class="json-wrap">
    <button type="button" class="json-copy" :class="{ ok: copied }" @click="copy">
      {{ copied ? '已复制' : '复制' }}
    </button>
    <pre class="json-view"><code><span v-for="(t, i) in tokens" :key="i" :class="t.cls">{{ t.text }}</span></code></pre>
  </div>
</template>

<script setup>
import { computed, ref } from 'vue'

const props = defineProps({
  data: { default: null },
})

const copied = ref(false)
let copyTimer = null
async function copy() {
  const text = JSON.stringify(props.data, null, 2) ?? ''
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    // 剪贴板权限被拒时降级：选中态提示交给按钮文案
  }
  copied.value = true
  clearTimeout(copyTimer)
  copyTimer = setTimeout(() => { copied.value = false }, 1600)
}

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
.json-wrap {
  position: relative;
}
.json-copy {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 1;
  padding: 3px 10px;
  font-size: 11.5px;
  border-radius: 7px;
  border: 1px solid var(--yt-card-border);
  background: var(--yt-header-bg);
  backdrop-filter: blur(6px);
  color: var(--yt-text-dim);
  cursor: pointer;
  transition: color 0.14s ease, border-color 0.14s ease;
}
.json-copy:hover {
  color: var(--yt-primary);
  border-color: var(--yt-primary);
}
.json-copy.ok {
  color: var(--yt-ok);
  border-color: var(--yt-ok);
}
.json-view {
  margin: 0;
  padding: 12px 14px;
  font-size: 12px;
  line-height: 1.65;
  overflow: auto;
  max-height: calc(100vh - 260px);
  border-radius: 10px;
  border: 1px solid var(--yt-card-border);
  background: var(--yt-code-bg);
  font-family: var(--yt-code-font);
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
