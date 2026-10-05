//! 词典解析与多源聚合（需求 2 & 6）。
//!
//! 核心流程：
//! 1. 按优先级依次请求启用的词典源
//! 2. 用 `FieldMapping` 把任意 JSON 结构归一化成 `WordEntry`
//! 3. 多源结果合并（merge_from），谁先给出有效字段就用谁的
//! 4. 全失败时交给上层用本地大模型兜底
//!
//! 这样「新增语言」= 加一条配置，不需要改 Rust 代码。

use crate::db::Db;
use crate::models::{DictSourceConfig, Example, Inflection, Sense, WordEntry};
use crate::net;
use anyhow::Result;
use std::collections::BTreeMap;

// 子模块：内置词典源定义 + 通用 JSON 路径取值器 + 规则变形兜底 + 词库导入
pub mod builtin;
pub mod importer;
pub mod inflect;
pub mod jsonpath;

/// 一个源的单次查询结果。
#[derive(Debug, Clone)]
pub struct SourceHit {
    pub source_id: String,
    pub source_name: String,
    pub entry: WordEntry,
    pub ok: bool,
    pub error: String,
    pub elapsed_ms: i64,
}

/// 聚合查询的完整结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LookupResult {
    pub word: String,
    pub lang: String,
    pub entry: WordEntry,
    /// 命中的源名列表
    pub sources: Vec<String>,
    /// 是否来自缓存
    pub from_cache: bool,
    /// 是否由本地大模型兜底生成
    pub from_llm: bool,
    /// 各源的调试信息
    pub trace: Vec<SourceTrace>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceTrace {
    pub source: String,
    pub ok: bool,
    pub error: String,
    pub elapsed_ms: i64,
}

/// 按字符所属书写系统猜测查询词的语言。
///
/// 为什么需要它：界面上的语言下拉只能表达「我打算查哪种语言」，
/// 而用户实际输入什么字符串是自由的。之前代码无条件
/// `entry.lang = 查询语言`，于是选「日语」查「开心」时，
/// 一个中文词会被打上「日语」标签、还去问日语词典（拿到英文释义）
/// —— 这就是「多语言适配有问题」的直接来源。
///
/// 判定规则（按书写系统的**排他性**排序，先命中先返回）：
/// - 假名（平/片假名）→ 日语：假名只用于日语，是决定性证据；
/// - 谚文 → 韩语；
/// - 汉字且**无假名** → 中文：纯汉字串更可能是中文；
/// - 西里尔 → 俄语；希腊 → 希腊语；泰文 / 阿拉伯文同理；
/// - 纯拉丁字母 → `None`（英/法/德/西…彼此无法区分），交回用户选择。
///
/// 返回 `None` 表示「判不出来，用用户选的」。
pub fn detect_lang(word: &str) -> Option<&'static str> {
    let mut kana = false;
    let mut hangul = false;
    let mut han = false;
    let mut cyrillic = false;
    let mut greek = false;
    let mut thai = false;
    let mut arabic = false;
    let mut latin = false;

    for c in word.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana = true,   // 平假名/片假名/片假名扩展
            0x1100..=0x11FF | 0xAC00..=0xD7AF => hangul = true, // 谚文字母 + 音节
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han = true,
            0x0400..=0x04FF => cyrillic = true,
            0x0370..=0x03FF => greek = true,
            0x0E00..=0x0E7F => thai = true,
            0x0600..=0x06FF | 0x0750..=0x077F => arabic = true,
            _ => {
                if c.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&(c as u32)) {
                    latin = true; // 含带变音符的拉丁字母（法/德/西/葡…）
                }
            }
        }
    }

    if kana {
        return Some("ja");
    }
    if hangul {
        return Some("ko");
    }
    if han {
        return Some("zh");
    }
    if cyrillic {
        return Some("ru");
    }
    if greek {
        return Some("el");
    }
    if thai {
        return Some("th");
    }
    if arabic {
        return Some("ar");
    }
    if latin {
        return None; // 交回用户选择
    }
    None
}

