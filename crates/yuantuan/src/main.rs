//! 云团装配入口：配置 → data 目录 → SQLite 迁移 → Event Bus / tracer → adapter-qq → WebUI。

mod config;
mod db;

use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    info!("云团骨架启动");

    // a. 配置：不存在则生成默认模板并继续使用默认值
    let cfg = config::load_or_default("config.toml")?;
    info!(
        data_dir = %cfg.data.dir,
        napcat_ws = %cfg.napcat.ws_url,
        napcat_enabled = cfg.napcat.enabled,
        log_level = %cfg.log.level,
        "配置加载完成"
    );

    // b. data/ 目录结构
    let data_root = db::init_data_dirs(&cfg.data.dir)?;
    info!(path = %data_root.display(), "data/ 目录结构就绪（memes/ artifacts/ archive/ logs/ backups/）");

    // c. SQLite（WAL）+ 迁移
    let mut conn = db::open(&cfg.data.dir)?;
    info!(db = %data_root.join("yuantuan.db").display(), "SQLite 已打开（WAL）");
    yuantuan_core::db::migrate(&mut conn)?;

    let tables = yuantuan_core::db::list_tables(&conn)?;
    info!(count = tables.len(), "迁移完成，库内表清单");
    drop(conn);

    let db_path = data_root.join("yuantuan.db");

    // c2. meme 入库扫描（md5/dHash 去重）
    let memes_dir = data_root.join("memes");
    match yuantuan_core::meme::scan_and_ingest(&memes_dir, &db_path) {
        Ok(s) => info!(scanned = s.scanned, added = s.added, skipped_dup = s.skipped_dup, "meme 库扫描完成"),
        Err(e) => tracing::warn!(error = %e, "meme 库扫描失败（不阻断启动）"),
    }

    // d. Event Bus + tracer（全事件落 events 表）
    let bus = yuantuan_core::event::EventBus::default();
    let _tracer = yuantuan_core::event::spawn_tracer(&bus, db_path.clone());

    // e. LLM Provider（providers.toml 缺失则生成模板；角色未配置则管线降级 ignore，不崩）
    let llm = load_llm_gateway();

    // f. adapter-qq（[napcat].enabled=false 则跳过；连不上退避重连不死进程）
    let self_ids = yuantuan_core::prefilter::SelfMsgIds::default();
    let adapter = if cfg.napcat.enabled {
        info!(ws_url = %cfg.napcat.ws_url, "adapter-qq 启动（正向 WS）");
        Some(yuantuan_adapter_qq::spawn(
            bus.clone(),
            db_path.clone(),
            yuantuan_adapter_qq::NapcatConfig {
                ws_url: cfg.napcat.ws_url.clone(),
                token: cfg.napcat.token.clone(),
            },
            self_ids.clone(),
        ))
    } else {
        info!("[napcat].enabled=false，跳过 adapter-qq");
        None
    };

    // g. 回复形态引擎（per-chat 发送队列；adapter 未接入时发送函数恒报错、reply 记事件跳过）
    //    mood 单实例：Decision 写回、引擎 ::meme 映射共读
    let mood = yuantuan_core::state::MoodState::default();
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
        yuantuan_core::reply_engine::ReplyCfg::default(),
        mood.clone(),
    )
    .spawn();

    // h. Decision 管线（订阅 MessageReceived → Prefilter → 成本闸 → Decision → 副作用 → reply/send_meme）
    let self_qq = adapter
        .as_ref()
        .map(|h| h.self_qq_shared())
        .unwrap_or_default();
    let _pipeline = yuantuan_core::bot::spawn_pipeline(yuantuan_core::bot::PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: llm.clone(),
        self_qq: self_qq.clone(),
        self_ids,
        mood,
        prefilter: yuantuan_core::prefilter::Config {
            window_secs: cfg.prefilter.window_secs,
            self_msg_cap: cfg.prefilter.self_msg_cap,
        },
        reply: Some(reply_engine),
        memes_dir: memes_dir.clone(),
    });

    // h2. 偷表情包监听（[meme].steal_enabled）
    if cfg.meme.steal_enabled {
        let _steal = yuantuan_core::meme::spawn_steal_listener(&bus, db_path.clone(), memes_dir.clone());
    } else {
        info!("[meme].steal_enabled=false，偷表情包关闭");
    }

    // i. 夜间归纳调度器（单实例锁；enabled=false 则跳过）
    if cfg.consolidation.enabled {
        let _consolidation = yuantuan_core::consolidation::spawn_scheduler(
            yuantuan_core::consolidation::ConsolidationDeps {
                db_path: db_path.clone(),
                llm: llm.clone(),
                bus: bus.clone(),
                self_qq,
                cfg: yuantuan_core::consolidation::ConsolidationCfg {
                    enabled: cfg.consolidation.enabled,
                    daily_time: cfg.consolidation.daily_time.clone(),
                    run_on_startup: cfg.consolidation.run_on_startup,
                },
            },
        );
    } else {
        info!("[consolidation].enabled=false，跳过夜间归纳调度器");
    }

    // f. WebUI（阻塞至进程结束）
    info!("云团骨架启动成功");
    info!(
        url = %format!("http://{}:{}/", cfg.webui.host, cfg.webui.port),
        "管理员面板；首次登录提交的密码即为管理员密码（首启引导）"
    );
    yuantuan_webui::serve(db_path, &cfg.webui.host, cfg.webui.port).await
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
        Ok(g) => Some(std::sync::Arc::new(g)),
        Err(e) => {
            tracing::warn!(error = %e, "providers.toml 解析失败，LLM 角色全部不可用");
            None
        }
    }
}
