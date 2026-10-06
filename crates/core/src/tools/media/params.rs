//! NAI 参数常量（Q016/Q018）：移植自 AstrBot `nai_params.rs` 中仍适用的部分（比例映射）+
//! 精选默认质量标签与负面词（用户 2026-10-06："year2025之类，但不要太多"——精简版配方）

/// 比例 → 像素尺寸（NAI 实际支持三种；其它比例由 mapper 拒绝）
pub const NAI_DIMENSIONS: &[(&str, (u32, u32))] = &[
    ("1:1", (1024, 1024)),
    ("2:3", (832, 1216)),
    ("3:2", (1216, 832)),
];

/// 默认质量标签（Q018）：注入到 prompt 头部（若用户 prompt 未自带）。
/// 选用 NAI 社区 2025 年精简配方——四个高信号标签足够，避免过度噪声。
pub const NAI_QUALITY_TAGS: &str = "masterpiece, best quality, year 2025, highres";

/// 默认负面词（Q018 精简版）：8 个高信号词，覆盖画质/解剖/水印三大问题。
/// AstrBot 原版有 17 个，砍半至核心。
pub const NAI_NEGATIVE_PROMPT: &str =
    "lowres, bad anatomy, bad hands, missing fingers, extra digits, worst quality, jpeg artifacts, watermark";

/// 给 prompt 注入默认质量标签（用户 prompt 已含任一标签则跳过，避免重复）。
/// 用法：`let final_prompt = with_quality_tags(&user_prompt);`
pub fn with_quality_tags(prompt: &str) -> String {
    let lower = prompt.to_lowercase();
    let tags: Vec<&str> = NAI_QUALITY_TAGS
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter(|t| !lower.contains(&t.to_lowercase()))
        .collect();
    if tags.is_empty() {
        return prompt.to_string();
    }
    format!("{}, {}", tags.join(", "), prompt)
}

/// 比例 → 像素（NAI 端点；未知比例返回 None 由上层报错）
pub fn nai_dimensions(ratio: &str) -> Option<(u32, u32)> {
    NAI_DIMENSIONS
        .iter()
        .find(|(r, _)| *r == ratio)
        .map(|(_, d)| *d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_tags_injected_when_missing() {
        let out = with_quality_tags("1girl, blue eyes");
        assert!(out.starts_with("masterpiece, best quality, year 2025, highres"));
        assert!(out.contains("blue eyes"));
    }

    #[test]
    fn quality_tags_not_duplicated() {
        let out = with_quality_tags("masterpiece, 1girl");
        assert_eq!(out.matches("masterpiece").count(), 1);
    }
}

