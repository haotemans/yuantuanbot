//! 云团装配入口：配置 → data 目录 → SQLite 迁移 → 日志。
//! 领域系统尚未接入（core / adapter-qq / webui 均为占位 crate）。

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
        log_level = %cfg.log.level,
        "配置加载完成"
    );

    // b. data/ 目录结构
    let data_root = db::init_data_dirs(&cfg.data.dir)?;
    info!(path = %data_root.display(), "data/ 目录结构就绪（memes/ artifacts/ archive/ logs/ backups/）");

    // c. SQLite（WAL）+ 迁移
    let mut conn = db::open(&cfg.data.dir)?;
    info!(db = %data_root.join("yuantuan.db").display(), "SQLite 已打开（WAL）");
    db::migrate(&mut conn)?;

    let tables = db::list_tables(&conn)?;
    info!(count = tables.len(), tables = ?tables, "迁移完成，库内表清单");

    // e. WebUI（占位数仪表盘 + 密码登录），阻塞至进程结束
    let db_path = std::path::Path::new(&cfg.data.dir).join("yuantuan.db");
    info!("云团骨架启动成功");
    info!(
        url = %format!("http://{}:{}/", cfg.webui.host, cfg.webui.port),
        "管理员面板；首次登录提交的密码即为管理员密码（首启引导）"
    );
    yuantuan_webui::serve(db_path, &cfg.webui.host, cfg.webui.port).await
}
