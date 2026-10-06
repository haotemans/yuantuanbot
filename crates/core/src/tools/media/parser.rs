//! /image 命令解析（Q013/Q015）：shlex 风格 tokenize + `-m/-r` 必填 + `--` 终止符
//!
//! 语法：`/image -m <alias> -r <ratio> [-n N] [-q Q] [--seed N] [--negative TEXT] [--] <prompt...>`
//!
//! 严格模式：任何选项缺值 / 值非法 / 必填缺失，返回 `ParseError`（Q015 统一文案）

use anyhow::bail;

/// 解析成功的命令
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedImageCmd {
    pub model_alias: String,
    pub ratio: String,
    pub count: u32,
    pub quality: Option<String>,
    pub seed: Option<i64>,
    pub negative: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    MissingModelValue,
    MissingRatioValue,
    UnknownModel(String, Vec<String>),
    InvalidRatio(String, Vec<String>),
    UnsupportedRatio { model: String, ratio: String, supported: Vec<String> },
    InvalidCount(String),
    MissingNegativeValue,
    MissingQualityValue,
    MissingSeedValue,
    InvalidSeed(String),
    UnknownOption(String),
    MissingPrompt,
    MissingModel,
    MissingRatio,
}

/// 把命令文本拆为 token（受双引号保护；不支持转义符，与 AstrBot 保持一致）
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// 解析 /image 命令（已去掉 /image 或 /画 前缀的 body）
pub fn parse_image_command(
    body: &str,
    known_models: &[String],
    model_supported_ratios: &dyn Fn(&str) -> Vec<String>,
) -> Result<ParsedImageCmd, ParseError> {
    // 先按 `--` 切成选项段 + prompt 段
    let (opts_text, prompt_text) = match body.split_once("--") {
        Some((a, b)) => (a, b),
        None => {
            // 没显式 `--`：从第一个不以 - 开头的位置起作为 prompt
            let tokens = tokenize(body);
            let mut idx = 0usize;
            while idx < tokens.len() {
                let t = &tokens[idx];
                if !t.starts_with('-') {
                    break;
                }
                // 跳过 -m/-r/-n/-q/--seed/--negative 的"选项 + 值"两段
                match t.as_str() {
                    "-m" | "--model" | "-r" | "--ratio" | "-n" | "--count" | "-q" | "--quality"
                    | "--seed" | "--negative" => idx += 2,
                    _ => idx += 1,
                }
            }
            let opt_end_byte = if idx >= tokens.len() {
                body.len()
            } else {
                // 找到第 idx 个 token 在 body 中的起始位置
                let mut byte_pos = 0usize;
                for (i, t) in tokens.iter().enumerate() {
                    if i == idx {
                        break;
                    }
                    if let Some(p) = body[byte_pos..].find(t.as_str()) {
                        byte_pos += p + t.len();
                    }
                }
                byte_pos
            };
            (&body[..opt_end_byte], &body[opt_end_byte..])
        }
    };

    let tokens = tokenize(opts_text);
    let mut model: Option<String> = None;
    let mut ratio: Option<String> = None;
    let mut count: u32 = 1;
    let mut quality: Option<String> = None;
    let mut seed: Option<i64> = None;
    let mut negative: Option<String> = None;

    let mut i = 0usize;
    while i < tokens.len() {
        let t = tokens[i].as_str();
        match t {
            "-m" | "--model" => {
                let v = tokens.get(i + 1).ok_or(ParseError::MissingModelValue)?;
                if !known_models.iter().any(|m| m == v) {
                    return Err(ParseError::UnknownModel(v.clone(), known_models.to_vec()));
                }
                model = Some(v.clone());
                i += 2;
            }
            "-r" | "--ratio" => {
                let v = tokens.get(i + 1).ok_or(ParseError::MissingRatioValue)?;
                let cleaned = v.trim().to_string();
                // 基本形式校验：数字:数字
                let ok = cleaned.split(':').count() == 2
                    && cleaned
                        .split(':')
                        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
                if !ok {
                    return Err(ParseError::InvalidRatio(v.clone(), vec![
                        "1:1".into(), "2:3".into(), "3:2".into(),
                        "4:3".into(), "3:4".into(),
                        "16:9".into(), "9:16".into(), "21:9".into(), "9:21".into(),
                    ]));
                }
                ratio = Some(cleaned);
                i += 2;
            }
            "-n" | "--count" => {
                let v = tokens.get(i + 1).ok_or(ParseError::InvalidCount("缺值".into()))?;
                let n: u32 = v.parse().map_err(|_| ParseError::InvalidCount(v.clone()))?;
                if n == 0 {
                    return Err(ParseError::InvalidCount(v.clone()));
                }
                count = n;
                i += 2;
            }
            "-q" | "--quality" => {
                let v = tokens.get(i + 1).ok_or(ParseError::MissingQualityValue)?;
                quality = Some(v.clone());
                i += 2;
            }
            "--seed" => {
                let v = tokens.get(i + 1).ok_or(ParseError::MissingSeedValue)?;
                let n: i64 = v.parse().map_err(|_| ParseError::InvalidSeed(v.clone()))?;
                seed = Some(n);
                i += 2;
            }
            "--negative" => {
                let v = tokens.get(i + 1).ok_or(ParseError::MissingNegativeValue)?;
                negative = Some(v.clone());
                i += 2;
            }
            _ => {
                if t.starts_with('-') {
                    return Err(ParseError::UnknownOption(t.to_string()));
                }
                // 非选项 token 出现在选项段（异常）→ 视为未知选项
                return Err(ParseError::UnknownOption(t.to_string()));
            }
        }
    }

    // 必填校验
    let model_alias = model.ok_or(ParseError::MissingModel)?;
    let ratio = ratio.ok_or(ParseError::MissingRatio)?;

    // 该模型是否支持此比例
    let supported = model_supported_ratios(&model_alias);
    if !supported.is_empty() && !supported.iter().any(|r| r == &ratio) {
        return Err(ParseError::UnsupportedRatio {
            model: model_alias.clone(),
            ratio: ratio.clone(),
            supported,
        });
    }

    let prompt = prompt_text.trim().to_string();
    if prompt.is_empty() {
        return Err(ParseError::MissingPrompt);
    }

    Ok(ParsedImageCmd {
        model_alias,
        ratio,
        count,
        quality,
        seed,
        negative,
        prompt,
    })
}

