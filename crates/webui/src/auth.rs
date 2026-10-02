//! 登录与 session 校验。
//! 首启引导：state_kv 无 admin_pass_hash 时，第一次提交的密码即管理员密码（argon2 入库）；
//! 之后校验，错密 401。session 为随机 token，内存存储，24h 过期。

use crate::AppState;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::distr::Alphanumeric;
use rand::{Rng, RngCore};
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
pub struct LoginReq {
    password: String,
}

/// 免 token：登录页据此显示真实首启引导状态
pub async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let need_setup = state
        .open_db()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT COUNT(*) > 0 FROM state_kv WHERE key = 'admin_pass_hash'",
                [],
                |r| r.get::<_, bool>(0),
            )
            .ok()
        })
        .map(|has| !has)
        .unwrap_or(false); // 库不可读时按已配置处理，不引导
    Json(json!({ "need_setup": need_setup }))
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginReq>,
) -> Response {
    if req.password.is_empty() {
        return err(StatusCode::BAD_REQUEST, "password 不能为空");
    }

    let conn = match state.open_db() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("数据库打开失败: {e}")),
    };
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM state_kv WHERE key = 'admin_pass_hash'",
            [],
            |r| r.get(0),
        )
        .ok();

    match stored {
        // 首启引导：第一次提交的密码即管理员密码
        None => {
            let mut salt_bytes = [0u8; 16];
            rand::rng().fill_bytes(&mut salt_bytes);
            let salt = match SaltString::encode_b64(&salt_bytes) {
                Ok(s) => s,
                Err(e) => {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("盐编码失败: {e}"),
                    )
                }
            };
            let hash = match Argon2::default().hash_password(req.password.as_bytes(), &salt) {
                Ok(h) => h.to_string(),
                Err(e) => {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("密码哈希失败: {e}"),
                    )
                }
            };
            let now = epoch_secs();
            if let Err(e) = conn.execute(
                "INSERT INTO state_kv(key, value, updated_at) VALUES ('admin_pass_hash', ?1, ?2)",
                rusqlite::params![hash, now],
            ) {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("写入管理员密码失败: {e}"),
                );
            }
            tracing::info!("首启引导：管理员密码已设置");
            issue(&state)
        }
        Some(hash) => {
            let parsed = match PasswordHash::new(&hash) {
                Ok(p) => p,
                Err(e) => {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("库中密码哈希损坏: {e}"),
                    )
                }
            };
            if Argon2::default()
                .verify_password(req.password.as_bytes(), &parsed)
                .is_err()
            {
                return err(StatusCode::UNAUTHORIZED, "密码错误");
            }
            issue(&state)
        }
    }
}

fn issue(state: &AppState) -> Response {
    let token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();
    state.issue_session(token.clone());
    (StatusCode::OK, Json(json!({ "token": token }))).into_response()
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// /api/*（login 除外）统一过 Bearer token 校验
pub async fn require_session(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match token {
        Some(t) if state.valid_session(t) => next.run(req).await,
        _ => err(StatusCode::UNAUTHORIZED, "未登录或会话已过期"),
    }
}
