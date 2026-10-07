# WebUI 主题 / 字体统一 Workbench

## 状态
- status: closed（卡片方阵 + token 收拢 + modal 详情 全 verified）
- owner: hsb + kimi
- last-grill: 2026-10-07
- frontier: 无

## 已确认裁决

### Q-T01 统一范围
- decision: 只收拢 token，不换色调
- rationale: 现有 indigo 主色 + cyan info + 亮暗双套 CSS 变量已成型；问题只是 4 处 .vue 局部手写 mono 栈没用 token，以及 chip 色调没 token
- status: confirmed

### Q-T02 汉字字体
- decision: 保持系统栈（-apple-system, Segoe UI, PingFang SC, Microsoft YaHei）
- rationale: 零请求，中文靠系统；Inter/Noto 都太重，自用面板没必要
- status: confirmed

### Q-T03 等宽字体
- decision: 不包 JetBrains Mono 的 woff2；保持现有 --yt-mono token 定义（JetBrains Mono → ui-monospace → SF Mono → Menlo → Consolas → 系统 mono fallback）
- rationale: bundle 零增加；Windows 的 Consolas 已经很好；
- status: confirmed

### Q-T04 Skill/Tool chip progenitor 语义色
- decision: 新增 --yt-violet 系（skill chip 用）与 --yt-cyan 系（工具调用 / info chip 用）两个语义 token；不强行用 5 个语义色（primary/success/warning/error/info）硬套
- rationale: skill ≠ info ≠ primary；tool ≠ success；各身份一种色调，用户一眼能分清能力类别
- status: confirmed

### Q-T05 卡片形态
- decision: 正方形圆角（aspect-ratio 1/1, border-radius 16px），可点打开详情
- rationale: 用户原话"一个一个卡片排列，卡片面写功能和开关和打开配置"
- status: confirmed

### Q-T06 详情呈现
- decision: 点击展开 = 弹 n-modal；新增/编辑 MCP 也走 modal
- rationale: 卡片面只呈现概览；详情和编辑不压缩列表
- status: confirmed

## 工程任务

- [x] MOD-T01 styles.css :root 与 dark 各新增 `--yt-violet{,-soft,-border,-ink}` + `--yt-cyan{,-soft,-border,-ink}` + `--yt-ok{,-soft,-ring,-pulse}` token
- [x] MOD-T02 Plugins.vue 的 .chip.skill / .chip.tool 删局部硬写，改读全局 styles.css 的 .chip 基类（已定义 violet/cyan 变体）
- [x] MOD-T03 Mcp.vue 的 .chip 同样上收；MCP tool chip 用 --yt-cyan 系（与 Plugins 的 tool 一致）
- [x] MOD-T04 Plugins/Mcp/Platform/Personality 内联 font-family 硬写全部删除，走全局 .mono / code 选择器（var(--yt-mono) 栈 = JetBrains Mono → ui-monospace → SF Mono → Menlo → Consolas → monospace）
- [x] MOD-T05 Plugins.vue 的 .chip / .mono 局部样式块删除，全局 styles.css 一份
- [x] MOD-T06 build 成功 + 浏览器亮色模式验证过：violet skill chip + cyan tool chip 渲染正常

## 代码反馈

- **`--yt-card-bg` 不存在**：初版 chip 写了 `background: var(--yt-card-bg)`，token 未定义导致 background 透明。改为 `background: var(--yt-header-bg)` 匹配现有约定；亮点：var() 缺省值没生效这件事 vite 不会 warn，审查 token 名要靠 grep
- **ui.dark 状态持久化到 localStorage(`yt_theme`)**：用户上次切到 dark 后这次刷新仍是 dark，但 naive 的 theme 变成 darkTheme 时如果 body 的 background 仍被 :root 的 --yt-bg override，会出现"dark token + light background"反白。已建议用户把面板 toggle 亮 / 暗 toggle 到 light；本次主题是 token 收拢没动这块。后续若要修，应为 body 底色写 var + 在 html.dark 时覆写 --yt-bg

## 状态收尾

- status: closed（决策全落地，build 过 + 亮色 mode 视觉确认 OK）
- 收口时间： 2026-10-07
