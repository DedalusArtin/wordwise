//! LM Studio 客户端（需求：基于本地大模型能力）。
//!
//! LM Studio 暴露的是 OpenAI 兼容接口，因此这里按 OpenAI Chat Completions
//! 协议实现，顺带也兼容 Ollama / vLLM / llama.cpp server 等同类服务。
//!
//! 提供两种调用方式：
//! - `chat`：一次性拿到完整回复（用于结构化生成词条）
//! - `chat_stream`：流式回调（用于讲解面板逐字显示，体验更好）

use crate::models::{Example, LlmConfig, Sense, WordEntry};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// LM Studio 模型信息。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub owned_by: String,
}

/// 连接状态检查结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmStatus {
    pub online: bool,
    pub base_url: String,
    pub models: Vec<String>,
    /// 自动选中的模型
    pub active_model: String,
    pub message: String,
}

/// 构造请求头。
fn headers(cfg: &LlmConfig) -> Vec<(String, String)> {
    let mut h = vec![("Content-Type".to_string(), "application/json".to_string())];
    if !cfg.api_key.is_empty() {
        h.push((
            "Authorization".to_string(),
            format!("Bearer {}", cfg.api_key),
        ));
    }
    h
}

/// 拼接接口地址，容忍用户填 `http://host:1234` 或带 `/v1` 或带完整路径。
pub fn endpoint(base: &str, path: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.ends_with(path.trim_start_matches('/')) {
        return b.to_string();
    }
    if b.ends_with("/v1") {
        format!("{}{}", b, path)
    } else {
        format!("{}/v1{}", b, path)
    }
}

/// 检查 LM Studio 是否在线，并列出可用模型。
pub async fn status(client: &reqwest::Client, cfg: &LlmConfig) -> LlmStatus {
    let url = endpoint(&cfg.base_url, "/models");
    let mut req = client.get(&url).timeout(Duration::from_secs(6));
    for (k, v) in headers(cfg) {
        req = req.header(k, v);
    }

    match req.send().await {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => {
                let models: Vec<String> = v
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let active = if !cfg.model.is_empty() && models.contains(&cfg.model) {
                    cfg.model.clone()
                } else {
                    models.first().cloned().unwrap_or_default()
                };

                let msg = if models.is_empty() {
                    "已连接 LM Studio，但尚未加载任何模型。请在 LM Studio 中加载一个模型。"
                        .to_string()
                } else {
                    format!("已连接，共 {} 个模型可用", models.len())
                };

                LlmStatus {
                    online: true,
                    base_url: cfg.base_url.clone(),
                    models,
                    active_model: active,
                    message: msg,
                }
            }
            Err(e) => LlmStatus {
                online: false,
                base_url: cfg.base_url.clone(),
                models: vec![],
                active_model: String::new(),
                message: format!("响应解析失败: {}", e),
            },
        },
        Ok(r) => LlmStatus {
            online: false,
            base_url: cfg.base_url.clone(),
            models: vec![],
            active_model: String::new(),
            message: format!("服务返回 HTTP {}，请确认 LM Studio 的本地服务已启动", r.status()),
        },
        Err(e) => LlmStatus {
            online: false,
            base_url: cfg.base_url.clone(),
            models: vec![],
            active_model: String::new(),
            message: format!(
                "无法连接 {}（{}）。请在 LM Studio 中打开 Developer → Start Server。",
                cfg.base_url,
                crate::net::friendly_reqwest_error(&e)
            ),
        },
    }
}

/// 解析实际使用的模型名：配置为空则取第一个。
async fn resolve_model(client: &reqwest::Client, cfg: &LlmConfig) -> Result<String> {
    if !cfg.model.trim().is_empty() {
        return Ok(cfg.model.clone());
    }
    let st = status(client, cfg).await;
    if !st.online {
        return Err(anyhow!(st.message));
    }
    if st.active_model.is_empty() {
        return Err(anyhow!(
            "LM Studio 已连接但没有可用模型，请先在 LM Studio 中加载一个模型"
        ));
    }
    Ok(st.active_model)
}

