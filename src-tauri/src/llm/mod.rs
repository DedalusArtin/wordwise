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
    /// `managed`（本程序托管的本机服务）/ `local`（本机其它服务，如 LM Studio）
    /// / `cloud`（在线 API）。前端据此决定措辞，别把在线 API 说成「本地模型」。
    pub endpoint_kind: String,
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

/// 基础地址的最后一段是不是版本号（`v1` / `v4` / `v1beta` …）。
///
/// 为什么不能只判 `/v1`：在线 API 的版本号五花八门 —— 智谱 GLM 是
/// `/api/paas/v4`。只认 `/v1` 的话会被拼成 `/api/paas/v4/v1/chat/completions`，
/// 服务端直接 404，而用户看到的只是「连不上」，根本想不到是地址被改坏了。
fn has_version_suffix(base: &str) -> bool {
    let Some(last) = base.rsplit('/').find(|s| !s.is_empty()) else {
        return false;
    };
    let Some(rest) = last.strip_prefix('v') else {
        return false;
    };
    // 数字开头即可：v1、v4、v1beta 都算；像 "vector" 这种不算
    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// 拼接接口地址，容忍用户填 `http://host:1234` 或带 `/v1` 或带完整路径。
pub fn endpoint(base: &str, path: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    let p = path.trim_start_matches('/');
    if b.ends_with(p) {
        return b.to_string();
    }
    if has_version_suffix(b) {
        format!("{}/{}", b, p)
    } else {
        format!("{}/v1/{}", b, p)
    }
}

/// 当前 AI 走的是哪一类服务。
///
/// 三态而不是「本地/在线」两态：本机 LM Studio 和「一键部署」托管的服务
/// 虽然都在 127.0.0.1，但排查话术完全不同（一个要用户去 LM Studio 点
/// Start Server，另一个由本程序自己管启停）。
pub fn endpoint_kind(base: &str) -> &'static str {
    if crate::localllm::is_managed_base(base) {
        return "managed";
    }
    let lower = base.to_ascii_lowercase();
    if lower.contains("127.0.0.1") || lower.contains("localhost") || lower.contains("[::1]") {
        return "local";
    }
    "cloud"
}

