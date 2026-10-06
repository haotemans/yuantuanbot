//! 备份系统（docs/changes/backup-page-workbench.md 裁决 Q-B01..Q-B07）：
//! 数据 = yuantuan.db + memes/ + artifacts/ + plugins/（暂不存在也跳过）
//! 出物 = data/backups/<UTC>.tar.gz
//! 推送 = 启用时 git add+commit+push 到配置的 GitHub 私有仓
//! 滚动 = 本地 backups/ 保留最近 keep_days 天（默认 7）
//! 恢复 = 启动时检测 data/.restore-pending，存在则解压覆盖 data/ 后删除标记

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackupCfg {
    pub enabled: bool,
    /// GitHub 私有仓 URL（如 https://github.com/hsb/yuantuan-backup.git）
    pub repo_url: String,
    /// PAT 环境变量名（密钥不入库不入仓）
    pub pat_env: String,
    /// 每日定时（HH:MM 本地）
    pub daily_time: String,
    /// 本地保留天数（>0；默认 7）
    pub keep_days: i64,
}

impl Default for BackupCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            repo_url: String::new(),
            pat_env: "YUANTUAN_BACKUP_PAT".into(),
            daily_time: "03:00".into(),
            keep_days: 7,
        }
    }
}

pub type SharedBackupCfg = Arc<RwLock<BackupCfg>>;

/// 便捷构造（孤儿规则不允许给 type alias impl Default）
pub fn shared_backup_cfg() -> SharedBackupCfg {
    Arc::new(RwLock::new(BackupCfg::default()))
}

#[derive(Debug, Clone)]
pub struct BackupArtifact {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub elapsed_ms: u64,
    pub pushed: bool,
    pub push_error: Option<String>,
}

/// 备份范围（相对 data/ 的路径）。logs/backups 不入备份。
const INCLUDE: &[&str] = &["yuantuan.db", "memes", "artifacts", "plugins"];
const RESTORE_PENDING: &str = ".restore-pending";

/// 执行一次备份：tar.gz → 滚动清理 → 可选 git push
pub async fn run_backup(cfg: &BackupCfg, data_dir: &Path) -> Result<BackupArtifact> {
    let started = std::time::Instant::now();
    let backups = data_dir.join("backups");
    tokio::fs::create_dir_all(&backups).await.ok();

    let stamp = chrono_stamp();
    let out = backups.join(format!("yt-{stamp}.tar.gz"));

    // 1) tar.gz 打包（tokio 阻塞任务，避免阻塞 async runtime）
    let data_dir_clone = data_dir.to_path_buf();
    let out_clone = out.clone();
    let tar_result = tokio::task::spawn_blocking(move || {
        tar_data_dir(&data_dir_clone, &out_clone)
    })
    .await
    .context("tar spawn_blocking 失败")??;

    // 2) 滚动清理本地
    cleanup_old(&backups, cfg.keep_days).await;

    // 3) git push（启用且配齐才尝试）
    let mut pushed = false;
    let mut push_error = None;
    if cfg.enabled && !cfg.repo_url.is_empty() {
        match git_push(cfg, &backups, &out).await {
            Ok(()) => {
                pushed = true;
                info!(file = %out.display(), "备份已推送 GitHub");
            }
            Err(e) => {
                let msg = format!("{e:#}");
                warn!(error = %msg, "备份 push 失败（tar 已本地保留）");
                push_error = Some(msg);
            }
        }
    }

    Ok(BackupArtifact {
        path: out,
        size_bytes: tar_result,
        elapsed_ms: started.elapsed().as_millis() as u64,
        pushed,
        push_error,
    })
}

fn chrono_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // UTC YYYYMMDD-HHMMSS 简易实现（不引 chrono）
    let secs = now % 86400;
    let days = now / 86400;
    let (y, m, d) = days_to_ymd(days as i64);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

fn days_to_ymd(mut days: i64) -> (i64, u32, u32) {
    // civil calendar from days since epoch
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

fn tar_data_dir(data_dir: &Path, out: &Path) -> Result<u64> {
    let f = std::fs::File::create(out).with_context(|| format!("创建 {} 失败", out.display()))?;
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);

    for rel in INCLUDE {
        let src = data_dir.join(rel);
        if !src.exists() {
            continue;
        }
        if src.is_dir() {
            tar.append_dir_all(rel, &src)
                .with_context(|| format!("打包目录 {rel} 失败"))?;
        } else {
            // yuantuan.db：WAL 模式下先 checkpoint，避免备份出不一致快照
            if *rel == "yuantuan.db" {
                checkpoint_wal(data_dir)?;
            }
            tar.append_path_with_name(&src, rel)
                .with_context(|| format!("打包文件 {rel} 失败"))?;
        }
    }
    let gz = tar.into_inner().context("tar 收尾失败")?;
    let f = gz.finish().context("gzip finish 失败")?;
    let meta = f.metadata().context("读取产物 metadata 失败")?;
    Ok(meta.len())
}

fn checkpoint_wal(data_dir: &Path) -> Result<()> {
    let db = data_dir.join("yuantuan.db");
    if !db.exists() {
        return Ok(());
    }
    let conn = rusqlite::Connection::open(&db).context("打开 db 做 checkpoint 失败")?;
    conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")
        .context("WAL checkpoint 失败")?;
    Ok(())
}