/// 一次性对话（非流式）。
pub async fn chat(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    system: &str,
    user: &str,
) -> Result<String> {
    chat_ex(client, cfg, system, user, cfg.temperature, cfg.max_tokens).await
}

/// 同上，但可覆盖采样参数。
///
/// 翻译（输出后处理层）必须用更高的 `max_tokens`：讲解正文可能有上千 token，
/// 沿用默认的 1024 会把译文**拦腰截断**，而截断后的译文看起来「翻了一半」，
/// 比不翻还糟。
pub async fn chat_ex(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: i64,
) -> Result<String> {
    let model = resolve_model(client, cfg).await?;
    let url = endpoint(&cfg.base_url, "/chat/completions");

    let mut messages = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": user}));

    let body = json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false,
    });

    let mut req = client
        .post(&url)
        .timeout(Duration::from_secs(cfg.timeout_secs.max(10) as u64))
        .json(&body);
    for (k, v) in headers(cfg) {
        req = req.header(k, v);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("调用本地模型失败：{}", crate::net::friendly_reqwest_error(&e)))?;

    let st = resp.status();
    let text = resp.text().await.context("读取响应失败")?;

    if !st.is_success() {
        // LM Studio 在模型未加载时返回的错误信息很有用，直接透传
        let hint = if text.contains("model") || st.as_u16() == 404 {
            "（请确认 LM Studio 中已加载模型，且模型名与配置一致）"
        } else {
            ""
        };
        return Err(anyhow!("本地模型返回 HTTP {} {}: {}", st.as_u16(), hint, truncate(&text, 300)));
    }

    let v: Value = serde_json::from_str(&text).context("模型响应不是合法 JSON")?;
    let content = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();

    if content.trim().is_empty() {
        // 有些推理模型会把内容放在 reasoning_content
        if let Some(rc) = v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("reasoning_content"))
            .and_then(|c| c.as_str())
        {
            if !rc.trim().is_empty() {
                return Ok(rc.to_string());
            }
        }
        return Err(anyhow!("模型返回内容为空，请检查模型是否正常加载"));
    }
    Ok(content)
}

/// 流式分片的类型。
///
/// 推理类模型（DeepSeek-R1、QwQ 等）会先输出思考过程再给正文，
/// 两者必须区分，否则用户会在讲解面板里看到一大堆英文思考内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaKind {
    /// 正式回答内容
    Content,
    /// 模型的思考过程（可折叠展示或忽略）
    Reasoning,
}

/// 流式对话。每收到一个增量片段就调用 `on_delta`。
///
/// 返回值是拼接好的**正文**（不含思考过程）。
pub async fn chat_stream<F>(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    system: &str,
    user: &str,
    mut on_delta: F,
) -> Result<String>
where
    F: FnMut(&str, DeltaKind),
{
    use futures_util::StreamExt;

    let model = resolve_model(client, cfg).await?;
    let url = endpoint(&cfg.base_url, "/chat/completions");

    let mut messages = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": user}));

    let body = json!({
        "model": model,
        "messages": messages,
        "temperature": cfg.temperature,
        "max_tokens": cfg.max_tokens,
        "stream": true,
    });

    let mut req = client
        .post(&url)
        .timeout(Duration::from_secs(cfg.timeout_secs.max(30) as u64))
        .json(&body);
    for (k, v) in headers(cfg) {
        req = req.header(k, v);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("调用本地模型失败：{}", crate::net::friendly_reqwest_error(&e)))?;

    if !resp.status().is_success() {
        let st = resp.status();
        let t = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "本地模型返回 HTTP {}: {}（请确认模型已加载）",
            st.as_u16(),
            truncate(&t, 200)
        ));
    }

    let mut stream = resp.bytes_stream();
    let mut full = String::new();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("流式读取失败: {}", e))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        // SSE 以 \n 分隔，逐行处理
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let data = match line.strip_prefix("data:") {
                Some(d) => d.trim(),
                None => continue,
            };
            if data == "[DONE]" {
                return Ok(full);
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(delta) = v
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.first())
                    .and_then(|c| c.get("delta"))
                {
                    // 正文增量
                    if let Some(s) = delta.get("content").and_then(|c| c.as_str()) {
                        if !s.is_empty() {
                            full.push_str(s);
                            on_delta(s, DeltaKind::Content);
                        }
                    }
                    // 部分推理模型把思考过程放在 reasoning_content
                    // （不计入返回值，仅作为独立事件推给前端）
                    if let Some(s) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                        if !s.is_empty() {
                            on_delta(s, DeltaKind::Reasoning);
                        }
                    }
                }
            }
        }
    }

    Ok(full)
}