/// 检查服务是否在线，并列出可用模型。
///
/// 本机服务和在线 API 共用一套 OpenAI 兼容协议，但**排查话术必须分开**：
/// 前者连不上就该去找本地服务，后者连不上多半是 Key、代理或地址的问题。
/// 一律说「请确认 LM Studio 已启动」会让用在线 API 的人彻底摸不着头脑。
pub async fn status(client: &reqwest::Client, cfg: &LlmConfig) -> LlmStatus {
    let url = endpoint(&cfg.base_url, "/models");
    let kind = endpoint_kind(&cfg.base_url);
    let is_local = kind != "cloud";
    let host = crate::net::host_of(&cfg.base_url);

    let mut req = client.get(&url).timeout(Duration::from_secs(6));
    for (k, v) in headers(cfg) {
        req = req.header(k, v);
    }

    let fail = |msg: String| LlmStatus {
        online: false,
        base_url: cfg.base_url.clone(),
        models: vec![],
        active_model: String::new(),
        message: msg,
        endpoint_kind: kind.to_string(),
    };

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

                // 本机服务：用户填的模型如果没被加载，退回第一个可用的是帮忙；
                // 在线 API：服务返回的清单未必含用户想用的模型（有的还只给
                // 一部分），此时**必须尊重用户填的**，不然他填了也不生效。
                let active = if !cfg.model.is_empty() && (!is_local || models.contains(&cfg.model)) {
                    cfg.model.clone()
                } else {
                    models.first().cloned().unwrap_or_default()
                };

                let msg = if models.is_empty() {
                    if is_local {
                        "已连接，但尚未加载任何模型。请先加载一个模型。".to_string()
                    } else {
                        format!("已连接 {host}，但服务没返回模型清单，请在下面手动填模型名。")
                    }
                } else {
                    format!("已连接 {host}，共 {} 个模型可用", models.len())
                };

                LlmStatus {
                    online: true,
                    base_url: cfg.base_url.clone(),
                    models,
                    active_model: active,
                    message: msg,
                    endpoint_kind: kind.to_string(),
                }
            }
            Err(e) => fail(format!("响应解析失败：{e}（{host} 可能不是 OpenAI 兼容接口）")),
        },
        Ok(r) => {
            let code = r.status().as_u16();
            let msg = match code {
                401 | 403 => {
                    if cfg.api_key.trim().is_empty() {
                        format!("{host} 需要 API Key，请在下面「API Key」里填上")
                    } else {
                        format!("{host} 拒绝了这次请求（HTTP {code}）：API Key 无效、过期或没权限")
                    }
                }
                404 => {
                    if is_local {
                        format!("{host} 返回 404：确认地址对不对（一般要带 /v1）")
                    } else {
                        format!("{host} 返回 404：这个地址可能不支持 /models 接口，可直接填模型名再试")
                    }
                }
                429 => format!("{host} 返回 429：请求太频繁被限流了，过一会儿再试"),
                _ => format!("{host} 返回 HTTP {code}"),
            };
            fail(msg)
        }
        Err(e) => {
            let friendly = crate::net::friendly_reqwest_error(&e);
            let msg = if is_local {
                format!("无法连接 {}（{friendly}）。本地服务要先启动。", cfg.base_url)
            } else {
                format!(
                    "无法连接 {}（{friendly}）。检查网络；需要代理的话到「设置 → 网络与代理」里打开。",
                    cfg.base_url
                )
            };
            fail(msg)
        }
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

