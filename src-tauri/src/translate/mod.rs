//! 翻译引擎（需求：独立翻译栏目 + 词级译文）。
//!
//! 三级策略，从快到慢、从准到兜底：
//!
//! ```text
//!   1. 本地缓存（SQLite trans_cache）  —— 命中即返回，零网络
//!   2. 有道在线翻译                    —— 快且准，但**有频率限制**
//!   3. 本地大模型                      —— 在线失败/限频时兜底，离线也能用
//! ```
//!
//! ## 为什么必须有第 1 和第 3 级
//!
//! 有道那个公开接口（`aidemo.youdao.com/trans`）实测**限频很严**：
//! 连续快速请求会返回 `errorCode: 411`。实测同一时刻连打 5 次只有前
//! 几次成功。所以：
//!
//! - 必须**缓存**：翻译页的「实时翻译」会反复请求同一句话的前缀；
//! - 必须**节流**：全局最小调用间隔 + 限频后冷却，避免越撞越久；
//! - 必须**兜底**：限频时降级到本地大模型，用户不至于看到「翻译失败」。
//!
//! ## 读音与文字的关系（需求 2）
//!
//! 有道返回的 `basic.phonetic` 对中文词是**拼音**、对日语词是**假名注音**。
//! 老代码把它当成「释义主体」渲染，于是「选日语查『你好』」只看到
//! `nǐ hǎo` 而看不到 `こんにちは`。本模块把二者严格分开：
//! `text` 是目标语言的**实际文字**，`phonetic` 是附属读音，前端单独一行展示。

use crate::models::{LlmConfig, NetworkConfig};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 源语言选择「自动检测」时的取值。
pub const AUTO: &str = "auto";

/// 有道语言码。
///
/// 注意中文必须是 `zh-CHS`（简体），用 `zh` 会被当成未知语言；
/// 而 `auto` 这个值该接口**不支持**（返回 411），所以源语言为 auto 时
/// 我们要先用 [`crate::dict::detect_lang`] 猜一个真实语言码。
pub fn youdao_code(code: &str) -> &'static str {
    match code.trim() {
        "zh" | "zh-CHS" | "zh-CN" | "zh-Hans" => "zh-CHS",
        "en" => "en",
        "ja" => "ja",
        "ko" => "ko",
        "fr" => "fr",
        "de" => "de",
        "es" => "es",
        "ru" => "ru",
        "pt" => "pt",
        "it" => "it",
        _ => "en",
    }
}

/// 有道的语言码 → 本项目语言码（用于回填「实际检测到的源语言」）。
pub fn from_youdao_code(code: &str) -> String {
    let c = code.trim();
    // 形如 `zh-CHS2ja`，取 2 左边
    let src = c.split('2').next().unwrap_or(c);
    match src {
        "zh-CHS" | "zh-CHT" => "zh".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => String::new(),
    }
}

/// 界面语言清单：(代码, 中文名, 该语言的自称)。
///
/// 中文名给界面下拉用，自称在提示词里对模型说更有效
/// （见 [`crate::llm::explain_lang_name`]）。
pub const LANGS: [(&str, &str, &str); 10] = [
    ("zh", "中文", "简体中文"),
    ("en", "英语", "English"),
    ("ja", "日语", "日本語"),
    ("ko", "韩语", "한국어"),
    ("fr", "法语", "Français"),
    ("de", "德语", "Deutsch"),
    ("es", "西班牙语", "Español"),
    ("ru", "俄语", "Русский"),
    ("pt", "葡萄牙语", "Português"),
    ("it", "意大利语", "Italiano"),
];

/// 语言码 → 中文名。
pub fn lang_name(code: &str) -> String {
    if code == AUTO {
        return "自动检测".to_string();
    }
    LANGS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, n, _)| n.to_string())
        .unwrap_or_else(|| code.to_string())
}

/// 语言码 → 该语言的自称（给模型看）。
pub fn lang_self_name(code: &str) -> String {
    LANGS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, _, s)| s.to_string())
        .unwrap_or_else(|| "简体中文".to_string())
}

/* ---------------- 限频保护 ---------------- */

