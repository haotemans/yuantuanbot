//! Meme 表情包管线（架构文档十四章 + docs/reference/data-model.md 七章）：
//! - 入库扫描：启动扫 memes 根（jpg/png/gif/webp），md5 精确 + dHash 汉明距≤5 感知去重，
//!   类别取子目录名（无子目录归 'misc'），手动放入的视为管理员入库（added_by='admin', active）
//! - 抽图：类别内 active 按 last_used_ts 最久未用优先——ORDER BY 取前 3 随机一（杜绝三连发），
//!   选中即更新 use_count/last_used_ts
//! - 偷表情包：订阅 MessageReceived，群图片消息（按 url 下载）→ _inbox/ → pending（added_by='steal'）
//!
//! dHash 手写（9x8 灰度缩图→64bit，image crate 解码，2C2G 预算内；gif 取首帧）。

use crate::db::params;
use crate::db::SqliteExt;
use crate::event::{Event, EventBus};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use sqlx::Row;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

pub const DHASH_DUP_THRESHOLD: u32 = 5;
const IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "gif"];

pub fn is_image_ext(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    IMAGE_EXTS.contains(&ext.as_str()) || ext == "webp"
}

// ---------- 哈希 ----------

pub fn compute_md5(bytes: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// dHash：9x8 灰度缩图，左>右置位得 64bit；解码失败 → None
pub fn compute_dhash(bytes: &[u8]) -> Option<u64> {
    let img = image::load_from_memory(bytes).ok()?;
    let small =
        image::imageops::resize(&img.to_luma8(), 9, 8, image::imageops::FilterType::Triangle);
    let px = small.as_raw();
    let mut hash: u64 = 0;
    for y in 0..8usize {
        for x in 0..8usize {
            hash <<= 1;
            if px[y * 9 + x] > px[y * 9 + x + 1] {
                hash |= 1;
            }
        }
    }
    Some(hash)
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

// ---------- 入库扫描 ----------

#[derive(Debug, Default, Clone, Copy)]
pub struct ScanStats {
    pub scanned: i64,
    pub added: i64,
    pub skipped_dup: i64,
}

/// 启动扫描 memes 根：根目录文件归 'misc'，一层子目录取目录名；
/// '_inbox'（偷表情包待审区）跳过——它的行只由 steal 流程创建。
pub async fn scan_and_ingest(memes_dir: &Path, db_path: &Path) -> Result<ScanStats> {
    let mut stats = ScanStats::default();
    if !memes_dir.is_dir() {
        return Ok(stats);
    }
    let mut conn = crate::db::connect(db_path).await?;
    for entry in std::fs::read_dir(memes_dir)
        .with_context(|| format!("读取 memes 目录失败: {}", memes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if file_name == "_inbox" {
                continue;
            }
            let Ok(sub) = std::fs::read_dir(&path) else {
                continue;
            };
            for sub_entry in sub.flatten() {
                let sp = sub_entry.path();
                if sp.is_file() && is_image_ext(&sub_entry.file_name().to_string_lossy()) {
                    ingest_one(&mut conn, memes_dir, &sp, &file_name, &mut stats).await?;
                }
            }
        } else if path.is_file() && is_image_ext(&file_name) {
            ingest_one(&mut conn, memes_dir, &path, "misc", &mut stats).await?;
        }
    }
    Ok(stats)
}

async fn ingest_one(
    conn: &mut sqlx::SqliteConnection,
    memes_dir: &Path,
    path: &Path,
    category: &str,
    stats: &mut ScanStats,
) -> Result<()> {
    stats.scanned += 1;
    let rel = path
        .strip_prefix(memes_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    // 已按路径入过库 → 跳过
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM meme_library WHERE file = ?1",
            params![rel],
            |r| r.try_get(0),
        )
        .await
        .unwrap_or(false);
    if exists {
        return Ok(());
    }
    let bytes = std::fs::read(path).with_context(|| format!("读取图片失败: {}", path.display()))?;
    match insert_new_meme(conn, &rel, category, "admin", "active", &bytes).await? {
        InsertOutcome::Added => stats.added += 1,
        InsertOutcome::Duplicated => stats.skipped_dup += 1,
    }
    Ok(())
}

enum InsertOutcome {
    Added,
    Duplicated,
}

/// md5 精确去重 + dHash 感知去重（≤5 即重）；通过则入库
async fn insert_new_meme(
    conn: &mut sqlx::SqliteConnection,
    rel: &str,
    category: &str,
    added_by: &str,
    status: &str,
    bytes: &[u8],
) -> Result<InsertOutcome> {
    let md5 = compute_md5(bytes);
    let dhash = compute_dhash(bytes);
    if is_duplicate(conn, &md5, dhash).await {
        debug!(file = rel, "md5/dHash 去重命中，跳过");
        return Ok(InsertOutcome::Duplicated);
    }
    let now = now_secs();
    conn.execute(
        "INSERT INTO meme_library(file, category, md5, phash, added_by, status, use_count, last_used_ts, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, ?7)",
        params![
            rel,
            category,
            md5,
            dhash.map(|h| format!("{h:016x}")),
            added_by,
            status,
            now
        ],
    ).await?;
    Ok(InsertOutcome::Added)
}

// ---------- 抽图 ----------

/// 类别内 active 抽一张：last_used_ts 最久未用优先（NULL 最先），取前 3 随机一；
/// 选中即更新 use_count/last_used_ts。返回绝对路径。
pub async fn pick(db_path: &Path, memes_dir: &Path, category: &str) -> Option<PathBuf> {
    pick_with(db_path, memes_dir, category, |n| {
        rand::Rng::random_range(&mut rand::rng(), 0..n)
    })
    .await
}

/// pick 的核心（候选选择器可注入，测试可确定性指定下标）
async fn pick_with(
    db_path: &Path,
    memes_dir: &Path,
    category: &str,
    choose: impl Fn(usize) -> usize,
) -> Option<PathBuf> {
    let mut conn = crate::db::connect(db_path).await.ok()?;
    let candidates: Vec<(i64, String)> = {
        let mut st = conn
            .prepare(
                "SELECT id, file FROM meme_library WHERE category = ?1 AND status = 'active'
             ORDER BY last_used_ts IS NOT NULL ASC, last_used_ts ASC LIMIT 3",
            )
            .await
            .ok()?;
        let rows = st
            .query_map(params![category], |r| Ok((r.try_get(0)?, r.try_get(1)?)))
            .await
            .ok()?;
        rows.collect::<std::result::Result<Vec<(i64, String)>, _>>()
    }
    .ok()?;
    if candidates.is_empty() {
        return None;
    }
    let idx = choose(candidates.len()).min(candidates.len() - 1);
    let (id, rel) = &candidates[idx];
    let now = now_secs();
    if let Err(e) = conn
        .execute(
            "UPDATE meme_library SET use_count = use_count + 1, last_used_ts = ?1 WHERE id = ?2",
            params![now, id],
        )
        .await
    {
        warn!(error = %e, "meme 使用计数更新失败");
    }
    Some(memes_dir.join(rel))
}

/// OneBot image 段 file 字段：file:// 绝对路径（正斜杠）
pub fn file_url(path: &Path) -> String {
    let abs = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    let abs = abs.strip_prefix("//?/").unwrap_or(&abs).to_string();
    format!("file:///{abs}")
}

// ---------- 偷表情包 ----------

/// 偷表情包开关共享槽（热应用：WebUI 配置写回后换槽，监听每事件读取当前值）
pub type SharedSteal = std::sync::Arc<std::sync::RwLock<bool>>;

/// 装配侧无条件启动；enabled 槽为 false 时跳过处理（订阅群图片消息，URL 下载 → _inbox → pending）
/// 概率采样：每张图只以 steal_sample_rate（默认 0.5%）概率入待审，避免待审队列被滥用爆库
pub fn spawn_steal_listener(
    bus: &EventBus,
    db_path: PathBuf,
    memes_dir: PathBuf,
    enabled: SharedSteal,
) -> JoinHandle<()> {
    spawn_steal_listener_with_rate(bus, db_path, memes_dir, enabled, STEAL_SAMPLE_RATE)
}

/// 偷图采样率：每张群图片只以 0.5% 概率入待审队列。原本"凡图必偷"的策略在多人群场景会爆待审区
pub const STEAL_SAMPLE_RATE: f64 = 0.005;

pub fn spawn_steal_listener_with_rate(
    bus: &EventBus,
    db_path: PathBuf,
    memes_dir: PathBuf,
    enabled: SharedSteal,
    sample_rate: f64,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let http = match reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "偷表情包 HTTP client 构建失败");
                return;
            }
        };
        info!(
            sample_rate,
            "偷表情包监听已启动（待审区 data/memes/_inbox/；开关走热应用槽；按概率采样）"
        );
        loop {
            match rx.recv().await {
                Ok(Event::MessageReceived(m)) => {
                    if !*enabled.read().unwrap() {
                        continue;
                    }
                    if m.chat_type != "group" || !m.has_image || m.image_urls.is_empty() {
                        continue;
                    }
                    for url in &m.image_urls {
                        // 每张图独立采样，0.5% 概率入待审
                        let roll: f64 = rand::Rng::random(&mut rand::rng());
                        if roll >= sample_rate {
                            continue;
                        }
                        if let Err(e) = steal_one(&http, &db_path, &memes_dir, url).await {
                            debug!(error = %e, url = %url, "图片不入待审（下载失败或重复）");
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "偷表情包消费滞后");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

async fn steal_one(
    http: &reqwest::Client,
    db_path: &Path,
    memes_dir: &Path,
    url: &str,
) -> Result<()> {
    let resp = http.get(url).send().await.context("下载图片失败")?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    if resp
        .content_length()
        .map(|n| n > 10 * 1024 * 1024)
        .unwrap_or(false)
    {
        anyhow::bail!("图片超过 10MB 上限");
    }
    let bytes = resp.bytes().await.context("读取图片体失败")?;

    let md5 = compute_md5(&bytes);
    let mut conn = crate::db::connect(db_path).await?;
    // 先查重再落盘：重复图与已入库行共享 md5 文件名，绝不能覆盖/误删已有行的文件
    if is_duplicate(&mut conn, &md5, compute_dhash(&bytes)).await {
        anyhow::bail!("与库内图片重复");
    }

    // 扩展名：URL 路径尾 → 嗅探 → bin
    let ext = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .and_then(|n| n.rsplit('.').next().map(|e| e.to_lowercase()))
        .filter(|e| IMAGE_EXTS.contains(&e.as_str()) || e == "webp")
        .or_else(|| {
            image::guess_format(&bytes).ok().map(|f| {
                f.extensions_str()
                    .first()
                    .copied()
                    .unwrap_or("bin")
                    .to_string()
            })
        })
        .unwrap_or_else(|| "bin".into());

    let inbox = memes_dir.join("_inbox");
    std::fs::create_dir_all(&inbox)?;
    let rel = format!("_inbox/{md5}.{ext}");
    let out = inbox.join(format!("{md5}.{ext}"));
    std::fs::write(&out, &bytes)?;

    insert_new_meme(&mut conn, &rel, "_inbox", "steal", "pending", &bytes).await?;
    info!(file = %rel, "偷到表情包，待审");
    Ok(())
}

/// md5 精确或 dHash 汉明距≤5 命中库内任意行
pub async fn is_duplicate(
    conn: &mut sqlx::SqliteConnection,
    md5: &str,
    dhash: Option<u64>,
) -> bool {
    let exact: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM meme_library WHERE md5 = ?1",
            params![md5],
            |r| r.try_get(0),
        )
        .await
        .unwrap_or(false);
    if exact {
        return true;
    }
    let Some(h) = dhash else { return false };
    let mut stmt = match conn
        .prepare("SELECT phash FROM meme_library WHERE phash IS NOT NULL AND phash != ''")
        .await
    {
        Ok(s) => s,
        Err(_) => return false,
    };
    let rows: Vec<String> = stmt
        .query_map(crate::db::params![], |r| r.try_get(0))
        .await
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    rows.iter()
        .filter_map(|p| u64::from_str_radix(p, 16).ok())
        .any(|existing| hamming(h, existing) <= DHASH_DUP_THRESHOLD)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_image(seed: u8) -> image::RgbImage {
        // 结构化图案：大块明暗 + 圆斑，dHash 对压缩伪影应以稳定为主
        let mut img = image::RgbImage::new(96, 96);
        for y in 0..96u32 {
            for x in 0..96u32 {
                let base = if (x + y) % 96 < 48 { 230u8 } else { 30u8 };
                let blob =
                    ((x as i32 - 24 * seed as i32).abs() < 12 && (y as i32 - 40).abs() < 12) as u8;
                let v = if blob == 1 {
                    255u8.saturating_sub(base)
                } else {
                    base
                };
                img.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        img
    }

    fn encode_png(img: &image::RgbImage) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn encode_jpeg(img: &image::RgbImage, quality: u8) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        img.write_with_encoder(enc).unwrap();
        buf
    }

    #[test]
    fn dhash_same_image_different_compression_close() {
        let img = pattern_image(2);
        let a = compute_dhash(&encode_png(&img)).unwrap();
        let b = compute_dhash(&encode_jpeg(&img, 60)).unwrap();
        let dist = hamming(a, b);
        assert!(
            dist <= DHASH_DUP_THRESHOLD,
            "同图不同压缩率距离 {dist} 应 ≤5"
        );
    }

    #[test]
    fn dhash_different_image_far() {
        let a = compute_dhash(&encode_png(&pattern_image(2))).unwrap();
        let b = compute_dhash(&encode_png(&pattern_image(3))).unwrap();
        let dist = hamming(a, b);
        assert!(dist > DHASH_DUP_THRESHOLD, "不同图距离 {dist} 应 >5");
    }

    async fn temp_db() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
            + (std::process::id() as u128)
            << 16;
        let dir = std::env::temp_dir().join(format!("yt-meme-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("yuantuan.db");
        let mut conn = crate::db::connect(&db).await.unwrap();
        crate::db::migrate(&mut conn).await.unwrap();
        dir
    }

    #[tokio::test]
    async fn pick_prefers_longest_unused() {
        let dir = temp_db().await;
        let db = dir.join("yuantuan.db");
        let memes = dir.join("memes");
        std::fs::create_dir_all(&memes).unwrap();
        let mut conn = crate::db::connect(&db).await.unwrap();
        for (id, ts) in [
            (1, Some(100)),
            (2, Some(200)),
            (3, Some(300)),
            (4, Some(400)),
        ] {
            conn.execute(
                "INSERT INTO meme_library(id, file, category, md5, added_by, status, use_count, last_used_ts, created_at)
                 VALUES (?1, ?2, '开心', ?3, 'admin', 'active', 0, ?4, 100)",
                params![id, format!("f{id}.png"), format!("md5-{id}"), ts],
            ).await
            .unwrap();
        }
        // 第一轮：top3 = {100,200,300}，ts=400 的 f4 不在候选内
        let p = pick(&db, &memes, "开心").await.expect("应有图可抽");
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert_ne!(name, "f4.png", "第一轮不应抽到最新使用者");
        assert!(matches!(name.as_str(), "f1.png" | "f2.png" | "f3.png"));
        // 抽中即写 use_count/last_used_ts
        let (used, ts): (i64, Option<i64>) = conn
            .query_row(
                "SELECT use_count, last_used_ts FROM meme_library WHERE file = ?1",
                params![name],
                |r| Ok((r.try_get(0)?, r.try_get(1)?)),
            )
            .await
            .unwrap();
        assert_eq!(used, 1);
        assert!(ts.unwrap_or(0) > 400, "last_used_ts 应被刷新到当下");
    }

    #[tokio::test]
    async fn pick_null_last_used_first() {
        let dir = temp_db().await;
        let db = dir.join("yuantuan.db");
        let memes = dir.join("memes");
        std::fs::create_dir_all(&memes).unwrap();
        let mut conn = crate::db::connect(&db).await.unwrap();
        conn.execute(
            "INSERT INTO meme_library(id, file, category, md5, added_by, status, use_count, last_used_ts, created_at)
             VALUES (1, 'fresh.png', '开心', 'md5-1', 'admin', 'active', 0, NULL, 100)", crate::db::params![],
        ).await
        .unwrap();
        conn.execute(
            "INSERT INTO meme_library(id, file, category, md5, added_by, status, use_count, last_used_ts, created_at)
             VALUES (2, 'used.png', '开心', 'md5-2', 'admin', 'active', 0, 100, 100)", crate::db::params![],
        ).await
        .unwrap();
        // NULL 排最前：候选选择器固定取下标 0，从未用过的图必为首选
        let p = pick_with(&db, &memes, "开心", |_| 0).await.unwrap();
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "fresh.png");
    }

    #[tokio::test]
    async fn pick_falls_back_none_on_empty_category() {
        let dir = temp_db().await;
        let db = dir.join("yuantuan.db");
        let memes = dir.join("memes");
        std::fs::create_dir_all(&memes).unwrap();
        assert!(pick(&db, &memes, "不存在").await.is_none());
    }
}