/// 让本地模型生成一个结构化词条（联网词典全部失败时的兜底，需求 2）。
pub async fn generate_entry(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    word: &str,
    lang: &str,
) -> Result<WordEntry> {
    let lang_name = lang_display_name(lang);
    let sys = "你是一个词典数据生成引擎。只输出 JSON，不要输出任何解释、Markdown 代码块标记或多余文字。";
    let user = format!(
        r#"请为{lang_name}单词「{word}」生成词典数据，严格按以下 JSON 结构输出：
{{
  "word": "单词原形",
  "phonetic": {{"uk": "英式音标(带斜杠)", "us": "美式音标(带斜杠)"}},
  "senses": [
    {{"pos": "词性缩写如 n./v./adj.", "definition": "简体中文释义",
      "examples": [{{"text": "英文例句", "translation": "中文翻译"}}]}}
  ],
  "inflections": [{{"label": "变形类型如 过去式/复数/比较级", "form": "变形后的词"}}],
  "related": ["相关词或同义词"],
  "mnemonic": "一句话词根词缀或记忆技巧"
}}
要求：
1. senses 至少 1 项，覆盖该词最常见的 2-3 个义项，按常用度排序。
2. definition 必须是简体中文，简洁准确（不超过 30 字）。
3. 每个义项给 1 个例句。
4. 若该词无变形（如名词不可数），inflections 可为空数组。
5. 若拼写疑似有误，仍按最可能的正确拼写生成。"#,
        lang_name = lang_name,
        word = word
    );

    let raw = chat(client, cfg, sys, &user).await?;
    let json_text = extract_json(&raw);
    let v: Value = serde_json::from_str(&json_text)
        .with_context(|| format!("模型未能输出合法 JSON，原始内容：{}", truncate(&raw, 300)))?;

    Ok(entry_from_json(&v, word, lang))
}

/// 从模型输出里抠出 JSON（应对偶发的 ```json 包裹或前后废话）。
pub fn extract_json(s: &str) -> String {
    let t = s.trim();
    // 去掉 markdown 代码块
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    // 退而求其次：找第一个 { 到最后一个 }
    if let (Some(a), Some(b)) = (t.find('{'), t.rfind('}')) {
        if b > a {
            return t[a..=b].to_string();
        }
    }
    t.to_string()
}

/// 把模型输出的 JSON 转成 WordEntry。
fn entry_from_json(v: &Value, fallback_word: &str, lang: &str) -> WordEntry {
    let mut e = WordEntry::new(fallback_word);
    e.lang = lang.to_string();
    e.source = "lmstudio".to_string();

    if let Some(w) = v.get("word").and_then(|x| x.as_str()) {
        if !w.trim().is_empty() {
            e.word = w.trim().to_string();
        }
    }
    if let Some(p) = v.get("phonetic") {
        e.phonetic.uk = p.get("uk").and_then(|x| x.as_str()).unwrap_or("").to_string();
        e.phonetic.us = p.get("us").and_then(|x| x.as_str()).unwrap_or("").to_string();
    }

    if let Some(arr) = v.get("senses").and_then(|x| x.as_array()) {
        for s in arr {
            let def = match s.get("definition") {
                Some(Value::String(x)) => x.clone(),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|i| i.as_str())
                    .collect::<Vec<_>>()
                    .join("；"),
                _ => String::new(),
            };
            if def.trim().is_empty() {
                continue;
            }
            let mut exs = Vec::new();
            if let Some(el) = s.get("examples").and_then(|x| x.as_array()) {
                for ex in el {
                    let text = if let Some(t) = ex.get("text").and_then(|x| x.as_str()) {
                        t.to_string()
                    } else if let Some(t) = ex.as_str() {
                        t.to_string()
                    } else {
                        String::new()
                    };
                    if !text.trim().is_empty() {
                        exs.push(Example {
                            text,
                            translation: ex
                                .get("translation")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                        });
                    }
                }
            }
            e.senses.push(Sense {
                pos: s
                    .get("pos")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                definition: def,
                examples: exs,
            });
        }
    }

    if let Some(arr) = v.get("inflections").and_then(|x| x.as_array()) {
        for i in arr {
            if let (Some(l), Some(f)) = (
                i.get("label").and_then(|x| x.as_str()),
                i.get("form").and_then(|x| x.as_str()),
            ) {
                if !f.trim().is_empty() {
                    e.inflections.push(crate::models::Inflection {
                        label: l.to_string(),
                        form: f.to_string(),
                    });
                }
            }
        }
    }

    if let Some(arr) = v.get("related").and_then(|x| x.as_array()) {
        for r in arr {
            if let Some(s) = r.as_str() {
                if !s.trim().is_empty() {
                    e.related.push(s.to_string());
                }
            }
        }
        e.related.truncate(15);
    }

    e.mnemonic = v
        .get("mnemonic")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    e
}

