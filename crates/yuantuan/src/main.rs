//! 云团装配入口：配置 → data 目录 → SQLite 迁移 → Event Bus / tracer → adapter-qq → WebUI。

mod config;
mod db;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    // G007 面板重启支持：新进程 spawn 时若带此环境变量则先 sleep（给旧进程释放端口的时间窗）
    if let Ok(ms) = std::env::var("YUANTUAN_DELAY_START_MS") {
        if let Ok(n) = ms.parse::<u64>() {
            if n > 0 {
                // 还没 init tracing（避免 banner 之前就输出），用 eprintln
                eprintln!("[yuantuan] 延迟启动 {n}ms（面板重启握手）");
                tokio::time::sleep(std::time::Duration::from_millis(n)).await;
            }
        }
    }
    tracing_subscriber::fmt().with_target(false).init();
    info!("云团骨架启动");

    // a. 配置：不存在则生成默认模板并继续使用默认值
    let cfg = config::load_or_default("config.toml")?;
    info!(
        data_dir = %cfg.data.dir,
        napcat_listen = %cfg.napcat.listen_addr,
        napcat_enabled = cfg.napcat.enabled,
        log_level = %cfg.log.level,
        "配置加载完成"
    );

    // b. data/ 目录结构
    let data_root = db::init_data_dirs(&cfg.data.dir)?;
    info!(path = %data_root.display(), "data/ 目录结构就绪（memes/ artifacts/ archive/ logs/ backups/）");

    // b'. 启动时检测恢复标记：用户在 WebUI 点了「一键恢复」+ 手动重启后，这里把 tar 解压覆盖 data/
    //     必须在 db::open 之前——恢复期间 db 文件可能被替换
    if let Err(e) = yuantuan_core::backup::restore_from_pending(&data_root).await {
        tracing::warn!(error = %e, "恢复标记处理失败（继续启动）");
    }

    // c. SQLite（WAL）+ 迁移
    let mut conn = db::open(&cfg.data.dir)?;
    info!(db = %data_root.join("yuantuan.db").display(), "SQLite 已打开（WAL）");
    yuantuan_core::db::migrate(&mut conn)?;

    let tables = yuantuan_core::db::list_tables(&conn)?;
    info!(count = tables.len(), "迁移完成，库内表清单");
    let has_admin: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM state_kv WHERE key = 'admin_pass_hash'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if has_admin {
        info!("管理员密码已配置");
    } else {
        info!("管理员密码未配置：首启引导中（WebUI 首次登录将设定管理员密码）");
    }
    drop(conn);

    let db_path = data_root.join("yuantuan.db");

    // c2. meme 入库扫描（md5/dHash 去重）
    let memes_dir = data_root.join("memes");
    match yuantuan_core::meme::scan_and_ingest(&memes_dir, &db_path) {
        Ok(s) => info!(scanned = s.scanned, added = s.added, skipped_dup = s.skipped_dup, "meme 库扫描完成"),
        Err(e) => tracing::warn!(error = %e, "meme 库扫描失败（不阻断启动）"),
    }

    // d. Event Bus + tracer（全事件落 events 表）；supervisor 接管：panic 自动拉起
    let bus = yuantuan_core::event::EventBus::default();
    let supervisor = yuantuan_core::supervisor::Supervisor::new();
    {
        let bus2 = bus.clone();
        let db2 = db_path.clone();
        supervisor.spawn("tracer", move || yuantuan_core::event::spawn_tracer(&bus2, db2.clone()));
    }

    // e. LLM Provider（providers.toml 缺失则生成模板；角色未配置则管线降级 ignore，不崩）
    let llm = load_llm_gateway();

    // f. adapter-qq（[napcat].enabled=false 则跳过；token 走共享槽让 webui 热应用立刻生效）
    let self_ids = yuantuan_core::prefilter::SelfMsgIds::with_cap(cfg.pipeline.self_msg_ids_cap);
    let napcat_token_slot = yuantuan_adapter_qq::shared_token(&cfg.napcat.token);
    let adapter = if cfg.napcat.enabled {
        info!(listen_addr = %cfg.napcat.listen_addr, "adapter-qq 启动（反向 WS，等待 NapCat Websockets客户端连入）");
        Some(yuantuan_adapter_qq::spawn(
            bus.clone(),
            db_path.clone(),
            yuantuan_adapter_qq::NapcatConfig {
                listen_addr: cfg.napcat.listen_addr.clone(),
                token: napcat_token_slot.clone(),
            },
            self_ids.clone(),
        ))
    } else {
        info!("[napcat].enabled=false，跳过 adapter-qq");
        None
    };

    // g. 回复形态引擎（per-chat 发送队列；adapter 未接入时发送函数恒报错、reply 记事件跳过）
    //    mood 单实例：Decision 写回、引擎 ::meme 映射共读；参数走热应用槽
    let mood = yuantuan_core::state::MoodState::default();
    let reply_slot: yuantuan_core::reply_engine::SharedReplyCfg =
        std::sync::Arc::new(std::sync::RwLock::new(yuantuan_core::reply_engine::ReplyCfg {
            first_delay_min_ms: cfg.reply.first_delay_min_ms,
            first_delay_max_ms: cfg.reply.first_delay_max_ms,
            base_delay_ms: cfg.reply.base_delay_ms,
            per_char_ms: cfg.reply.per_char_ms,
            jitter_ratio: cfg.reply.jitter_ratio,
            min_delay_ms: cfg.reply.min_delay_ms,
            max_delay_ms: cfg.reply.max_delay_ms,
            total_budget_ms: cfg.reply.total_budget_ms,
            bubble_cap: cfg.reply.bubble_cap,
            bubble_char_cap: cfg.reply.bubble_char_cap,
        }));
    let ctx_slot: yuantuan_core::context_builder::SharedContextCfg =
        std::sync::Arc::new(std::sync::RwLock::new(yuantuan_core::context_builder::ContextCfg {
            budget_chars: cfg.context.budget_chars,
            k_init: cfg.context.k,
            roster_mem_per: cfg.context.roster_mem_per,
        }));
    let send = adapter
        .as_ref()
        .map(|h| yuantuan_adapter_qq::send_fn(h.clone()))
        .unwrap_or_else(|| {
            std::sync::Arc::new(|_req| {
                Box::pin(async move {
                    anyhow::bail!("adapter 未接入（[napcat].enabled=false）")
                })
                    as std::pin::Pin<
                        Box<
                            dyn std::future::Future<Output = anyhow::Result<serde_json::Value>>
                                + Send,
                        >,
                    >
            })
        });
    let reply_engine = yuantuan_core::reply_engine::ReplyEngine::new(
        db_path.clone(),
        bus.clone(),
        send,
        self_ids.clone(),
        reply_slot.clone(),
        mood.clone(),
    )
    .spawn();

    // h1'. 插件层装配（先放在 PipelineDeps 之前：PipelineDeps.skill_registry 需要借用注册表）
    //     Q-P01 编译期加载 Rust crate；Q-P04 面板启禁走 enabled 标记文件
    //     当前加载：plugins/<name>/enabled 存在则其 register() / skills() 被调用，
    //     返回的 Tool / Skill 分别进 Registry / SkillRegistry
    let tools_registry = yuantuan_core::tools::Registry::new();
    if cfg.sandbox.enabled {
        let sandbox = yuantuan_core::tools::sandbox::SandboxExecTool::new(cfg.sandbox.clone())?;
        sandbox.check_available().await?;
        tools_registry.register(sandbox);
        info!(image = %cfg.sandbox.image, "Docker 开发沙箱已就绪");
    }
    let skill_registry = yuantuan_core::skills::SkillRegistry::new();
    let enabled_plugins = load_enabled_plugins(&tools_registry, &skill_registry);
    if !enabled_plugins.is_empty() {
        info!(
            plugins = ?enabled_plugins,
            skills = ?skill_registry.names(),
            "插件层装配完成"
        );
    }

    // h1''. MCP stdio client（Q-M01 裁决：spawn 外部 MCP server，tools 注入 ToolRegistry）
    //        未配置 = 跳过；任一 server 失败 = warn + 继续（不阻塞其他）
    let (mcp_manager, mcp_failures) = yuantuan_core::mcp::McpManager::spawn_all(&cfg.mcp).await;
    if !cfg.mcp.servers.is_empty() {
        info!(
            total = cfg.mcp.servers.len(),
            ok = mcp_manager.clients.len(),
            failed = mcp_failures.len(),
            "MCP 客户端装配完成"
        );
        for (name, err) in &mcp_failures {
            warn!(server = %name, error = %err, "MCP server 启动失败");
        }
    }
    mcp_manager.register_tools(&tools_registry);
    let mcp_manager = std::sync::Arc::new(mcp_manager);

    // h. Decision 管线（订阅 MessageReceived → Prefilter → 成本闸 → Decision → 副作用 → reply/send_meme）
    //    llm/prefilter/reply/context 走共享槽：WebUI config 写回即热应用
    let llm_slot: yuantuan_core::bot::SharedLlm = std::sync::Arc::new(std::sync::RwLock::new(llm.clone()));
    let prefilter_slot: yuantuan_core::bot::SharedPrefilter = std::sync::Arc::new(std::sync::RwLock::new(
        yuantuan_core::prefilter::Config {
            window_secs: cfg.prefilter.window_secs,
            self_msg_cap: cfg.prefilter.self_msg_cap,
            decision_cost_per_min: cfg.prefilter.decision_cost_per_min,
        },
    ));
    // G008 调优参数入槽
    let per_chat_cap_slot: yuantuan_core::bot::SharedPerChatCap =
        std::sync::Arc::new(std::sync::RwLock::new(cfg.pipeline.per_chat_queue_cap));
    {
        let init_cpm = cfg.pipeline.decision_cost_per_min_init.max(1);
        if let Some(gw) = llm_slot.read().unwrap().as_ref() {
            gw.set_cost_per_min(init_cpm);
        }
        // self_ids 已在 line 74 with_cap 构造完毕，这里不再重复 set_cap
    }
    let self_qq = adapter
        .as_ref()
        .map(|h| h.self_qq_shared())
        .unwrap_or_default();
    let pipeline_deps = yuantuan_core::bot::PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: llm_slot.clone(),
        self_qq: self_qq.clone(),
        self_ids: self_ids.clone(),
        mood: mood.clone(),
        prefilter: prefilter_slot.clone(),
        reply: Some(reply_engine.clone()),
        reply_cfg: reply_slot.clone(),
        ctx_cfg: ctx_slot.clone(),
        memes_dir: memes_dir.clone(),
        media_ctx: Some(yuantuan_core::tools::media::command::MediaCtx {
            db_path: db_path.clone(),
            data_dir: data_root.clone(),
            bus: bus.clone(),
            reply_engine: Some(reply_engine.clone()),
            registry: load_media_registry(&db_path),
            // Q009 admin 列表：从 config.toml [media].admin_qq 读（QQ 号 → "p_<qq>" 形式比对）
            self_pid_admin: {
                let admins = cfg.media.admin_qq.clone();
                std::sync::Arc::new(move |pid: &str| {
                    let qq = pid.trim_start_matches("p_").parse::<u64>().unwrap_or(0);
                    admins.contains(&qq)
                })
            },
            // Q010 提示词优化使用的 LLM gateway（当前直通；接通后按 optimizer_role 调 chat）
            llm: llm_slot.clone(),
        }),
        skill_registry: Some(skill_registry.clone()),
        per_chat_cap: per_chat_cap_slot.clone(),
    };

    // h''. Q55 恢复消费：把上次进程退出前来不及处理的 messages 回放进管线
    //      （跳过 /image 直派命令；自身消息 ingest 时已标记不扫入）
    match yuantuan_core::bot::replay_pending(&pipeline_deps).await {
        Ok(n) if n > 0 => info!(count = n, "Q55 重启回放完成"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "Q55 回放扫描失败（不阻断启动）"),
    }
    {
        let deps = pipeline_deps.clone();
        supervisor.spawn("pipeline", move || yuantuan_core::bot::spawn_pipeline(deps.clone()));
    }

    // h'''. Task runner（Q-A01 同步工具循环）：订阅 TaskCreated → 每任务一个执行协程
    //       工具范围 = ToolRegistry 全部已注册工具（Q-A02）；静默不发群（Q-A03）
    {
        let deps = yuantuan_core::agent::TaskRunnerDeps {
            db_path: db_path.clone(),
            llm: llm_slot.clone(),
            tools: tools_registry.clone(),
            bus: bus.clone(),
        };
        supervisor.spawn("task_runner", move || {
            yuantuan_core::agent::spawn_runner(deps.clone())
        });
    }

    // 装配完 LlmGateway 后让插件的 Skill 能调 BotChat LLM
    // （Skill trait 不依赖 LLM；插件用 OnceLock 槽在装配时被注入；未注入时退回模板渲染）
    {
        let gw_opt = llm_slot.read().unwrap().clone();
        if let Some(gw) = gw_opt {
            // 目前仅 hello 插件；未来扩为多插件时改成各自命名空间注入
            let _ = yuantuan_plugin_hello::set_llm(gw);
        }
    }

    // h2. 偷表情包监听（开关走热应用槽，进程内常驻）
    let steal_slot: yuantuan_core::meme::SharedSteal =
        std::sync::Arc::new(std::sync::RwLock::new(cfg.meme.steal_enabled));
    {
        let bus2 = bus.clone();
        let db2 = db_path.clone();
        let memes2 = memes_dir.clone();
        let slot = steal_slot.clone();
        supervisor.spawn("steal_listener", move || {
            yuantuan_core::meme::spawn_steal_listener(
                &bus2,
                db2.clone(),
                memes2.clone(),
                slot.clone(),
            )
        });
    }

    // i. 夜间归纳调度器（句柄注册：WebUI 改 [consolidation] 后取消旧定时器按新配置重建）
    let consolidation_handle: std::sync::Arc<
        std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(None));
    if cfg.consolidation.enabled {
        let h = yuantuan_core::consolidation::spawn_scheduler(
            yuantuan_core::consolidation::ConsolidationDeps {
                db_path: db_path.clone(),
                llm: llm.clone(),
                bus: bus.clone(),
                self_qq: self_qq.clone(),
                cfg: yuantuan_core::consolidation::ConsolidationCfg {
                    enabled: cfg.consolidation.enabled,
                    daily_time: cfg.consolidation.daily_time.clone(),
                    run_on_startup: cfg.consolidation.run_on_startup,
                },
            },
        );
        *consolidation_handle.lock().unwrap() = Some(h);
    } else {
        info!("[consolidation].enabled=false，跳过夜间归纳调度器");
    }

    // i'. 备份调度器（每日定时 + 热应用槽）。启用与否由槽内 cfg.enabled 决定（运行时可热切换）
    let backup_cfg_slot: yuantuan_core::backup::SharedBackupCfg =
        yuantuan_core::backup::shared_backup_cfg();
    {
        *backup_cfg_slot.write().unwrap() = cfg.backup.clone();
    }
    let _backup_scheduler = yuantuan_core::backup::spawn_daily_scheduler(
        backup_cfg_slot.clone(),
        data_root.clone(),
    );

    // f. WebUI（阻塞至进程结束）
    info!("云团骨架启动成功");
    let panel_url = format!("http://{}:{}/", cfg.webui.host, cfg.webui.port);
    info!("");
    info!("  ┌─ 管理面板 ─────────────────────────────────");
    info!("  │  {}", panel_url);
    info!("  └────────────────────────────────────────────");
    if cfg.napcat.enabled {
        info!("  NapCat 反向 WS 监听 ws://{}/ws", cfg.napcat.listen_addr);
    }
    info!("  首次登录密码 = 管理员密码（首启引导）");
    info!("");

    // 自动打开面板：仅 Windows 有效（你日常开发机）。
    // Linux 服务器上通常没 GUI、且访问是从本地浏览器跨网到来，启动时根本不该 open
    #[cfg(target_os = "windows")]
    {
        let url = panel_url.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn();
        });
        info!("  已自动打开浏览器到面板");
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 服务器：banner 里给 ssh 端口转发提示，用户自己挑访问方式
        if cfg.webui.host == "127.0.0.1" {
            info!("  远程访问：ssh -L {}:127.0.0.1:{} user@本机，浏览器开 http://127.0.0.1:{}/",
                cfg.webui.port, cfg.webui.port, cfg.webui.port);
        }
    }
    let extras = yuantuan_webui::Extras {
        bus: bus.clone(),
        llm_slot,
        prefilter_slot,
        reply_slot,
        ctx_slot,
        steal_slot,
        consolidation: consolidation_handle,
        self_qq: self_qq.clone(),
        mood: mood.clone(),
        adapter_connected: adapter
            .as_ref()
            .map(|h| {
                let h = h.clone();
                std::sync::Arc::new(move || h.is_connected())
                    as std::sync::Arc<dyn Fn() -> bool + Send + Sync>
            })
            .unwrap_or_else(|| std::sync::Arc::new(|| false)),
        napcat_token_slot: if cfg.napcat.enabled {
            Some(napcat_token_slot.clone())
        } else {
            None
        },
        backup_cfg: backup_cfg_slot.clone(),
        tools_registry: tools_registry.clone(),
        skill_registry: skill_registry.clone(),
        mcp_manager: Some(mcp_manager.clone()),
        per_chat_cap: per_chat_cap_slot.clone(),
        self_ids: self_ids.clone(),
        config_path: std::path::PathBuf::from("config.toml"),
        providers_path: std::path::PathBuf::from("providers.toml"),
    };
    let webui_serve = yuantuan_webui::serve(db_path.clone(), &cfg.webui.host, cfg.webui.port, extras);

    // MOD-B11 优雅停机(runtime-design 三章「SIGTERM → 停止接收 → 排空 → checkpoint → 退出」)
    // Windows 无 SIGTERM,用 Ctrl+C(tokio::signal::ctrl_c 跨平台):
    //   ① 停 adapter(不再接受新消息) → ② 等 5s 让在飞消息发完 → ③ WAL checkpoint → ④ exit(0)
    tokio::select! {
        res = webui_serve => {
            info!(result = ?res, "WebUI 退出(服务自然结束)");
            res
        }
        _ = tokio::signal::ctrl_c() => {
            info!("收到 Ctrl+C,启动优雅停机序列");
            // ① adapter: drop handle 触发其内部关闭(若实现);这里只记日志——真正停止接消息靠进程退出
            info!("① 停止接收消息(adapter 将随进程退出)");
            // ② 等 5s 让回复引擎把在飞泡发完(目前没有暴露 drain API,用固定窗口)
            info!("② 等待 5s 发送队列排空");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            // ③ WAL checkpoint:把 -wal 合并回主 db 文件,避免下次启动恢复慢
            info!("③ SQLite WAL checkpoint");
            match yuantuan_core::db::connect(&db_path) {
                Ok(conn) => {
                    match conn.pragma_update(None, "wal_checkpoint", "TRUNCATE") {
                        Ok(_) => info!("WAL checkpoint 完成"),
                        Err(e) => warn!(error = %e, "WAL checkpoint 失败(不影响退出)"),
                    }
                }
                Err(e) => warn!(error = %e, "打开 db 做 checkpoint 失败"),
            }
            info!("④ 优雅停机完成,退出");
            Ok(())
        }
    }
}