/// 渲染 URL 模板。
fn render_url(tpl: &str, word: &str, lang: &str, key: &str) -> String {
    tpl.replace("{word}", &urlencoding::encode(word))
        .replace("{lang}", lang)
        .replace("{key}", key)
}

/// 把一个源的原始 JSON 归一化成 WordEntry。
pub fn normalize(raw: &serde_json::Value, cfg: &DictSourceConfig, word: &str, lang: &str) -> WordEntry {
    let m = &cfg.mapping;
    let mut e = WordEntry::new(word);
    e.lang = lang.to_string();
    e.source = cfg.id.clone();

    // 词条本身：优先映射，取不到就用查询词
    let mapped_word = crate::dict::jsonpath::query_str(raw, &m.word);
    if !mapped_word.is_empty() {
        e.word = mapped_word;
    }

    // 音标：可能命中数组，过滤空值后取第一个/全部
    let uks = crate::dict::jsonpath::query_list(raw, &m.phonetic_uk);
    let uss = crate::dict::jsonpath::query_list(raw, &m.phonetic_us);
    e.phonetic.uk = uks
        .iter()
        .find(|s| !s.trim().is_empty() && s.contains('/'))
        .or_else(|| uks.iter().find(|s| !s.trim().is_empty()))
        .cloned()
        .unwrap_or_default();
    e.phonetic.us = uss
        .iter()
        .find(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_default();
    if e.phonetic.uk.is_empty() {
        e.phonetic.uk = e.phonetic.us.clone();
    }

    let audios = crate::dict::jsonpath::query_list(raw, &m.audio);
    e.phonetic.audio = audios
        .iter()
        .find(|s| s.starts_with("http"))
        .cloned()
        .unwrap_or_default();

    // 义项
    let objs = crate::dict::jsonpath::query_objects(raw, &m.senses);
    for o in objs {
        let pos = crate::dict::jsonpath::query_text(o, &m.sense_pos);
        // 用 query_text 而不是 query_str：有道的释义常常是数组
        // （`def: ["心情愉快；高兴"]` / `i: ["", {"#text": "..."}]`），
        // 只取字符串会得到空串，整条词条被误判为「无法解析出有效释义」。
        let def = crate::dict::jsonpath::query_text(o, &m.sense_def);
        // 有道系 JSON 的文本里混着 <self>/<b> 之类的内联标记，直接渲染会把
        // 标签当成释义的一部分显示出来，这里统一清掉。
        let def = strip_markup(&def);
        if def.trim().is_empty() {
            continue;
        }
        let mut examples = Vec::new();
        if !m.sense_examples.is_empty() {
            // `examples` 这类字段本身常常就是一个字符串数组，wildcard 取回来
            // 的是「数组」而不是元素，所以先把数组摊平再逐个处理
            let mut raw_examples: Vec<&serde_json::Value> = Vec::new();
            for v in crate::dict::jsonpath::query_wildcard(o, &m.sense_examples) {
                match v {
                    serde_json::Value::Array(arr) => raw_examples.extend(arr.iter()),
                    other => raw_examples.push(other),
                }
            }
            for ex in raw_examples {
                let text = match ex {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(_) => {
                        crate::dict::jsonpath::query_str(ex, &m.example_text)
                    }
                    _ => String::new(),
                };
                // 汉语规范词典的例句用 <self>词</self> 标出词条本身，
                // 不清掉就会在界面上原样显示标签
                let text = strip_markup(&text);
                if !text.trim().is_empty() {
                    examples.push(Example {
                        text,
                        translation: strip_markup(&crate::dict::jsonpath::query_str(
                            ex,
                            &m.example_translation,
                        )),
                    });
                }
            }
        }
        e.senses.push(Sense {
            pos: normalize_pos(&pos),
            definition: def,
            examples,
        });
    }

    // 变形：支持两种形态——对象列表 {label,form} 或字符串列表
    if !m.inflections.is_empty() {
        for it in crate::dict::jsonpath::query_wildcard(raw, &m.inflections) {
            match it {
                serde_json::Value::Object(_) => {
                    let label = crate::dict::jsonpath::query_str(it, &m.inflection_label);
                    let form = crate::dict::jsonpath::query_str(it, &m.inflection_form);
                    if !form.is_empty() {
                        e.inflections.push(Inflection { label, form });
                    }
                }
                serde_json::Value::String(s) if !s.is_empty() => {
                    e.inflections.push(Inflection {
                        label: String::new(),
                        form: s.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    // 相关词：去重
    if !m.related.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for w in crate::dict::jsonpath::query_list(raw, &m.related) {
            if w.len() < 40 && seen.insert(w.clone()) {
                e.related.push(w);
            }
        }
        e.related.truncate(20);
    }

    if !m.mnemonic.is_empty() {
        e.mnemonic = crate::dict::jsonpath::query_str(raw, &m.mnemonic);
    }

    // 变形兜底：内置词典源普遍不提供变形字段，
    // 当映射未给出变形且为英语单词时，用规则推导保证标签页有内容（需求 5）。
    if e.inflections.is_empty() && e.lang == "en" {
        let poss: Vec<String> = e.senses.iter().map(|s| s.pos.clone()).collect();
        let derived = crate::dict::inflect::derive(&e.word, "", &poss);
        if !derived.is_empty() {
            e.inflections = derived;
        }
    }

    // 保留原始结构便于前端兜底展示
    e.extra
        .insert("raw".to_string(), serde_json::json!(raw.to_string()));
    e
}

/// 把英文词性缩写统一成中文，界面更友好。
fn normalize_pos(p: &str) -> String {
    let t = p.trim().to_lowercase();
    let t = t.trim_end_matches('.');
    match t {
        "n" | "noun" => "n.",
        "v" | "verb" => "v.",
        "adj" | "adjective" => "adj.",
        "adv" | "adverb" => "adv.",
        "prep" | "preposition" => "prep.",
        "conj" | "conjunction" => "conj.",
        "pron" | "pronoun" => "pron.",
        "num" | "numeral" => "num.",
        "art" | "article" => "art.",
        "int" | "interjection" => "int.",
        "phrase" => "phrase",
        "abbrev" | "abbreviation" => "abbr.",
        _ => return p.trim().to_string(),
    }
    .to_string()
}

/// 去掉文本里的内联标记（`<self>词</self>`、`<b>…</b>` 等）。
///
/// 有道系接口把词条/强调部分标成 HTML 标签；这些文本最终是走
/// `esc()` 转义后原样渲染的，不清理就会在释义里看到一堆尖括号。
/// 这里只做标签剥离，不解析 HTML 实体——实体在文本词典里几乎不出现，
/// 而引入解码表只会增加出错面。
fn strip_markup(s: &str) -> String {
    if !s.contains('<') {
        return s.trim().to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    out.push(c);
                }
            }
        }
    }
    // 合并因为剥离标签而出现的多余空格
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 判断词条是否算「有效」——至少要有释义或音标。
fn is_meaningful(e: &WordEntry) -> bool {
    !e.senses.is_empty() || !e.phonetic.uk.is_empty() || !e.related.is_empty()
}

/// 访问单个词典源，返回归一化后的词条（或错误）。
async fn query_one_source(
    word: &str,
    lang: &str,
    cfg: &DictSourceConfig,
    client: &reqwest::Client,
    timeout_secs: u64,
    retries: usize,
) -> (Option<WordEntry>, String, i64) {
    let url = render_url(&cfg.url_template, word, lang, &cfg.api_key);
    let started = std::time::Instant::now();

    let resp = if cfg.method.eq_ignore_ascii_case("POST") {
        // 目标语言：非中文的源语言统一翻译为中文，中文则翻译为英文
        let target = if lang == "zh" { "en" } else { "zh" };
        net::post_json(
            client, &url, &cfg.headers, &cfg.api_key, word, lang, target, timeout_secs,
        )
        .await
    } else {
        net::get_json(
            client, &url, &cfg.headers, &cfg.api_key, timeout_secs, retries, word, lang,
        )
        .await
    };

    let elapsed = started.elapsed().as_millis() as i64;
    match resp {
        Ok(v) => {
            let e = normalize(&v, cfg, word, lang);
            if is_meaningful(&e) {
                (Some(e), String::new(), elapsed)
            } else {
                (None, "返回结构无法解析出有效释义".into(), elapsed)
            }
        }
        Err(e) => (None, e.to_string(), elapsed),
    }
}

/// 并发访问各词典源，聚合结果。
///
/// 为什么必须并发：老实现是串行 for 循环，一旦某个源被墙（例如
/// `en.wiktionary.org` 在国内直连必然是超时），就要先把它的
/// 「重试 × 超时」全部等完才轮到下一个源。用户感受到的就是「转圈几十秒，
/// 最后还查不出东西」。现在所有源同时发出，整体耗时≈最快的可用源，
/// 而不是所有源耗时之和。
///
/// 另外两个针对性优化：
/// - 单源超时短（默认 5 秒）+ 查询不重试，避免个别源拖垮整条链路；
/// - `needs_proxy` 的源在没有代理时直接跳过并在 trace 里注明，
///   免得用户以为「查不出词」是自己拼错了。
pub async fn lookup_multi(
    word: &str,
    lang: &str,
    sources: &[DictSourceConfig],
    client: &reqwest::Client,
    timeout_secs: u64,
    retries: usize,
    proxy_active: bool,
    trace_out: &mut Vec<SourceTrace>,
) -> Option<(WordEntry, Vec<String>)> {
    use futures_util::stream::{FuturesUnordered, StreamExt};

    // 按优先级排序，只取启用的、且支持该语言的源
    let mut ordered: Vec<&DictSourceConfig> = sources
        .iter()
        .filter(|s| s.enabled)
        .filter(|s| s.langs.is_empty() || s.langs.iter().any(|l| l == lang))
        .collect();
    ordered.sort_by_key(|s| s.priority);

    let mut usable: Vec<&DictSourceConfig> = Vec::with_capacity(ordered.len());
    for cfg in ordered {
        if cfg.needs_proxy && !proxy_active {
            trace_out.push(SourceTrace {
                source: cfg.name.clone(),
                ok: false,
                error: "已跳过：该源在国内需走代理（可在设置里配置代理后重试）".into(),
                elapsed_ms: 0,
            });
            continue;
        }
        usable.push(cfg);
    }

    if usable.is_empty() {
        return None;
    }

    let budget = std::time::Duration::from_secs(timeout_secs.clamp(3, 30));
    let deadline = tokio::time::Instant::now() + budget;

    let mut pending = FuturesUnordered::new();
    for (idx, cfg) in usable.iter().enumerate() {
        pending.push(async move {
            let (entry, err, elapsed) =
                query_one_source(word, lang, cfg, client, timeout_secs, retries).await;
            (idx, entry, err, elapsed)
        });
    }

    // 收集所有（在预算内）返回的结果；每个源只保留 idx 且顺序可恢复，
    // 以便按优先级合并，而不是「谁先返回谁说了算」。
    let mut outcomes: Vec<(usize, Option<WordEntry>, String, i64)> = Vec::new();
    let mut merged: Option<WordEntry> = None;

    let collect = async {
        while let Some(item) = pending.next().await {
            let (idx, entry, err, elapsed) = item;
            if let Some(e) = &entry {
                match &mut merged {
                    Some(m) => m.merge_from(e.clone()),
                    None => merged = Some(e.clone()),
                }
            }
            outcomes.push((idx, entry, err, elapsed));

            // 已经拿到「释义 + 音标」这种完整词条就没必要再等了
            if let Some(m) = &merged {
                if !m.senses.is_empty() && !m.phonetic.uk.is_empty() {
                    break;
                }
            }
        }
    };
    // 超时后保留已经收到的结果，不让用户白等
    let _ = tokio::time::timeout_at(deadline, collect).await;

    // 按优先级顺序合并（而不是按返回顺序），保证高质量源优先填充字段
    let mut ordered_outcomes = outcomes;
    ordered_outcomes.sort_by_key(|(idx, _, _, _)| *idx);

    let mut merged: Option<WordEntry> = None;
    let mut hit_sources: Vec<String> = Vec::new();
    for (idx, entry, err, elapsed) in ordered_outcomes {
        let cfg = usable[idx];
        match entry {
            Some(e) => {
                trace_out.push(SourceTrace {
                    source: cfg.name.clone(),
                    ok: true,
                    error: String::new(),
                    elapsed_ms: elapsed,
                });
                hit_sources.push(cfg.name.clone());
                match &mut merged {
                    Some(m) => m.merge_from(e),
                    None => merged = Some(e),
                }
            }
            None => {
                trace_out.push(SourceTrace {
                    source: cfg.name.clone(),
                    ok: false,
                    error: if err.is_empty() { "无有效结果".into() } else { err },
                    elapsed_ms: elapsed,
                });
            }
        }
    }

    // 没有返回的任何源，一律标注为「超时未响应」，方便用户在设置里排查
    if merged.is_none() {
        for cfg in usable.iter() {
            if !trace_out.iter().any(|t| t.source == cfg.name) {
                trace_out.push(SourceTrace {
                    source: cfg.name.clone(),
                    ok: false,
                    error: format!("{} 秒内未响应（超时）", timeout_secs),
                    elapsed_ms: (timeout_secs as i64) * 1000,
                });
            }
        }
    }

    merged.map(|m| (m, hit_sources))
}

/// 带缓存的查词入口：先查缓存，再联网，最后可选大模型兜底。
///
/// `net_cfg` 决定单源超时与重试；`proxy_active` 用于跳过需要代理的源。
#[allow(clippy::too_many_arguments)]
pub async fn lookup_with_cache(
    db: &Db,
    word: &str,
    lang: &str,
    sources: &[DictSourceConfig],
    client: &reqwest::Client,
    cache_ttl_secs: i64,
    allow_llm: bool,
    net_cfg: &crate::models::NetworkConfig,
    proxy_active: bool,
    llm_fallback: impl std::future::Future<Output = Result<WordEntry>>,
) -> Result<LookupResult> {
    let now = crate::timeutil::now_ts();
    let key = word.trim().to_lowercase();

    // 1) 词库命中直接返回（用户已导入的高质量词条优先）
    if let Ok(Some(e)) = db.get_word(&key, lang) {
        return Ok(LookupResult {
            word: e.word.clone(),
            lang: e.lang.clone(),
            entry: e,
            sources: vec!["本地词库".into()],
            from_cache: true,
            from_llm: false,
            trace: vec![],
        });
    }

    // 2) 词典缓存
    if let Ok(Some(e)) = db.get_cached(&key, lang, cache_ttl_secs, now) {
        return Ok(LookupResult {
            word: e.word.clone(),
            lang: e.lang.clone(),
            entry: e,
            sources: vec!["缓存".into()],
            from_cache: true,
            from_llm: false,
            trace: vec![],
        });
    }

    // 3) 联网多源聚合
    let mut trace = Vec::new();
    let lookup_timeout = net_cfg.lookup_timeout_secs.clamp(3, 30);
    if let Some((mut entry, srcs)) = lookup_multi(
        &key,
        lang,
        sources,
        client,
        lookup_timeout,
        0, // 查词不做重试：宁可快速判失败，也别让用户干等
        proxy_active,
        &mut trace,
    )
    .await
    {
        entry.lang = lang.to_string();
        // 保留用户查询时的原始拼写形式，避免大小写/变形导致后续查不到
        if entry.word.trim().is_empty() {
            entry.word = key.clone();
        }
        db.cache_entry(&entry, &srcs.join(","), now).ok();
        return Ok(LookupResult {
            word: entry.word.clone(),
            lang: entry.lang.clone(),
            entry,
            sources: srcs,
            from_cache: false,
            from_llm: false,
            trace,
        });
    }

    // 4) 本地大模型兜底（离线也能用）
    //
    // 注意：本地模型的超时是 `llm.timeout_secs`（默认 120 秒）。在线词典全部失败时，
    // 界面会在这段时间里一直转圈、且毫无进展提示，看起来就像「卡死了」。
    // 这里给兜底单独压一个上限，超时就直接报错，让前端能给出明确提示。
    if allow_llm {
        const LLM_FALLBACK_TIMEOUT_SECS: u64 = 45;
        let r = match tokio::time::timeout(
            std::time::Duration::from_secs(LLM_FALLBACK_TIMEOUT_SECS),
            llm_fallback,
        )
        .await
        {
            Ok(r) => r,
            Err(_) => anyhow::bail!(
                "所有在线词典源均失败，本地大模型 {} 秒内未返回结果。\
                 可到「设置 → 本地大模型」关闭 AI 讲解，或换一个更小的模型。",
                LLM_FALLBACK_TIMEOUT_SECS
            ),
        };
        match r {
            Ok(mut entry) => {
                let mut entry = {
                    entry.word = if entry.word.trim().is_empty() {
                        key.clone()
                    } else {
                        entry.word
                    };
                    entry.lang = lang.to_string();
                    entry.source = "lmstudio".into();
                    entry
                };
                entry.senses.retain(|s| !s.definition.trim().is_empty());
                if is_meaningful(&entry) {
                    db.cache_entry(&entry, "lmstudio", now).ok();
                    return Ok(LookupResult {
                        word: entry.word.clone(),
                        lang: entry.lang.clone(),
                        entry,
                        sources: vec!["本地大模型".into()],
                        from_cache: false,
                        from_llm: true,
                        trace,
                    });
                }
                anyhow::bail!("本地大模型返回内容为空");
            }
            Err(e) => {
                anyhow::bail!(
                    "所有在线词典源均失败，且本地大模型不可用（{}）。请检查网络或启动 LM Studio。",
                    e
                )
            }
        }
    }

    anyhow::bail!("未找到该单词的释义：{}", word)
}

/// 批量预取若干词的释义（导入词库 / 预热缓存时使用）。
pub async fn prefetch(
    db: &Db,
    words: &[String],
    lang: &str,
    sources: &[DictSourceConfig],
    client: &reqwest::Client,
    concurrency: usize,
) -> Result<usize> {
    use futures_util::stream::{self, StreamExt};
    let now = crate::timeutil::now_ts();

    let results: Vec<Option<WordEntry>> = stream::iter(words.iter().cloned())
        .map(|w| {
            let cl = client.clone();
            let srcs = sources.to_vec();
            async move {
                let mut tr = Vec::new();
                let net_cfg = crate::models::NetworkConfig::default();
                // `proxy_active` 跟着网络配置走，而不是写死 true：
                // 默认配置是直连，写死 true 会让需要代理的源白等超时。
                lookup_multi(
                    &w,
                    lang,
                    &srcs,
                    &cl,
                    net_cfg.lookup_timeout_secs,
                    0,
                    net_cfg.enable_proxy,
                    &mut tr,
                )
                .await
                .map(|(e, _)| e)
            }
        })
        .buffer_unordered(concurrency.max(1).min(8))
        .collect()
        .await;

    let mut n = 0;
    for e in results.into_iter().flatten() {
        if is_meaningful(&e) {
            db.cache_entry(&e, &e.source.clone(), now).ok();
            n += 1;
        }
    }
    Ok(n)
}

/// 合并两个词条，暴露给命令层。
pub fn merge_entries(a: &mut WordEntry, b: WordEntry) {
    a.merge_from(b);
}

/// 快捷构造：用于本地大模型兜底时手工组装。
pub fn make_entry(word: &str, lang: &str, senses: Vec<(String, String)>, source: &str) -> WordEntry {
    let mut e = WordEntry::new(word);
    e.lang = lang.to_string();
    e.source = source.to_string();
    for (pos, def) in senses {
        e.senses.push(Sense {
            pos,
            definition: def,
            examples: vec![],
        });
    }
    e
}

/// 便捷：从 BTreeMap 构造 headers。
pub fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::builtin;

    #[test]
    fn render_url_encodes_word() {
        let u = render_url("https://x.com/{word}?l={lang}", "hello world", "en", "");
        assert!(u.contains("hello%20world"));
        assert!(u.contains("l=en"));
    }

    /// 语言判定：这是「多语言适配」的核心——中文词不能被当成日语词。
    #[test]
    fn detect_lang_by_script() {
        // 中文
        assert_eq!(detect_lang("开心"), Some("zh"));
        assert_eq!(detect_lang("苹果"), Some("zh"));
        // 日语：只要出现假名就一定是日语
        assert_eq!(detect_lang("たべる"), Some("ja"));
        assert_eq!(detect_lang("カタカナ"), Some("ja"));
        assert_eq!(detect_lang("食べる"), Some("ja"));
        // 韩语
        assert_eq!(detect_lang("배우다"), Some("ko"));
        // 俄语
        assert_eq!(detect_lang("привет"), Some("ru"));
        // 拉丁字母无法区分英/法/德/西，交回用户选择
        assert_eq!(detect_lang("apple"), None);
        assert_eq!(detect_lang("bonjour"), None);
        assert_eq!(detect_lang("café"), None);
        // 混合串：假名优先于汉字
        assert_eq!(detect_lang("日本語を学ぶ"), Some("ja"));
        // 数字/符号等判不出来
        assert_eq!(detect_lang("123"), None);
        assert_eq!(detect_lang(""), None);
    }

    /// 有道 jsonapi 的 `newhh` / `ce` 两段能被正确归一化出中文释义。
    ///
    /// 回归保护：原先 `normalize` 用 `query_str` 取释义，遇到
    /// `def: ["心情愉快；高兴"]` 这种数组会得到空串，整条词条被判为
    /// 「无法解析出有效释义」而丢弃 —— 用户看到的就是「中文词查不出来」。
    #[test]
    fn normalize_youdao_jsonapi_shapes() {
        let raw = serde_json::json!({
            "meta": { "input": "开心", "guessLanguage": "zh" },
            "simple": { "word": [{ "phone": "kāi xīn", "return-phrase": "开心" }] },
            "newhh": {
                "dataList": [{
                    "word": "开心",
                    "sense": [{
                        "cat": "形容词",
                        "def": ["心情愉快；高兴"],
                        "examples": ["小日子过得很开心"]
                    }]
                }]
            }
        });
        let cfg = builtin::youdao_jsonapi_newhh();
        let e = normalize(&raw, &cfg, "开心", "zh");
        assert_eq!(e.word, "开心");
        assert_eq!(e.lang, "zh");
        assert_eq!(e.senses.len(), 1);
        assert_eq!(e.senses[0].definition, "心情愉快；高兴");
        assert_eq!(e.senses[0].pos, "形容词");
        assert_eq!(e.senses[0].examples.len(), 1);
        assert!(is_meaningful(&e));

        // `ce` 段（中/日/韩…通用）
        let raw2 = serde_json::json!({
            "meta": { "input": "日本語" },
            "ce": { "word": [{ "trs": [{ "tr": [{ "l": {
                "pos": "adj.",
                "i": ["", { "#text": "Japanese" }, " "],
                "#tran": "日本（人）的；日语的；日本文化的；"
            } }] }] }] }
        });
        let cfg2 = builtin::youdao_jsonapi_ce();
        let e2 = normalize(&raw2, &cfg2, "日本語", "ja");
        assert_eq!(e2.senses.len(), 1);
        assert_eq!(e2.senses[0].definition, "日本（人）的；日语的；日本文化的；");
        assert_eq!(e2.senses[0].pos, "adj.");
        assert!(is_meaningful(&e2));
    }

    #[test]
    fn normalize_free_dictionary_shape() {
        // freedictionaryapi.com 的真实结构（截取 apple 的首个 entry）
        let raw = serde_json::json!({
            "word": "apple",
            "entries": [{
                "language": {"code": "en", "name": "English"},
                "partOfSpeech": "noun",
                "pronunciations": [
                    {"type": "ipa", "text": "/ˈæp.əl/", "tags": []},
                    {"type": "ipa", "text": "/ˈæ.pɘl/", "tags": []}
                ],
                "forms": [{"word": "apples", "tags": ["plural"]}],
                "senses": [{
                    "definition": "A common, firm, round fruit produced by a tree of the genus Malus.",
                    "examples": ["I ate an apple."],
                    "synonyms": [],
                    "subsenses": []
                }]
            }],
            "source": {"url": "https://en.wiktionary.org/wiki/apple"}
        });
        let cfg = builtin::free_dictionary();
        let e = normalize(&raw, &cfg, "apple", "en");
        assert_eq!(e.word, "apple");
        assert!(!e.senses.is_empty());
        assert_eq!(e.senses[0].pos, "n.");
        assert!(e.senses[0].definition.contains("round fruit"));
        // 音标：多个 IPA 里取第一个带斜杠的
        assert_eq!(e.phonetic.uk, "/ˈæp.əl/");
        // 词形变化来自 forms
        assert!(
            e.inflections.iter().any(|i| i.form == "apples"),
            "应从 forms 解析出词形变化，实际：{:?}",
            e.inflections
        );
    }

    #[test]
    fn normalize_youdao_shape() {
        let raw = serde_json::json!({
            "query": "apple",
            "data": {"entries": [{"type": "n.", "explain": "苹果 n. 苹果树"}]}
        });
        let cfg = builtin::youdao_suggest();
        let e = normalize(&raw, &cfg, "apple", "en");
        assert_eq!(e.senses.len(), 1);
        assert_eq!(e.senses[0].definition, "苹果 n. 苹果树");
    }

    /// 失效的 dictionaryapi.dev 结构不该再被内置源指向——
    /// 它现在对所有词都返回 404，留着只会让每次查词白等一次。
    #[test]
    fn builtin_english_source_is_not_the_dead_api() {
        let fd = builtin::free_dictionary();
        assert!(
            !fd.url_template.contains("dictionaryapi.dev"),
            "内置英英源已失效的接口不应再被使用"
        );
        assert!(fd.enabled, "英英源应默认启用");
        assert!(!fd.needs_proxy, "英英源必须国内可直连");
    }

    #[test]
    fn meaningful_detection() {
        let mut e = WordEntry::new("x");
        assert!(!is_meaningful(&e));
        e.senses.push(Sense { pos: "n.".into(), definition: "y".into(), examples: vec![] });
        assert!(is_meaningful(&e));
    }

    #[test]
    fn pos_normalization() {
        assert_eq!(normalize_pos("noun"), "n.");
        assert_eq!(normalize_pos("VERB"), "v.");
        assert_eq!(normalize_pos("adj."), "adj.");
        assert_eq!(normalize_pos("xyz"), "xyz");
    }
}