/// 构造讲解提示词（需求 5：答错时的完整讲解）。
pub fn explain_prompt(entry: &WordEntry, mode_hint: &str) -> String {
    let senses = entry
        .senses
        .iter()
        .map(|s| format!("{} {}", s.pos, s.definition))
        .collect::<Vec<_>>()
        .join("；");

    if senses.is_empty() {
        format!(
            "请讲解单词「{}」{}。\
             请按：核心含义 → 词根词缀记忆法 → 常见搭配 → 例句与翻译 → 易混词辨析 的顺序讲解。",
            entry.word, mode_hint
        )
    } else {
        format!(
            "请讲解单词「{}」{mode_hint}。\n\
             已知词典释义（供参考，可补充纠正）：{senses}\n\
             请按：核心含义 → 词根词缀记忆法 → 常见搭配与用法区别 → 两个地道例句及翻译 → 易混词辨析 的顺序讲解。",
            entry.word,
            mode_hint = mode_hint,
            senses = senses
        )
    }
}

/// 讲解语言代码 → **目标语言的自称**。
///
/// 为什么不用「日语」「英语」这种中文名：实测本地小模型对
/// 「用 日本語 回答」的遵循度明显高于「用日语回答」—— 后者被当成
/// 「一句关于日语的要求」，前者才是明确的语言标签。
pub fn explain_lang_name(code: &str) -> &'static str {
    match code {
        "zh" => "简体中文",
        "en" => "English",
        "ja" => "日本語",
        "ko" => "한국어",
        "fr" => "Français",
        "de" => "Deutsch",
        "es" => "Español",
        "ru" => "Русский",
        "it" => "Italiano",
        "pt" => "Português",
        _ => "简体中文",
    }
}

/// 把《输出语言》约束追加到 system prompt 末尾。
///
/// 这是**提示词层**的落点：语言要求写进 system，代价为零（没有额外请求），
/// 还能顺带保住 Markdown 结构；小模型不吃这套时再由
/// [`translate_markdown`] 在输出后处理层兜底。
pub fn with_lang_constraint(system_prompt: &str, explain_lang: &str) -> String {
    let lang = explain_lang_name(explain_lang);
    let block = crate::models::EXPLAIN_LANG_CONSTRAINT.replace("{LANG}", lang);
    format!("{}\n\n{}", system_prompt.trim_end(), block)
}

/* ---------------- 输出后处理层：语言检测 + 技术内容保护 + 翻译模板 ---------------- */

fn is_han(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}
fn is_kana(c: char) -> bool {
    ('\u{3040}'..='\u{30ff}').contains(&c)
}
fn is_hangul(c: char) -> bool {
    ('\u{ac00}'..='\u{d7af}').contains(&c)
}
fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04ff}').contains(&c)
}
fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06ff}').contains(&c)
}
fn is_thai(c: char) -> bool {
    ('\u{0e00}'..='\u{0e7f}').contains(&c)
}