/// 扫描 plugins/<name>/enabled 标记：启用的插件调用其 register() 把 Tool 注册进 Registry
/// 当前编译期决定（crate 是否被 link 进 yuantuan）；enabled 文件只控制运行时是否注册
/// 未来热加插件需要 build.rs 监听这个目录做条件 include——本期先做编译期注册
fn load_enabled_plugins(
    registry: &yuantuan_core::tools::Registry,
    skill_registry: &yuantuan_core::skills::SkillRegistry,
) -> Vec<String> {
    let plugins_dir = std::path::Path::new("plugins");
    if !plugins_dir.exists() {
        return vec![];
    }
    let mut out = Vec::new();
    // 已知编译进来的插件：构建期通过 Cargo feature 决定是否链接；enabled 文件只是运行时开关
    #[cfg(feature = "plugin-hello")]
    {
        let enabled_marker = plugins_dir.join("hello").join("enabled");
        if enabled_marker.exists() {
            for tool in yuantuan_plugin_hello::register() {
                registry.register_arc(tool);
            }
            for skill in yuantuan_plugin_hello::skills() {
                skill_registry.register_arc(skill);
            }
            out.push("hello".to_string());
            info!(
                skills = ?skill_registry.names(),
                "插件 hello 已启用并注册"
            );
        } else {
            info!("插件 hello 存在但未启用（缺 plugins/hello/enabled 标记）");
        }
    }
    out
}

