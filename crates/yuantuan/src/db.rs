//! data/ 目录初始化与数据库打开（迁移与连接实现在 yuantuan_core::db）。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use yuantuan_core::db::Connection;

const SUBDIRS: [&str; 5] = ["memes", "artifacts", "archive", "logs", "backups"];

/// 创建 data/ 及五个子目录，返回规范化后的根路径
pub fn init_data_dirs(data_dir: &str) -> Result<PathBuf> {
    let root = Path::new(data_dir);
    std::fs::create_dir_all(root)
        .with_context(|| format!("创建数据目录失败: {}", root.display()))?;
    for sub in SUBDIRS {
        let p = root.join(sub);
        std::fs::create_dir_all(&p).with_context(|| format!("创建子目录失败: {}", p.display()))?;
    }
    Ok(root.to_path_buf())
}

/// 打开 data/yuantuan.db（WAL / 外键 / busy_timeout 见 yuantuan_core::db::connect）
pub async fn open(data_dir: &str) -> Result<Connection> {
    yuantuan_core::db::connect(&Path::new(data_dir).join("yuantuan.db")).await
}