/// 从一条 `/chat/completions` 响应里取出 `(正文, 思考过程)`。
///
/// 两者必须分开：推理模型（Qwen3、DeepSeek-R1、QwQ…）会先输出一大段
/// `reasoning_content`，那**不是答案**。
fn split_content_and_reasoning(v: &Value) -> (String, String) {
    let msg = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"));

    let pick = |key: &str| -> String {
        msg.and_then(|m| m.get(key))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let content = pick("content");
    // 有的服务端把思考过程塞在 content 里、用标签包起来（DeepSeek 的
    // ` thinking…<｜end▁of▁thinking｜>`、部分 Qwen 部署的 `Thinking Process:`），必须剥掉，
    // 否则用户会在讲解/译文里看到一整段英文思考。
    let (content, inline_reason) = strip_thinking_tags(&content);
    let mut reasoning = pick("reasoning_content");
    reasoning.push_str(&inline_reason);
    (content, reasoning)
}

/// 剥掉正文里内联的思考过程标签，返回 `(纯正文, 被剥掉的思考内容)`。
fn strip_thinking_tags(s: &str) -> (String, String) {
    const PAIRS: [(&str, &str); 3] = [
        (" thinking", "<｜end▁of▁thinking｜>"),
        ("<thinking>", "</thinking>"),
        ("<reasoning>", "</reasoning>"),
    ];
    let mut out = s.to_string();
    let mut dropped = String::new();
    for (open, close) in PAIRS {
        while let Some(i) = out.find(open) {
            let Some(j) = out[i..].find(close) else {
                // 没有闭合标签：截断到开头（后面全是没写完的思考）
                dropped.push_str(&out[i..]);
                out.truncate(i);
                break;
            };
            let end = i + j + close.len();
            dropped.push_str(&out[i..end]);
            out.replace_range(i..end, "");
        }
    }
    (out.trim().to_string(), dropped)
}

/// 同上，但可覆盖采样参数。
///
/// ★ 两个必须守住的点（都踩过坑）：
///
/// 1. **绝不把 `reasoning_content` 当正文返回。** 老代码在 `content` 为空时
///    会把思考过程原样返回，于是「自动翻译成日语」的面板里出现了
///    `Thinking Process: 1. Analyze the Request: Role: Professional Translator…`
///    —— 那是翻译**模板本身**被模型复述了一遍，根本不是译文。
///    内容为空就该报错，让上层走「保留原文 + 提示」的降级路径。
///
/// 2. **主动关闭推理模型的思考过程。** 非流式调用拿不到 `reasoning_content`
///    的前端展示（本项目只在流式讲解里用），开着思考纯粹是浪费时间与
///    `max_tokens` 预算：模型可能把预算全花在思考上，正文还没开始就被截断。
///    LM Studio 对支持该模板变量的模型识别
///    `chat_template_kwargs.enable_thinking`；不识别时会报 4xx/5xx，
///    这里自动摘掉该字段重试一次，保证兼容性。
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

    // 第一次带关闭思考的模板变量；服务端不认就摘掉重试
    let mut with_no_think = true;
    loop {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });
        if with_no_think {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }

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
            // 服务端不认 chat_template_kwargs → 摘掉重试一次
            if with_no_think {
                with_no_think = false;
                continue;
            }
            // LM Studio 在模型未加载时返回的错误信息很有用，直接透传
            let hint = if text.contains("model") || st.as_u16() == 404 {
                "（请确认 LM Studio 中已加载模型，且模型名与配置一致）"
            } else {
                ""
            };
            return Err(anyhow!(
                "本地模型返回 HTTP {} {}: {}",
                st.as_u16(),
                hint,
                truncate(&text, 300)
            ));
        }

        let v: Value = serde_json::from_str(&text).context("模型响应不是合法 JSON")?;
        let (content, reasoning) = split_content_and_reasoning(&v);

        if content.trim().is_empty() {
            if !reasoning.trim().is_empty() {
                return Err(anyhow!(
                    "模型只输出了思考过程、没有输出正文（思考 {} 字）。\
                     请在 LM Studio 中关闭该模型的推理模式，或换用非推理模型。\
                     思考片段：{}",
                    reasoning.chars().count(),
                    truncate(reasoning.trim(), 120)
                ));
            }
            return Err(anyhow!("模型返回内容为空，请检查模型是否正常加载"));
        }
        return Ok(content);
    }
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
  "inflections": [{{"label": "变形类型(中文)，如 复数/过去式/过去分词/现在分词/第三人称单数/比较级/最高级", "form": "变形后的词"}}],
  "related": ["相关词或同义词"],
  "mnemonic": "一句话词根词缀或记忆技巧"
}}
要求：
1. senses 至少 1 项，覆盖该词最常见的 2-3 个义项，按常用度排序。
2. definition 必须是简体中文，简洁准确（不超过 30 字）。
3. 每个义项给 1 个例句。
4. **inflections 按该词的词性把变形给全**（用户要求「动名形容时态单复数变形要表明」）：
   - 动词 → 过去式、过去分词、现在分词、第三人称单数；
   - 名词（可数）→ 复数（不可数名词不编复数）；
   - 形容词/副词 → 比较级、最高级（没有则省略）。
   每条 label 用**中文**写清变形类型，form 用正确形式；没有的变形才省略，不要编。
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

