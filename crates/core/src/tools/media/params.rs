//! NAI 参数常量（Q016）：移植自 AstrBot `nai_params.rs` 中仍适用的部分（比例映射 + 默认负面词）

/// 比例 → 像素尺寸（NAI 实际支持三种；其它比例由 mapper 拒绝）
pub const NAI_DIMENSIONS: &[(&str, (u32, u32))] = &[
    ("1:1", (1024, 1024)),
    ("2:3", (832, 1216)),
    ("3:2", (1216, 832)),
];

/// 默认负面提示词（用户 `--negative` 未提供时使用）
pub const NAI_NEGATIVE_PROMPT: &str = "lowres, artistic error, film grain, scan artifacts, worst quality, bad quality, jpeg artifacts, very displeasing, chromatic aberration, dithering, halftone, screentone, multiple views, logo, too many watermarks, negative space, blank page";

/// 比例 → 像素（NAI 端点；未知比例返回 None 由上层报错）
pub fn nai_dimensions(ratio: &str) -> Option<(u32, u32)> {
    NAI_DIMENSIONS
        .iter()
        .find(|(r, _)| *r == ratio)
        .map(|(_, d)| *d)
}