/// 判断一段讲解**是否已经是**目标语言，用来决定要不要走后处理翻译兜底。
///
/// 只对「书写系统可区分」的语言做强判定（中/日/韩/俄/阿/泰/希腊）；
/// 英/法/德/西/葡/意 都是拉丁字母，靠字符分布判不准，一律返回 `true`
/// （即不触发兜底）—— 宁可偶尔漏翻，也不能把一段正确的外语讲解
/// 反复丢给模型重译，那是纯粹的浪费与风险。
pub fn text_matches_lang(text: &str, code: &str) -> bool {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    // 太短的文本（「暂无」/「N/A」）比例噪声太大，别据此下判断
    if letters < 12 {
        return true;
    }
    let n = letters as f64;
    let count = |f: fn(char) -> bool| { text.chars().filter(|&c| f(c)).count() as f64 };
    let ratio = |f: fn(char) -> bool| count(f) / n;

    let han = ratio(is_han);
    let kana = ratio(is_kana);
    let hangul = ratio(is_hangul);

    match code {
        // 中文里必然夹着英文单词/例句，汉字阈值不能太高；
        // 但一旦出现成规模的假名/谚文，就说明这是日语/韩语而非中文。
        "zh" => han >= 0.25 && kana < 0.05 && hangul < 0.05,
        // 日语与中文共用汉字，唯一的可靠区分标志是假名（助词、送假名必有）。
        "ja" => kana >= 0.05 && (kana + han) >= 0.25,
        "ko" => hangul >= 0.25,
        "ru" => ratio(is_cyrillic) >= 0.25,
        "ar" => ratio(is_arabic) >= 0.25,
        "th" => ratio(is_thai) >= 0.25,
        "el" => ratio(|c| ('\u{0370}'..='\u{03ff}').contains(&c)) >= 0.25,
        _ => true,
    }
}

/// 占位符包裹符（U+27E6 / U+27E7 数学白方括号）。
///
/// 特意选这两个生僻字符：普通方括号 `[]` 在 Markdown 里太常见
/// （链接、引用、音标），容易被模型顺手改写或与语法混淆。
const SPAN_OPEN: char = '\u{27e6}';
const SPAN_CLOSE: char = '\u{27e7}';

fn push_span(out: &mut String, spans: &mut Vec<String>, seg: &str) {
    let idx = spans.len();
    spans.push(seg.to_string());
    out.push(SPAN_OPEN);
    out.push_str(&idx.to_string());
    out.push(SPAN_CLOSE);
}

fn starts_with_at(buf: &[char], i: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    if i + p.len() > buf.len() {
        return false;
    }
    buf[i..i + p.len()] == p[..]
}

