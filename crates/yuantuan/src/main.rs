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

    // d. Event Bus + tracer（全事件落 events 表）
    let bus = yuantuan_core::event::EventBus::default();
    let db_path = data_root.join("yuantuan.db");
    let _tracer = yuantuan_core::event::spawn_tracer(&bus, db_path.clone());

    // e. adapter-qq（[napcat].enabled=false 则跳过；连不上退避重连不死进程）
    let _adapter = if cfg.napcat.enabled {
        info!(ws_url = %cfg.napcat.ws_url, "adapter-qq 启动（正向 WS）");
        Some(yuantuan_adapter_qq::spawn(
            bus.clone(),
            db_path.clone(),
            yuantuan_adapter_qq::NapcatConfig {
                ws_url: cfg.napcat.ws_url.clone(),
                token: cfg.napcat.token.clone(),
            },
        ))
    } else {
        info!("[napcat].enabled=false，跳过 adapter-qq");
        None
    };

    // f. WebUI（阻塞至进程结束）
    info!("云团骨架启动成功");
    info!(
        url = %format!("http://{}:{}/", cfg.webui.host, cfg.webui.port),
        "管理员面板；首次登录提交的密码即为管理员密码（首启引导）"
    );
    yuantuan_webui::serve(db_path, &cfg.webui.host, cfg.webui.port).await
}