/// 把 ParseError 映射为统一文案（Q015 模板）
pub fn render_error(e: &ParseError) -> String {
    let reason = match e {
        ParseError::MissingModel => "-m 缺少模型别名".to_string(),
        ParseError::MissingRatio => "-r 缺少比例".to_string(),
        ParseError::MissingModelValue => "-m 缺少值".to_string(),
        ParseError::MissingRatioValue => "-r 缺少值".to_string(),
        ParseError::UnknownModel(m, known) => {
            format!("模型别名 \"{m}\" 未注册（可用：{}）", known.join(", "))
        }
        ParseError::InvalidRatio(r, supported) => {
            format!("-r 比例非法: \"{r}\"（应为如 16:9；常用：{}）", supported.join(", "))
        }
        ParseError::UnsupportedRatio { model, ratio, supported } => {
            format!("模型 \"{model}\" 不支持比例 \"{ratio}\"（支持：{}）", supported.join(", "))
        }
        ParseError::InvalidCount(c) => format!("-n 数量必须为正整数，收到 \"{c}\""),
        ParseError::MissingNegativeValue => "--negative 缺少值".to_string(),
        ParseError::MissingQualityValue => "-q 缺少值".to_string(),
        ParseError::MissingSeedValue => "--seed 缺少值".to_string(),
        ParseError::InvalidSeed(s) => format!("--seed 必须为整数，收到 \"{s}\""),
        ParseError::UnknownOption(o) => format!("未知选项 \"{o}\""),
        ParseError::MissingPrompt => "缺少提示词".to_string(),
    };
    format!(
        "❌ /image 命令格式错误\n\n原因：{reason}\n\n用法：/image -m <模型别名> -r <比例> [选项] <提示词>\n  -m   模型别名\n  -r   比例（1:1 / 16:9 / 9:16 / 2:3 / 3:2 / 21:9 等）\n  -n   数量（默认 1）\n  -q   质量档位（按模型支持）\n  --seed <int>       随机种子\n  --negative <text>  负面提示词\n  --                  之后所有视为 prompt 原文\n\n示例：/image -m nai4.5 -r 1:1 一只蓝眼睛的猫"
    )
}

/// 早期 bail 兼容（少数调用点只需要 anyhow），保留 API
#[allow(dead_code)]
pub fn parse_or_bail(
    body: &str,
    known_models: &[String],
    model_supported_ratios: &dyn Fn(&str) -> Vec<String>,
) -> anyhow::Result<ParsedImageCmd> {
    match parse_image_command(body, known_models, model_supported_ratios) {
        Ok(cmd) => Ok(cmd),
        Err(e) => bail!("{}", render_error(&e)),
    }
}
