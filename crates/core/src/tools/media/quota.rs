//! 配额 / 余额 / 权限三次校验（Q009）。当前阶段：实现 SQL 层骨架与判定函数；
//! 实际表结构（media_credits / media_models.daily_quota / media_models.cost_per_result）
//! 在 MOD-022b 落地，本文件 API 已就位等待装配。

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    AdminOnly,
    Everyone,
}

#[derive(Debug, Clone)]
pub struct QuotaCheck {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// 三次校验一体（顺序：权限 → 每日配额 → 余额）；任何一步失败立即给出"拒答文案"
/// - `is_admin`：调用方（QQ 号）是否是管理员
/// - `perm`：该模型的权限
/// - `daily_used/daily_quota`：该模型今日已用 / 配额（quota=0 视为无上限）
/// - `balance/cost`：调用方余额 / 本次任务总成本（单价 × 数量）
pub fn check_can_generate(
    is_admin: bool,
    perm: Permission,
    daily_used: u32,
    daily_quota: u32,
    balance: i64,
    cost: i64,
) -> Result<QuotaCheck> {
    if matches!(perm, Permission::AdminOnly) && !is_admin {
        bail!("该模型仅管理员可用")
    }
    if daily_quota > 0 && daily_used >= daily_quota {
        bail!("本模型今日配额已用完（{daily_used}/{daily_quota}）")
    }
    if cost > 0 && balance < cost {
        bail!("余额不足：本次需 {cost}，当前 {balance}")
    }
    Ok(QuotaCheck {
        allowed: true,
        reason: None,
    })
}