/// 两次有道调用之间的最小间隔。实测该接口在密集请求下会返回 411。
const MIN_INTERVAL: Duration = Duration::from_millis(1200);
/// 撞上限频后的冷却时间。
const COOLDOWN: Duration = Duration::from_secs(20);

/// 上次调用时刻 + 冷却截止时刻。进程内全局。
static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle {
    last: None,
    cooldown_until: None,
});

struct Throttle {
    last: Option<Instant>,
    cooldown_until: Option<Instant>,
}

impl Throttle {
    /// 现在能不能发请求？返回 `Err(还要等多久)` 表示不能。
    fn check(&self) -> Result<(), Duration> {
        let now = Instant::now();
        if let Some(until) = self.cooldown_until {
            if now < until {
                return Err(until - now);
            }
        }
        if let Some(last) = self.last {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < MIN_INTERVAL {
                return Err(MIN_INTERVAL - elapsed);
            }
        }
        Ok(())
    }
}

fn lock_throttle() -> std::sync::MutexGuard<'static, Throttle> {
    THROTTLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 现在是否处于限频冷却中（前端据此显示提示）。
pub fn cooldown_remaining() -> Option<Duration> {
    lock_throttle().check().err()
}

/// 记录一次调用；`rate_limited` 为真时进入冷却。
fn note_call(rate_limited: bool) {
    let mut t = lock_throttle();
    t.last = Some(Instant::now());
    if rate_limited {
        t.cooldown_until = Some(Instant::now() + COOLDOWN);
    }
}

/* ---------------- 结果结构 ---------------- */

/// 单词级的词典补充信息（有道 `basic` 字段）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DictBrief {
    /// 音标 / 罗马音 / 假名注音（**附属信息**，不是译文主体）
    pub phonetic: String,
    /// 词性 + 释义的行
    pub explains: Vec<String>,
}

/// 一次翻译的结果。
///
/// 同时实现 `Deserialize`：翻译结果会被整体序列化进 `trans_cache`，
/// 读缓存时要原样还原。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranslateResult {
    /// 原始输入
    pub source: String,
    /// ★ 目标语言的**实际文字**（例：「你好」→「こんにちは」）
    pub text: String,
    /// 其他候选译文
    pub alternatives: Vec<String>,
    /// 实际生效的源语言（自动检测后会回填检测结果）
    pub from: String,
    /// 目标语言
    pub to: String,
    /// 读音标注，单独一行展示（拼音 / 罗马音 / 假名注音）
    pub phonetic: String,
    /// 目标语言朗读音频地址（有道 TTS）
    pub tts_url: String,
    /// 命中的引擎：`cache` / `youdao` / `llm`
    pub engine: String,
    /// 是否来自缓存
    pub from_cache: bool,
    /// 降级/失败说明。非空时前端应如实告知用户
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 单词级的词性与释义
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dict: Option<DictBrief>,
    /// 网络释义（有道 web 字段）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web: Vec<String>,
    /// 这条翻译在历史表里的记录 id，前端「收藏」按钮直接用它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_id: Option<i64>,
}

impl TranslateResult {
    /// 译文是否已经有内容
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/* ---------------- 有道在线翻译 ---------------- */

const YOUDAO_API: &str = "https://aidemo.youdao.com/trans";

/// 有道返回体。字段名与官方 JSON 一致，用 `rename` 对齐。
#[derive(Debug, Deserialize)]
struct YoudaoResp {
    #[serde(rename = "errorCode", default)]
    error_code: String,
    #[serde(default)]
    translation: Vec<String>,
    /// 形如 `zh-CHS2ja`：实际生效的方向
    #[serde(default)]
    l: String,
    #[serde(rename = "tSpeakUrl", default)]
    t_speak_url: String,
    #[serde(default)]
    basic: Option<Value>,
    #[serde(default)]
    web: Option<Vec<Value>>,
}

/// 有道错误码 → 人类可读说明。
fn youdao_error(code: &str) -> String {
    match code {
        "0" => String::new(),
        // 该公开接口在密集请求下就是靠 411 限流
        "411" => "在线翻译接口触发频率限制".to_string(),
        "401" | "402" | "403" => "在线翻译接口拒绝了本次请求".to_string(),
        other => format!("在线翻译接口返回错误码 {}", other),
    }
}

/// 用文本内容粗判语言。
///
/// 优先用 [`crate::dict::detect_lang`] 的书写系统判定；判不出来（纯拉丁字母）
/// 时按「非中文母语用户最可能查的是英语」兜底成 `en`。
pub fn guess(text: &str) -> String {
    crate::dict::detect_lang(text)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "en".to_string())
}