/// 从 media_providers 表装配 MediaProvider registry：
/// - 按 endpoint 形态实例化对应适配器（当前仅 NAI；openai_compat/gemini/xai 后续按需扩展）
/// - api_key_env 读环境变量；缺失则跳过该 provider 并告警
/// - 失败行不阻断启动，只 warn
fn load_media_registry(db_path: &std::path::Path) -> Arc<RwLock<HashMap<String, Arc<dyn yuantuan_core::tools::media::provider::MediaProvider>>>> {
    use yuantuan_core::tools::media::provider::{EndpointStyle, MediaProvider, ProviderCfg};
    use yuantuan_core::tools::media::provider::nai::NaiProvider;

    let registry: HashMap<String, Arc<dyn MediaProvider>> = HashMap::new();
    let registry = Arc::new(RwLock::new(registry));

    let conn = match yuantuan_core::db::connect(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "media_providers 装配：打开 db 失败，registry 为空");
            return registry;
        }
    };
    let mut stmt = match conn.prepare(
        "SELECT name, base_url, api_key_env, default_endpoint FROM media_providers WHERE enabled = 1"
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "media_providers 装配：prepare 失败，registry 为空");
            return registry;
        }
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    });
    let rows: Vec<_> = match rows {
        Ok(rs) => rs.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "media_providers 装配：query 失败，registry 为空");
            return registry;
        }
    };

    let mut reg = registry.write().expect("media registry poisoned");
    for (name, base_url, api_key_env, default_endpoint) in rows {
        let api_key = if api_key_env.is_empty() {
            None
        } else {
            match std::env::var(&api_key_env) {
                Ok(k) if !k.is_empty() => Some(k),
                _ => {
                    tracing::warn!(provider = %name, env = %api_key_env, "media provider 的 API key 环境变量缺失，跳过");
                    continue;
                }
            }
        };
        let endpoint = match default_endpoint.as_str() {
            "nai_native" => EndpointStyle::NaiNative,
            "openai_compat" => EndpointStyle::OpenaiCompat,
            "gemini" => EndpointStyle::Gemini,
            "xai" => EndpointStyle::Xai,
            _ => EndpointStyle::NaiNative,
        };
        let cfg = ProviderCfg {
            name: name.clone(),
            base_url,
            api_key,
            default_endpoint: endpoint,
        };
        let provider: Arc<dyn MediaProvider> = match endpoint {
            EndpointStyle::NaiNative | EndpointStyle::OpenaiCompat => Arc::new(NaiProvider::new(cfg)),
            EndpointStyle::Gemini | EndpointStyle::Xai => {
                tracing::warn!(provider = %name, endpoint = ?endpoint, "暂不支持该 endpoint，跳过");
                continue;
            }
        };
        info!(provider = %name, endpoint = ?endpoint, "media provider 装配完成");
        reg.insert(name, provider);
    }
    drop(reg);
    registry
}