/// 行内技术片段的保护：`` `code` ``、URL、Windows/Unix 路径。
///
/// 只做这几类**形制明确**的片段，不做「看起来像标识符就保护」的激进猜测 ——
/// 那是把中文词也误伤的常见来源。其余的靠提示词约束。
fn protect_inline(line: &str, spans: &mut Vec<String>) -> String {
    let b: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        // 行内代码
        if c == '`' {
            if let Some(j) = b[i + 1..].iter().position(|&x| x == '`') {
                let end = i + 1 + j;
                let seg: String = b[i..=end].iter().collect();
                push_span(&mut out, spans, &seg);
                i = end + 1;
                continue;
            }
        }
        // URL
        if c == 'h' && (starts_with_at(&b, i, "http://") || starts_with_at(&b, i, "https://")) {
            let mut j = i;
            while j < b.len()
                && !b[j].is_whitespace()
                && !matches!(b[j], ')' | '>' | '）' | '，' | '。' | '、' | '"' | '\'')
            {
                j += 1;
            }
            let seg: String = b[i..j].iter().collect();
            push_span(&mut out, spans, &seg);
            i = j;
            continue;
        }
        // Windows 盘符路径（C:\… / D:/…）
        if c.is_ascii_alphabetic()
            && i + 2 < b.len()
            && b[i + 1] == ':'
            && (b[i + 2] == '\\' || b[i + 2] == '/')
        {
            let mut j = i;
            while j < b.len()
                && !b[j].is_whitespace()
                && !matches!(b[j], '`' | ')' | '（' | '，' | '。' | '、' | '"')
            {
                j += 1;
            }
            let seg: String = b[i..j].iter().collect();
            push_span(&mut out, spans, &seg);
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 把讲解正文里的技术片段抽成占位符，返回 `(带占位符的文本, 片段表)`。
///
/// 保护的粒度是「整块代码围栏」和「行内代码 / URL / 路径」：
/// 这些是最容易被模型好心翻译坏的东西（`fn main()` → 「主函数」、
/// `C:\Users\a` → 被拆成句子）。
pub fn protect_technical_spans(text: &str) -> (String, Vec<String>) {
    let mut spans: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    let mut fence = String::new();

    for line in text.split_inclusive('\n') {
        let is_fence_marker = line.trim_start().starts_with("```");
        if in_fence {
            fence.push_str(line);
            if is_fence_marker {
                in_fence = false;
                let block = std::mem::take(&mut fence);
                let kept_nl = block.ends_with('\n');
                push_span(&mut out, &mut spans, block.trim_end_matches('\n'));
                if kept_nl {
                    out.push('\n');
                }
            }
            continue;
        }
        if is_fence_marker {
            in_fence = true;
            fence.clear();
            fence.push_str(line);
            continue;
        }
        out.push_str(&protect_inline(line, &mut spans));
    }
    // 未闭合的围栏：整块保护，别让它泄漏进译文
    if !fence.is_empty() {
        let block = std::mem::take(&mut fence);
        push_span(&mut out, &mut spans, block.trim_end_matches('\n'));
    }
    (out, spans)
}

/// [`protect_technical_spans`] 的逆操作：把占位符换回原文。
///
/// 容错：模型偶尔会把 `⟦12⟧` 写成 `⟦ 12 ⟧` 或漏掉半个符号。
/// 编号越界/残缺时原样保留占位符文本（总比丢内容好）。
pub fn restore_technical_spans(text: &str, spans: &[String]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let b: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == SPAN_OPEN {
            let mut j = i + 1;
            let mut num = String::new();
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == ' ') {
                if b[j].is_ascii_digit() {
                    num.push(b[j]);
                }
                j += 1;
            }
            if !num.is_empty() && j < b.len() && b[j] == SPAN_CLOSE {
                if let Ok(k) = num.parse::<usize>() {
                    if let Some(s) = spans.get(k) {
                        out.push_str(s);
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// 模型有时会把整段答案再套一层 ``` 围栏，这里剥掉。
fn strip_outer_fence(s: &str) -> String {
    let t = s.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let mut lines = t.lines();
    let first = lines.next().unwrap_or("").trim().to_string();
    // 第一行必须是「```」或「```md」这种纯围栏：
    // 若后面还跟着正文（` ``` 这是代码 `），说明不是外层包裹，原样返回。
    let lang_ok = first[3..]
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-' | '_'));
    if !lang_ok {
        return t.to_string();
    }
    let mut body: Vec<&str> = lines.collect();
    if let Some(last) = body.last() {
        if last.trim_start().starts_with("```") {
            body.pop();
        }
    }
    body.join("\n").trim().to_string()
}

/// **输出后处理层的翻译模板**：把整段讲解重译成目标语言。
///
/// 流程：抽出技术片段 → 占位符化 → 套模板让模型翻译 → 回填占位符。
/// 任何一步失败都返回 `Err`，由调用方决定降级（本项目选择保留原文 + 提示）。
pub async fn translate_markdown(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    template: &str,
    text: &str,
    explain_lang: &str,
) -> Result<String> {
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    let (masked, spans) = protect_technical_spans(text);
    let lang = explain_lang_name(explain_lang);
    let tmpl = if template.trim().is_empty() {
        crate::models::DEFAULT_TRANSLATE_TEMPLATE
    } else {
        template
    };
    let user = tmpl.replace("{LANG}", lang).replace("{CONTENT}", &masked);

    // 温度压低：翻译要的是稳定复述，不是创作。
    // max_tokens 至少 2048，防止长讲解的译文被拦腰截断。
    let raw = chat_ex(
        client,
        cfg,
        "",
        &user,
        0.2,
        cfg.max_tokens.max(2048),
    )
    .await?;
    let cleaned = strip_outer_fence(&raw);
    if cleaned.trim().is_empty() {
        return Err(anyhow!("模型返回了空译文"));
    }
    Ok(restore_technical_spans(&cleaned, &spans))
}

/// 语言代码 → 中文名。
pub fn lang_display_name(code: &str) -> &'static str {
    match code {
        "en" => "英语",
        "ja" => "日语",
        "ko" => "韩语",
        "fr" => "法语",
        "de" => "德语",
        "es" => "西班牙语",
        "ru" => "俄语",
        "it" => "意大利语",
        "pt" => "葡萄牙语",
        "ar" => "阿拉伯语",
        "hi" => "印地语",
        _ => "外语",
    }
}

/// 截断长文本用于错误提示。
fn truncate(s: &str, n: usize) -> String {
    let cleaned: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{}…", cleaned)
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_normalization() {
        assert_eq!(
            endpoint("http://127.0.0.1:1234", "/models"),
            "http://127.0.0.1:1234/v1/models"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:1234/v1", "/models"),
            "http://127.0.0.1:1234/v1/models"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:1234/v1/", "/models"),
            "http://127.0.0.1:1234/v1/models"
        );
    }

    #[test]
    fn extract_json_from_codeblock() {
        let s = "好的，以下是结果：\n```json\n{\"word\":\"apple\"}\n```\n希望有帮助";
        let j = extract_json(s);
        assert_eq!(j, "{\"word\":\"apple\"}");
        assert!(serde_json::from_str::<Value>(&j).is_ok());
    }

    #[test]
    fn extract_json_bare() {
        let s = "{\"a\":1}";
        assert_eq!(extract_json(s), "{\"a\":1}");
    }

    #[test]
    fn entry_from_model_json() {
        let v = serde_json::json!({
            "word": "apple",
            "phonetic": {"uk": "/ˈæp.əl/", "us": "/ˈæp.əl/"},
            "senses": [{"pos": "n.", "definition": "苹果",
                "examples": [{"text": "An apple a day.", "translation": "一天一苹果。"}]}],
            "inflections": [{"label": "复数", "form": "apples"}],
            "related": ["fruit"],
            "mnemonic": "a+pple 联想"
        });
        let e = entry_from_json(&v, "apple", "en");
        assert_eq!(e.word, "apple");
        assert_eq!(e.senses.len(), 1);
        assert_eq!(e.senses[0].examples[0].translation, "一天一苹果。");
        assert_eq!(e.inflections[0].form, "apples");
        assert_eq!(e.related, vec!["fruit"]);
    }

    #[test]
    fn entry_from_json_tolerates_array_definition() {
        let v = serde_json::json!({
            "senses": [{"pos": "n.", "definition": ["苹果", "苹果树"]}]
        });
        let e = entry_from_json(&v, "apple", "en");
        assert_eq!(e.senses[0].definition, "苹果；苹果树");
    }

    #[test]
    fn lang_names() {
        assert_eq!(lang_display_name("ja"), "日语");
        assert_eq!(lang_display_name("xx"), "外语");
    }

    #[test]
    fn explain_prompt_includes_senses() {
        let mut e = WordEntry::new("apple");
        e.senses.push(Sense {
            pos: "n.".into(),
            definition: "苹果".into(),
            examples: vec![],
        });
        let p = explain_prompt(&e, "");
        assert!(p.contains("apple"));
        assert!(p.contains("苹果"));
    }

    /* ---- 讲解语言：提示词层 ---- */

    /// 语言约束必须被追加到 system prompt，且指名道姓写清目标语言。
    #[test]
    fn lang_constraint_is_appended() {
        let s = with_lang_constraint("你是词汇老师。", "ja");
        assert!(s.starts_with("你是词汇老师。"));
        assert!(s.contains("【输出语言"));
        assert!(s.contains("日本語"), "要写目标语言的自称：{}", s);
        // 技术内容保原文这条不能丢
        assert!(s.contains("代码标识符") || s.contains("文件路径"));
    }

    /// 未收录的语言代码兜底成中文，不能出现空约束。
    #[test]
    fn lang_constraint_falls_back() {
        let s = with_lang_constraint("x", "xx");
        assert!(s.contains("简体中文"));
    }

    /* ---- 讲解语言：输出后处理层 ---- */

    /// 语言检测：中文讲解里夹英文单词/例句不算「不是中文」。
    #[test]
    fn detect_explanation_language() {
        let zh = "## 核心含义\n- **n.** 苹果；一种落叶乔木的果实，常见于温带地区。\n\n例句：An apple a day keeps the doctor away.";
        assert!(text_matches_lang(zh, "zh"), "含大量汉字应判定为中文");
        assert!(!text_matches_lang(zh, "ja"), "没有假名，不该判成日语");

        // 日中共享汉字，靠假名区分：有假名才算日语，且不再算中文
        let ja = "## 核心意味\n- **n.** りんご。バラ科の落葉高木の果実で、温帯で広く栽培される。\n\n例文：An apple a day keeps the doctor away.";
        assert!(text_matches_lang(ja, "ja"), "有假名应判定为日语");
        assert!(!text_matches_lang(ja, "zh"), "有假名就不该当成中文，否则不会触发翻译");

        let en = "## Core meaning\n- **n.** a round fruit with red or green skin and firm white flesh, growing on a tree.";
        assert!(!text_matches_lang(en, "zh"), "全英文应触发中文兜底翻译");
        assert!(!text_matches_lang(en, "ja"));

        // 拉丁字母语言之间无法可靠区分 → 一律不触发兜底（避免无意义重译）
        assert!(text_matches_lang(en, "fr"));
        assert!(text_matches_lang(en, "de"));

        // 太短的文本不下判断
        assert!(text_matches_lang("暂无", "zh"));
    }

    /// 技术片段必须被抽成占位符，且能一模一样还原。
    #[test]
    fn technical_spans_roundtrip() {
        let src = "运行 `cargo test --lib` 即可。\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\n详见 https://example.com/a?b=1 与 D:\\Projects\\wordwise\\src\\js\\ui.js\n";
        let (masked, spans) = protect_technical_spans(src);
        assert!(!masked.contains("cargo test"), "行内代码应被保护：{}", masked);
        assert!(!masked.contains("fn main"), "代码围栏应被保护：{}", masked);
        assert!(!masked.contains("example.com"), "URL 应被保护：{}", masked);
        assert!(!masked.contains("ui.js"), "Windows 路径应被保护：{}", masked);
        assert!(masked.contains('\u{27e6}'));
        assert_eq!(restore_technical_spans(&masked, &spans), src);
    }

    /// 模型把占位符写成 `⟦ 0 ⟧` 也要能还原；编号越界则原样保留。
    #[test]
    fn restore_tolerates_malformed_placeholders() {
        let spans = vec!["`code`".to_string()];
        assert_eq!(restore_technical_spans("见 ⟦ 0 ⟧ 处", &spans), "见 `code` 处");
        assert_eq!(restore_technical_spans("见 ⟦9⟧ 处", &spans), "见 ⟦9⟧ 处");
    }

    /// 译文被模型套了一层 ``` 时要剥掉（否则前端会把整段当代码块渲染）。
    #[test]
    fn outer_fence_is_stripped() {
        assert_eq!(strip_outer_fence("```md\n## 标题\n正文\n```"), "## 标题\n正文");
        assert_eq!(strip_outer_fence("```\n只有一行\n```"), "只有一行");
        // 正常内容不动
        assert_eq!(strip_outer_fence("## 标题\n正文"), "## 标题\n正文");
        // 首行是「``` + 正文」→ 不是外层包裹
        assert_eq!(strip_outer_fence("``` 这是代码"), "``` 这是代码");
    }

    /// 翻译模板要带上目标语言与正文，且正文里的占位符原样传入。
    #[test]
    fn translate_template_placeholders() {
        let t = crate::models::DEFAULT_TRANSLATE_TEMPLATE;
        assert!(t.contains("{LANG}") && t.contains("{CONTENT}"));
        assert!(t.contains("⟦0⟧"), "模板要显式要求保留占位符");
    }
}