/// 互换方向时，「自动检测」该换成哪个目标语言。
///
/// 源语言是「自动检测」意味着我们不知道原文是什么语言，也就无法直接对调。
/// 此时按用户的真实意图处理：他想反向翻译，而本软件的用户界面与 AI 讲解
/// 默认都是中文，所以反向的目标最可能是中文；若当前目标本来就是中文，
/// 就退到英语（最常见的第二语言）。
pub fn default_target_for(current_target: &str) -> String {
    if current_target == "zh" {
        "en".to_string()
    } else {
        "zh".to_string()
    }
}

/// 调用有道翻译。`from` 为 [`AUTO`] 时先本地猜一个语言码
/// （该接口不接受 `auto`）。
pub async fn youdao_translate(
    client: &reqwest::Client,
    net: &NetworkConfig,
    from: &str,
    to: &str,
    text: &str,
) -> Result<TranslateResult> {
    if cooldown_remaining().is_some() {
        return Err(anyhow!("在线翻译正在冷却中（触发过频率限制）"));
    }

    let src_code = if from == AUTO {
        guess(text)
    } else {
        from.to_string()
    };
    let y_from = youdao_code(&src_code);
    let y_to = youdao_code(to);

    // UA 由 client 统一带上（见 net::build_client）
    let resp = client
        .get(YOUDAO_API)
        .query(&[("q", text), ("from", y_from), ("to", y_to)])
        .timeout(Duration::from_secs(net.timeout_secs.clamp(5, 30) as u64))
        .send()
        .await
        .map_err(|e| {
            anyhow!(
                "在线翻译请求失败：{}",
                crate::net::friendly_reqwest_error(&e)
            )
        })?;

    let status = resp.status();
    let body = resp.text().await.context("读取在线翻译响应失败")?;

    if !status.is_success() {
        return Err(anyhow!(
            "在线翻译返回 HTTP {}：{}",
            status.as_u16(),
            body.chars().take(160).collect::<String>()
        ));
    }

    let parsed: YoudaoResp = serde_json::from_str(&body)
        .with_context(|| format!("在线翻译响应解析失败：{}", body.chars().take(160).collect::<String>()))?;

    if parsed.error_code != "0" {
        let msg = youdao_error(&parsed.error_code);
        note_call(parsed.error_code == "411");
        return Err(anyhow!(msg));
    }
    note_call(false);

    // 译文主体：第一条是主译文，其余作为候选
    let mut translation = parsed.translation;
    let text_out = if translation.is_empty() {
        String::new()
    } else {
        translation.remove(0)
    };

    // `l` 形如 `zh-CHS2ja`，能拿到「实际检测到的源语言」
    let detected = from_youdao_code(&parsed.l);
    let from_out = if from == AUTO && !detected.is_empty() {
        detected
    } else {
        src_code.clone()
    };

    let mut dict = None;
    let mut phonetic = String::new();
    if let Some(b) = parsed.basic.as_ref() {
        // 读音：音标 / 罗马音 / 假名注音。**只作附属标注**
        for key in ["phonetic", "us-phonetic", "uk-phonetic"] {
            if let Some(s) = b.get(key).and_then(|v| v.as_str()) {
                if !s.trim().is_empty() {
                    phonetic = s.trim().to_string();
                    break;
                }
            }
        }
        let explains = b
            .get("explains")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !explains.is_empty() || !phonetic.is_empty() {
            dict = Some(DictBrief {
                phonetic: phonetic.clone(),
                explains,
            });
        }
    }

    // 网络释义：web[].value
    let web = parsed
        .web
        .unwrap_or_default()
        .iter()
        .filter_map(|w| w.get("value").and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>();

    Ok(TranslateResult {
        source: text.to_string(),
        text: text_out,
        alternatives: translation
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        from: from_out,
        to: to.to_string(),
        phonetic,
        tts_url: parsed.t_speak_url,
        engine: "youdao".to_string(),
        from_cache: false,
        note: None,
        record_id: None,
        dict,
        web,
    })
}

/* ---------------- 本地大模型兜底 ---------------- */

/// 翻译用的 system 提示词。
///
/// 与 [`crate::llm::translate_markdown`] 同一套原则：把「规则」放 system、
/// 「待翻译内容」放 user，并明确禁止输出思考过程 —— 否则推理类模型会把
/// 规则本身复述一遍当答案。
const TRANSLATE_SYSTEM: &str = "你是一个翻译引擎。把用户给出的内容翻译成指定语言。\
只输出译文本身：不要分析、不要思考过程、不要前言结语、不要复述规则、不要加引号包裹。";

/// 用本地大模型翻译（离线可用，也是限频时的兜底）。
pub async fn llm_translate(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    from: &str,
    to: &str,
    text: &str,
) -> Result<TranslateResult> {
    let src = if from == AUTO {
        guess(text)
    } else {
        from.to_string()
    };
    let user = format!(
        "把下面的内容从{from}翻译成{to}。\n\
         要求：只输出译文；保留原有换行与段落；不要输出拼音/罗马音；不要解释。\n\n\
         {text}",
        from = lang_self_name(&src),
        to = lang_self_name(to),
        text = text
    );

    let raw = crate::llm::chat_ex(client, cfg, TRANSLATE_SYSTEM, &user, 0.2, cfg.max_tokens.max(2048))
        .await?;
    let text_out = raw.trim().trim_matches('"').trim().to_string();
    if text_out.is_empty() {
        return Err(anyhow!("本地模型返回空译文"));
    }

    Ok(TranslateResult {
        source: text.to_string(),
        text: text_out,
        alternatives: Vec::new(),
        from: src,
        to: to.to_string(),
        phonetic: String::new(),
        tts_url: String::new(),
        engine: "llm".to_string(),
        from_cache: false,
        note: None,
        dict: None,
        web: Vec::new(),
        record_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youdao_lang_codes_are_mapped() {
        assert_eq!(youdao_code("zh"), "zh-CHS", "中文必须是 zh-CHS");
        assert_eq!(youdao_code("ja"), "ja");
        assert_eq!(youdao_code("zh-CN"), "zh-CHS");
        // 未知语言兜底成英语，绝不返回 auto（该接口不认，会 411）
        assert_eq!(youdao_code("xx"), "en");
        assert_ne!(youdao_code(AUTO), "auto");
    }

    #[test]
    fn detects_real_direction_from_response() {
        assert_eq!(from_youdao_code("zh-CHS2ja"), "zh");
        assert_eq!(from_youdao_code("en2zh-CHS"), "en");
        assert_eq!(from_youdao_code("ja2zh-CHS"), "ja");
        assert_eq!(from_youdao_code(""), "");
    }

    #[test]
    fn lang_names_are_human_readable() {
        assert_eq!(lang_name("ja"), "日语");
        assert_eq!(lang_name(AUTO), "自动检测");
        assert_eq!(lang_self_name("ja"), "日本語", "给模型看要用自称");
        assert_eq!(lang_name("xx"), "xx");
    }

    /// 限频错误码必须被识别成「频率限制」，而不是笼统的失败 ——
    /// 前端要据此显示「已降级到本地模型」。
    #[test]
    fn rate_limit_error_is_named() {
        assert!(youdao_error("411").contains("频率限制"));
        assert!(youdao_error("0").is_empty());
        assert!(youdao_error("999").contains("999"));
    }

    /// 源语言为 auto 时不能把 `auto` 透传给接口。
    #[test]
    fn auto_source_falls_back_to_guess() {
        assert_eq!(guess("你好"), "zh");
        assert_eq!(guess("こんにちは"), "ja");
        assert_eq!(guess("hello"), "en");
    }
}