/// providers.toml：缺失则生成模板；存在则解析（失败仅告警，角色全部不可用、管线降级）
fn load_llm_gateway() -> Option<std::sync::Arc<yuantuan_core::llm::LlmGateway>> {
    let path = std::path::Path::new("providers.toml");
    if !path.exists() {
        if let Err(e) = std::fs::write(path, yuantuan_core::llm::DEFAULT_PROVIDERS_TEMPLATE) {
            tracing::warn!(error = %e, "写入 providers.toml 模板失败");
        } else {
            info!("providers.toml 不存在，已生成默认模板（配置后 LLM 角色生效）");
        }
        return None;
    }
    match yuantuan_core::llm::LlmGateway::load(path) {
        Ok(mut g) => {
            // token 用量落库：llm_usage 表（仪表盘"今日 token"卡）
            let db = std::path::PathBuf::from("data/yuantuan.db");
            g.set_usage_sink(std::sync::Arc::new(move |rec: yuantuan_core::llm::LlmUsageRecord| {
                let db = db.clone();
                tokio::spawn(async move {
                    let res = tokio::task::spawn_blocking(move || {
                        let conn = yuantuan_core::db::connect(&db).ok()?;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        conn.execute(
                            "INSERT INTO llm_usage(ts, role, model, prompt_tokens, completion_tokens, total_tokens)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            rusqlite::params![
                                now,
                                rec.role.to_string(),
                                rec.model,
                                rec.usage.prompt_tokens as i64,
                                rec.usage.completion_tokens as i64,
                                rec.usage.total_tokens as i64,
                            ],
                        ).ok()
                    }).await;
                    if let Err(e) = res {
                        tracing::debug!(error = %e, "llm_usage 落库任务失败");
                    }
                });
            }));
            Some(std::sync::Arc::new(g))
        }
        Err(e) => {
            tracing::warn!(error = %e, "providers.toml 解析失败，LLM 角色全部不可用");
            None
        }
    }
}
