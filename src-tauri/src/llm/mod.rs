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
}