/// 把一段**已有的 AI 讲解**收敛成结构化词条（「讲解并入词库」用）。
///
/// 为什么不复用 [`generate_entry`]：那会抛开讲解、让模型重新凭记忆编一遍，
/// 既浪费算力，又可能和用户刚才看到的讲解对不上。这里把讲解正文当作唯一
/// 事实来源，只做「格式转换」，所以同一段讲解转出来的词条是稳定的。
///
/// 讲解是 Markdown、结构松散，因此要求模型**只搬运不发挥**：正文里没写的
/// 信息（如音标）宁可留空，也不要编。
pub async fn entry_from_explain(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    word: &str,
    lang: &str,
    markdown: &str,
) -> Result<WordEntry> {
    let lang_name = lang_display_name(lang);
    let sys = "你是一个词典数据整理引擎。只输出 JSON，不要输出任何解释、Markdown 代码块标记或多余文字。";
    let user = format!(
        r#"下面是一段关于{lang_name}单词「{word}」的讲解正文。请把它整理成词典数据，严格按以下 JSON 结构输出：
{{
  "word": "单词原形",
  "phonetic": {{"uk": "英式音标(带斜杠，讲解里没写就留空字符串)", "us": "美式音标(同上)"}},
  "senses": [
    {{"pos": "词性缩写如 n./v./adj.", "definition": "简体中文释义",
      "examples": [{{"text": "英文例句", "translation": "中文翻译"}}]}}
  ],
  "inflections": [{{"label": "变形类型如 过去式/复数/比较级", "form": "变形后的词"}}],
  "related": ["相关词或同义词"],
  "mnemonic": "一句话词根词缀或记忆技巧"
}}
要求：
1. **只搬运讲解正文里已经出现的信息**，不要自己发挥、不要补充讲解里没有的义项。
2. senses 至少 1 项；若讲解里确实没有可用的释义，就基于该词给出最基础的一条。
3. definition 必须是简体中文，简洁准确（不超过 30 字）。
4. 讲解里没有的字段一律留空字符串或空数组，**不要编造**。

=== 讲解正文开始 ===
{markdown}
=== 讲解正文结束 ==="#,
        lang_name = lang_name,
        word = word,
        markdown = truncate(markdown, 6000)
    );

    let raw = chat(client, cfg, sys, &user).await?;
    let json_text = extract_json(&raw);
    let v: Value = serde_json::from_str(&json_text)
        .with_context(|| format!("模型未能输出合法 JSON，原始内容：{}", truncate(&raw, 300)))?;

    let mut e = entry_from_json(&v, word, lang);
    // 释义全空说明模型没搬好，这种情况下让调用方知道（返回仍可用，但会被上层丢弃）
    e.source = "explain-store".to_string();
    Ok(e)
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

/// 把联网检索到的资料拼进讲解提示词（需求 5）。
///
/// 入参是 `(标题, 摘要)` 的切片，刻意**不**依赖 `search` 模块的类型 ——
/// 「怎么格式化一份参考资料」是提示词工程的事，与「怎么抓搜索结果」无关，
/// 分开之后这个函数就是纯函数，可以直接写单测。
///
/// 摘要里的空白会被压平：搜索引擎返回的 snippet 常带换行与制表符，
/// 直接贴进去会让模型把一行资料读成多行，进而把半句话当成一个独立要点。
pub fn with_web_refs(prompt: &str, refs: &[(String, String)]) -> String {
    // 过滤掉没标题也没摘要的噪声项；这类条目只会在提示词里占位
    let items: Vec<(String, String)> = refs
        .iter()
        .filter(|(t, s)| !t.trim().is_empty() || !s.trim().is_empty())
        .map(|(t, s)| (flatten_ws(t), flatten_ws(s)))
        .collect();
    if items.is_empty() {
        return prompt.to_string();
    }

    let mut block = String::from("\n\n【联网检索资料（补充例句/变形的唯一依据，见 system 约束）】\n");
    for (i, (title, snippet)) in items.iter().enumerate() {
        block.push_str(&format!("{}. 标题：{}\n", i + 1, title));
        if !snippet.is_empty() {
            block.push_str(&format!("   摘要：{}\n", snippet));
        }
    }
    format!("{}{}", prompt.trim_end(), block)
}

/// 把一段文本里的换行/制表符压成单个空格，并丢掉首尾空白。
fn flatten_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 构造讲解提示词（需求 5：答错时的完整讲解）。
///
/// ★ 正确性约束（用户反馈「讲解了根本不存在的词，有道上查没有」后加的）：
///   旧版 senses 分支写的是「供参考，**可补充纠正**」—— 这四个字直接
///   纵容模型推翻词典：grievaunce 的词典释义明明是
///   「Obsolete form of grievance」，模型却"纠正"成 grieve 的 n/v/adj
///   三词性，词源、搭配、例句整段编造。现在词典释义是**唯一事实来源**，
///   并逐条约束词性/词源；词是变体时（释义指向另一个词）要求开门见山。
pub fn explain_prompt(entry: &WordEntry, mode_hint: &str) -> String {
    let senses = entry
        .senses
        .iter()
        .map(|s| format!("{} {}", s.pos, s.definition))
        .collect::<Vec<_>>()
        .join("；");

    // 词典把该词标注为另一个词的变体/派生（"Obsolete form of grievance"、
    // "plural of child"…）→ 讲解必须开门见山说明这层关系
    let variant_line = match detect_variant_of(entry) {
        Some(v) => format!(
            "3. 词典将「{word}」标注为「{v}」的变体/派生形式 —— **第一句就说明这层关系**，并把讲解重心放在 {v} 上；\n",
            word = entry.word,
            v = v
        ),
        None => String::new(),
    };

    if senses.is_empty() {
        format!(
            "请讲解单词「{word}」{mode_hint}。\n\
             ★ 你没有任何词典释义，正确性要求（必须遵守）：\n             1. 若你认为这不是标准词/是变体或罕见拼写，**第一段就直接说明**并给出规范形式；\n             2. 词性与词义只讲你有把握的，把握不足就注明「未经词典收录」；\n             3. 词源没有把握时**不得编造** —— 只讲可验证的构词拆分，拆不出来就写「暂无可靠的词根信息」；\n             请按：核心含义 → 词根词缀记忆法 → 常见搭配 → 例句与翻译 → 易混词辨析 的顺序讲解。",
            word = entry.word,
            mode_hint = mode_hint
        )
    } else {
        format!(
            "请讲解单词「{word}」{mode_hint}。\n\
             词典释义（**唯一事实来源，讲解必须与之一致**）：{senses}\n\
             ★ 正确性要求（必须遵守）：\n             1. 核心含义的**词性与词义只能来自上面的词典释义** —— 不得添加词典没有的词性，\
                不得给出与之矛盾的释义；\n             2. 词源没有把握时**不得编造** —— 写「暂无定论」，或只做构词拆分并注明是联想记忆；\n             {variant_line}\
             请按：核心含义 → 词根词缀记忆法 → 常见搭配与用法区别 → 两个地道例句及翻译 → 易混词辨析 的顺序讲解。",
            word = entry.word,
            mode_hint = mode_hint,
            senses = senses,
            variant_line = variant_line
        )
    }
}

/// 词典释义把该词指向另一个词（变体/派生/复数/古体）时，提取被指向的词。
///
/// 匹配英文释义里的固定说法：`Obsolete form of grievance`、
/// `plural of child`、`misspelling of …`、`variant of …`。
fn detect_variant_of(entry: &WordEntry) -> Option<String> {
    const PATTERNS: [&str; 5] = [
        "form of ",
        "variant of ",
        "misspelling of ",
        "alternative spelling of ",
        "plural of ",
    ];
    for s in &entry.senses {
        let d = s.definition.to_lowercase();
        for p in PATTERNS {
            if let Some(idx) = d.find(p) {
                let rest = &d[idx + p.len()..];
                let w: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic() || *c == '-' || *c == '\'')
                    .collect();
                if w.chars().count() >= 2 {
                    return Some(w);
                }
            }
        }
    }
    None
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

/// 把《联网资料·用法》约束追加到 system prompt 末尾（需求 5）。
///
/// 与 [`with_lang_constraint`] 分开是刻意的：**只有真的抓到资料时才注入**。
/// 没有资料却要求模型「只能依据资料补充例句」，它会连一句例句都写不出来 ——
/// 那是把一个加分项变成了减分项。
pub fn with_web_ref_constraint(system_prompt: &str, explain_lang: &str) -> String {
    let lang = explain_lang_name(explain_lang);
    let block = crate::models::EXPLAIN_WEB_REF_CONSTRAINT.replace("{LANG}", lang);
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
pub(crate) fn strip_outer_fence(s: &str) -> String {
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
    // max_tokens 至少 4096，防止长讲解的译文被拦腰截断。
    let raw = chat_ex(client, cfg, TRANSLATE_SYSTEM, &user, 0.2, cfg.max_tokens.max(4096))
        .await?;
    let cleaned = strip_outer_fence(&raw);

    // 最后一道闸：模型把翻译规则本身复述回来了，那这次翻译就是失败的。
    // 宁可让上层降级为「保留原文 + 提示」，也不能把模板当译文展示给用户。
    if let Some(why) = looks_like_rule_echo(&cleaned) {
        return Err(anyhow!("模型没有输出译文，而是复述了翻译规则（{}）", why));
    }
    if cleaned.trim().is_empty() {
        return Err(anyhow!("模型返回了空译文"));
    }
    Ok(restore_technical_spans(&cleaned, &spans))
}

/// 翻译请求的 system 提示词：把「规则」和「待翻译内容」彻底分开。
///
/// 之前把整段模板塞进 **user** 消息（system 留空），模型很容易把模板
/// 当成一个「需要分析的任务」—— 于是吐出一段
/// `Thinking Process: 1. Analyze the Request: Role: Professional Translator…`
/// 的思考过程。放进 system 并明确「不得复述规则」，这类跑偏会少很多。
const TRANSLATE_SYSTEM: &str = "你是一个翻译引擎。严格按用户消息中给出的规则输出译文。\
不得输出任何分析、思考过程（如 Thinking Process、Analyze the Request、Let me think）、\
前言、结语、解释，也不得复述或总结收到的规则本身。只输出译文。";

/// 判断模型返回的是不是「把翻译规则复述了一遍」。
///
/// 只在**开头**一段里找特征短语：正文里偶尔出现 "constraint" 之类的词
/// 是正常的，但译文**以** "Thinking Process:" 开头就一定是跑偏了。
fn looks_like_rule_echo(s: &str) -> Option<&'static str> {
    const HEADS: [(&str, &str); 6] = [
        ("Thinking Process", "思考过程标题"),
        ("Analyze the Request", "分析请求"),
        ("Let me think", "自述思考"),
        ("Role: Professional Translator", "复述角色设定"),
        ("Markdown Layout Engineer", "复述角色设定"),
        ("Constraints:", "复述规则清单"),
    ];
    // 只看前 400 个字符，避免正文深处的正常用词被误判
    let head: String = s.chars().take(400).collect();
    for (needle, why) in HEADS {
        if head.contains(needle) {
            return Some(why);
        }
    }
    None
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
    fn web_refs_block_absent_when_no_refs() {
        // 没抓到资料时必须**原样**返回：多一个空标题的资料块会让模型
        // 以为「有联网资料可用」却一条也找不到，反而更容易编。
        let p = "请讲解单词「happy」。";
        assert_eq!(with_web_refs(p, &[]), p);
        // 全是空白的条目等同于没有资料
        let blank = vec![("  ".to_string(), "\n\t".to_string())];
        assert_eq!(with_web_refs(p, &blank), p);
    }

    #[test]
    fn web_refs_block_formats_and_flattens() {
        let refs = vec![
            ("  happiness 的用法 ".to_string(), "派生名词：\n  happiness  n.\t快乐".to_string()),
            ("只用标题的条目".to_string(), String::new()),
        ];
        let out = with_web_refs("请讲解单词「happy」。", &refs);
        // 原文保留在开头
        assert!(out.starts_with("请讲解单词「happy」。"));
        assert!(out.contains("【联网检索资料"));
        // 编号 + 标题
        assert!(out.contains("1. 标题：happiness 的用法"));
        // 摘要里的换行/制表符被压成空格，模型不会把半句当独立要点
        assert!(out.contains("摘要：派生名词： happiness n. 快乐"));
        // 没摘要的条目只写标题，不出现空的「摘要：」行
        assert!(out.contains("2. 标题：只用标题的条目"));
        assert!(!out.contains("2. 标题：只用标题的条目\n   摘要：\n"));
    }

    #[test]
    fn web_ref_constraint_mentions_lang_placeholder() {
        // 约束块要能被 with_lang_constraint 的替换流程处理（含 {LANG}）
        assert!(crate::models::EXPLAIN_WEB_REF_CONSTRAINT.contains("{LANG}"));
        assert!(crate::models::EXPLAIN_WEB_REF_CONSTRAINT.contains("编造"));
    }

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
    fn endpoint_keeps_non_v1_version_suffix() {
        // 智谱 GLM 的 base 是 /api/paas/v4。只认 /v1 的老写法会拼成
        // /api/paas/v4/v1/chat/completions —— 服务端 404，用户只看到「连不上」。
        assert_eq!(
            endpoint("https://open.bigmodel.cn/api/paas/v4", "/chat/completions"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            endpoint("https://open.bigmodel.cn/api/paas/v4/", "/models"),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );
        // v1beta 也要认（部分服务用它）
        assert_eq!(
            endpoint("https://example.com/v1beta", "/models"),
            "https://example.com/v1beta/models"
        );
        // 不是版本号的末段不能被误判：这个词以 v 开头但不是版本
        assert_eq!(
            endpoint("https://example.com/vector", "/models"),
            "https://example.com/vector/v1/models"
        );
        // 已经带了完整路径就原样返回，别再拼一层
        assert_eq!(
            endpoint("https://api.deepseek.com/v1/chat/completions", "/chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_kind_splits_managed_local_cloud() {
        use crate::localllm::{MANAGED_PORT_MAX, MANAGED_PORT_MIN};
        // 本程序托管的服务：端口段与 pick_port 同源
        assert_eq!(endpoint_kind("http://127.0.0.1:18080/v1"), "managed");
        assert_eq!(
            endpoint_kind(&format!("http://127.0.0.1:{}/v1", MANAGED_PORT_MIN)),
            "managed"
        );
        assert_eq!(
            endpoint_kind(&format!("http://127.0.0.1:{}/v1", MANAGED_PORT_MAX - 1)),
            "managed"
        );
        // 本机但不是我们托管的 → LM Studio 之类
        assert_eq!(endpoint_kind("http://127.0.0.1:1234/v1"), "local");
        assert_eq!(endpoint_kind("http://localhost:1234/v1"), "local");
        // 在线 API
        assert_eq!(endpoint_kind("https://api.deepseek.com/v1"), "cloud");
        assert_eq!(endpoint_kind("https://api.openai.com/v1"), "cloud");
        // 端口段的边界：18180 不属于托管
        assert_eq!(endpoint_kind("http://127.0.0.1:18180/v1"), "local");
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

    /// ★ 正确性闸（用户反馈「讲解了有道上根本没有的词」）：
    /// 词典释义必须是唯一事实来源，词是变体时要注入「第一句说明关系」。
    #[test]
    fn explain_prompt_anchors_dict_and_flags_variants() {
        // 有释义的词：不得再出现「可补充纠正」这种纵容措辞
        let mut e = WordEntry::new("apple");
        e.senses.push(Sense {
            pos: "n.".into(),
            definition: "苹果".into(),
            examples: vec![],
        });
        let p = explain_prompt(&e, "");
        assert!(p.contains("唯一事实来源"), "释义必须声明为唯一事实来源");
        assert!(!p.contains("可补充纠正"), "「可补充纠正」的幻觉口子必须关掉");
        assert!(p.contains("不得添加词典没有的词性"));

        // 变体词：释义指向另一个词 → 注入「第一句说明关系」的约束
        let mut v = WordEntry::new("grievaunce");
        v.senses.push(Sense {
            pos: String::new(),
            definition: "Obsolete form of grievance.".into(),
            examples: vec![],
        });
        let pv = explain_prompt(&v, "");
        assert!(pv.contains("grievance"), "要注入被指向的词");
        assert!(pv.contains("第一句就说明这层关系"), "变体关系必须开门见山");
        assert_eq!(detect_variant_of(&v).as_deref(), Some("grievance"));

        // 无释义的词：禁止编造词源
        let empty = WordEntry::new("zzzqqq");
        let pe = explain_prompt(&empty, "");
        assert!(pe.contains("不得编造"), "无释义时词源不得编造");
        assert!(pe.contains("没有任何词典释义"));
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

    /* ---- 思考过程绝不能当正文（截图里「译文」是一段 Thinking Process） ---- */

    /// `content` 为空时，绝不能把 `reasoning_content` 当正文返回。
    ///
    /// 这是「自动翻译成日语，结果面板里是 Role: Professional Translator…」的根因。
    #[test]
    fn reasoning_is_never_returned_as_content() {
        let v: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"",
                "reasoning_content":"Thinking Process:\n1. Analyze the Request: Role: Professional Translator"}}]}"#,
        )
        .unwrap();
        let (content, reasoning) = split_content_and_reasoning(&v);
        assert!(content.is_empty(), "正文必须是空的，不能顶替成思考过程");
        assert!(reasoning.contains("Thinking Process"), "思考过程应单独取出");
    }

    /// 正文与思考过程同时存在时，只取正文。
    #[test]
    fn content_wins_over_reasoning() {
        let v: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"こんにちは","reasoning_content":"嗯，需要翻译成日语…"}}]}"#,
        )
        .unwrap();
        let (content, reasoning) = split_content_and_reasoning(&v);
        assert_eq!(content, "こんにちは");
        assert!(reasoning.contains("日语"));
    }

    /// 有的部署把思考过程内联在 content 里、用标签包住，必须剥掉。
    #[test]
    fn inline_thinking_tags_are_stripped() {
        let v: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"<thinking>让我想想怎么翻</thinking>你好，世界"}}]}"#,
        )
        .unwrap();
        let (content, reasoning) = split_content_and_reasoning(&v);
        assert_eq!(content, "你好，世界", "标签内的思考必须剥掉");
        assert!(reasoning.contains("让我想想"));

        // 没闭合的标签：后面全是没写完的思考，整体丢弃
        let (c2, r2) = strip_thinking_tags("正文在  thinking但这里没闭合");
        assert_eq!(c2, "正文在");
        assert!(r2.contains("没闭合"));
    }

    /// 模型把翻译规则复述回来时，必须判定为失败（走降级），不能当译文展示。
    #[test]
    fn rule_echo_is_detected() {
        let bad = "Thinking Process:\n\n1. Analyze the Request:\n   - Role: Professional Translator & Markdown Layout Engineer.\n   - Task: Translate a given Chinese text into Japanese.";
        assert!(looks_like_rule_echo(bad).is_some());

        assert!(looks_like_rule_echo("Constraints:\n1. Output ONLY the translation").is_some());
        assert!(looks_like_rule_echo("Let me think about this...").is_some());

        // 正常译文不能误判
        assert!(looks_like_rule_echo("## 核心含义\n- **感叹词** 你好，用于问候。").is_none());
        assert!(looks_like_rule_echo("こんにちは").is_none());
        // 特征词出现在正文深处（400 字之后）不算
        let deep = format!("{}Thinking Process", "あ".repeat(500));
        assert!(looks_like_rule_echo(&deep).is_none());
    }
}
