# 仪表盘资源 & Token 指标 Workbench

## 状态
- status: closed
- owner: hsb + kimi
- last-grill: 2026-10-07

## 已确认裁决

### Q-D01 Token 来源
- decision: LlmGateway.chat() 解析响应 usage.{prompt,completion,total}_tokens，通过注入的 UsageSink 回调落 llm_usage 表
- rationale: 准确；OpenAI 兼容端点普遍返回 usage。llm 网关不依赖 db（core 纯净性），sink 由装配侧（main / config_api 热重建）注入
- affected-code: crates/core/src/llm.rs, crates/yuantuan/src/main.rs, crates/webui/src/config_api.rs
- status: confirmed

### Q-D02 系统指标
- decision: sysinfo crate（跨平台 Windows/Linux），静态共享 System 实例做 CPU% 增量采样
- rationale: 跨平台零手写；首次调用 CPU 返回 0 属正常（无基准）
- affected-code: crates/webui/Cargo.toml, crates/webui/src/dashboard.rs
- status: confirmed

### Q-D03 呈现
- decision: 6 卡（今日收/发/Decision/活跃任务/今日 token/进程内存）+ CPU 进头部 tag（>=50% 黄 >=80% 红）
- status: confirmed

## 工程任务

- [x] MOD-D01 db.rs V3 迁移：llm_usage 表（ts/role/model/prompt/completion/total + 2 索引）
- [x] MOD-D02 llm.rs：LlmUsage/LlmUsageRecord/UsageSink 类型 + set_usage_sink + chat() 成功路径解析 usage 回调
- [x] MOD-D03 main.rs load_llm_gateway 注入 sink（tokio::spawn + spawn_blocking 写库，不阻塞 chat 主链路）
- [x] MOD-D04 config_api.rs providers 热重建也注入 sink（否则换槽后统计断流）
- [x] MOD-D05 dashboard.rs：tokens_today/tokens_in/tokens_out/llm_calls + mem_rss/mem_total/cpu_percent
- [x] MOD-D06 Dashboard.vue：6 卡 grid（xl:6）+ stat-sub hint 行 + CPU 头 tag + 2 个新 trend 序列
- [x] MOD-D07 styles.css：.stat-sub 样式

## 验证

- cargo test --workspace 全过（47 单测 + 各 crate 集成）
- /api/dashboard 返回全部新字段（mem_rss=23MB, cpu=0.004% 首次）
- 浏览器亮色模式渲染 OK：6 卡 + CPU tag + hint 行

## 代码反馈

- sysinfo CPU% 需要两次采样 delta，单次新建 System 永远返 0 → 静态 Mutex<Option<System>> 共享
- sink 是 Arc<dyn Fn>，回调里不能 block（chat 是 async 链路）→ tokio::spawn + spawn_blocking
- UsageSink 未覆盖 test_chat / list_models 等非 chat 路径（不需要——那些不算业务 token）
- provider 不返回 usage 时静默跳过（total_tokens=0 不落库）—— mock / 本地模型场景不污染表