async fn cleanup_old(backups: &Path, keep_days: i64) {
    if keep_days <= 0 {
        return;
    }
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs((keep_days as u64) * 86400);
    let mut rd = match tokio::fs::read_dir(backups).await {
        Ok(r) => r,
        Err(_) => return,
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("yt-") || !name.ends_with(".tar.gz") {
            continue;
        }
        let mtime = e
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok());
        if let Some(t) = mtime {
            if t < cutoff {
                let _ = tokio::fs::remove_file(e.path()).await;
                info!(file = %name, "清理过期备份");
            }
        }
    }
}

/// 恢复标记：data/.restore-pending 内容是要恢复的 tar 文件名（相对 data/backups/）
pub async fn restore_from_pending(data_dir: &Path) -> Result<Option<PathBuf>> {
    let marker = data_dir.join(RESTORE_PENDING);
    if !marker.exists() {
        return Ok(None);
    }
    let target_name = tokio::fs::read_to_string(&marker)
        .await
        .context("读取 .restore-pending 失败")?
        .trim()
        .to_string();
    if target_name.is_empty() {
        tokio::fs::remove_file(&marker).await.ok();
        return Ok(None);
    }
    let tar_path = data_dir.join("backups").join(&target_name);
    if !tar_path.exists() {
        warn!(file = %target_name, "恢复标记指向的 tar 不存在，跳过并删除标记");
        tokio::fs::remove_file(&marker).await.ok();
        return Ok(None);
    }
    info!(file = %target_name, "启动检测到恢复标记，开始解压覆盖 data/");
    let data_dir_clone = data_dir.to_path_buf();
    let tar_clone = tar_path.clone();
    tokio::task::spawn_blocking(move || untar_into(&tar_clone, &data_dir_clone))
        .await
        .context("restore spawn_blocking 失败")??;
    tokio::fs::remove_file(&marker).await.ok();
    info!("恢复完成，标记已清除");
    Ok(Some(tar_path))
}

fn untar_into(tar_path: &Path, data_dir: &Path) -> Result<()> {
    let f = std::fs::File::open(tar_path).with_context(|| format!("打开 {} 失败", tar_path.display()))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(data_dir).context("解压到 data/ 失败")?;
    Ok(())
}

async fn git_push(cfg: &BackupCfg, backups: &Path, new_tar: &Path) -> Result<()> {
    let pat = std::env::var(&cfg.pat_env)
        .with_context(|| format!("环境变量 {} 未设置", cfg.pat_env))?;
    let repo_dir = backups.join("_repo");
    if !repo_dir.exists() {
        // 首次：clone
        let url = inject_pat(&cfg.repo_url, &pat);
        run_git(&backups, &["clone", "--depth", "1", &url, "_repo"]).await?;
    }
    // 拷贝新 tar 到 repo
    let name = new_tar.file_name().unwrap().to_string_lossy().to_string();
    let dst = repo_dir.join(&name);
    tokio::fs::copy(new_tar, &dst).await.context("拷贝 tar 到 repo 失败")?;
    run_git(&repo_dir, &["add", "-A"]).await?;
    run_git(&repo_dir, &["-c", "user.email=bot@yuantuan", "-c", "user.name=yuantuan-backup",
                          "commit", "-m", &format!("backup {name}")]).await?;
    run_git(&repo_dir, &["push"]).await?;
    Ok(())
}

fn inject_pat(url: &str, pat: &str) -> String {
    // https://github.com/x/y.git → https://<pat>@github.com/x/y.git
    if let Some(rest) = url.strip_prefix("https://") {
        format!("https://{pat}@{rest}")
    } else {
        url.to_string()
    }
}

async fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("git 命令启动失败")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {:?} 失败：{}", args, &stderr[..stderr.len().min(200)]);
    }
    Ok(())
}

/// 每日定时调度：每天 daily_time 触发一次 run_backup。enabled=false 时跳过
pub fn spawn_daily_scheduler(cfg: SharedBackupCfg, data_dir: PathBuf) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (enabled, daily_time) = {
                let c = cfg.read().unwrap();
                (c.enabled, c.daily_time.clone())
            };
            if !enabled {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }
            let wait = secs_until(&daily_time);
            info!(hours = wait / 3600, daily_time = %daily_time, "备份下次执行倒计时");
            tokio::time::sleep(std::time::Duration::from_secs(wait.max(1))).await;
            let cfg_snap = cfg.read().unwrap().clone();
            match run_backup(&cfg_snap, &data_dir).await {
                Ok(art) => info!(size = art.size_bytes, pushed = art.pushed, "定时备份完成"),
                Err(e) => warn!(error = %e, "定时备份失败"),
            }
        }
    })
}

fn secs_until(daily_time: &str) -> u64 {
    let (h, m) = daily_time.split_once(':').unwrap_or(("3", "0"));
    let h: u64 = h.trim().parse().unwrap_or(3);
    let m: u64 = m.trim().parse().unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let today_secs = now % 86400;
    let target_secs = h * 3600 + m * 60;
    let diff = if target_secs > today_secs {
        target_secs - today_secs
    } else {
        86400 - today_secs + target_secs
    };
    diff
}
