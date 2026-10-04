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

// 子模块：内置词典源定义 + 通用 JSON 路径取值器
pub mod builtin;
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
        let pos = crate::dict::jsonpath::query_str(o, &m.sense_pos);
        let def = crate::dict::jsonpath::query_str(o, &m.sense_def);
        if def.trim().is_empty() {
            continue;
        }
        let mut examples = Vec::new();
        if !m.sense_examples.is_empty() {
            for ex in crate::dict::jsonpath::query_wildcard(o, &m.sense_examples) {
                let text = match ex {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(_) => {
                        crate::dict::jsonpath::query_str(ex, &m.example_text)
                    }
                    _ => String::new(),
                };
                if !text.trim().is_empty() {
                    examples.push(Example {
                        text: text.clone(),
                        translation: crate::dict::jsonpath::query_str(ex, &m.example_translation),
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

/// 判断词条是否算「有效」——至少要有释义或音标。
fn is_meaningful(e: &WordEntry) -> bool {
    !e.senses.is_empty() || !e.phonetic.uk.is_empty() || !e.related.is_empty()
}

/// 依次访问各词典源，聚合结果。任一源成功即返回，同时尽量补全。
pub async fn lookup_multi(
    word: &str,
    lang: &str,
    sources: &[DictSourceConfig],
    client: &reqwest::Client,
    trace_out: &mut Vec<SourceTrace>,
) -> Option<(WordEntry, Vec<String>)> {
    // 按优先级排序，只取启用的、且支持该语言的源
    let mut ordered: Vec<&DictSourceConfig> = sources
        .iter()
        .filter(|s| s.enabled)
        .filter(|s| s.langs.is_empty() || s.langs.iter().any(|l| l == lang))
        .collect();
    ordered.sort_by_key(|s| s.priority);

    let mut merged: Option<WordEntry> = None;
    let mut hit_sources: Vec<String> = Vec::new();

    for cfg in ordered {
        let url = render_url(&cfg.url_template, word, lang, &cfg.api_key);
        let started = std::time::Instant::now();

        let resp = if cfg.method.eq_ignore_ascii_case("POST") {
            // 目标语言：非中文的源语言统一翻译为中文，中文则翻译为英文
            let target = if lang == "zh" { "en" } else { "zh" };
            net::post_json(client, &url, &cfg.headers, &cfg.api_key, word, lang, target).await
        } else {
            net::get_json(client, &url, &cfg.headers, &cfg.api_key, word, lang).await
        };

        let elapsed = started.elapsed().as_millis() as i64;

        match resp {
            Ok(v) => {
                let e = normalize(&v, cfg, word, lang);
                if is_meaningful(&e) {
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
                    // 已经拿到释义 + 音标 + 例句，没必要继续请求
                    if let Some(m) = &merged {
                        if !m.senses.is_empty() && !m.phonetic.uk.is_empty() {
                            let has_ex = m.senses.iter().any(|s| !s.examples.is_empty());
                            if has_ex || m.lang != "en" {
                                break;
                            }
                        }
                    }
                } else {
                    trace_out.push(SourceTrace {
                        source: cfg.name.clone(),
                        ok: false,
                        error: "返回结构无法解析出有效释义".into(),
                        elapsed_ms: elapsed,
                    });
                }
            }
            Err(e) => {
                trace_out.push(SourceTrace {
                    source: cfg.name.clone(),
                    ok: false,
                    error: e.to_string(),
                    elapsed_ms: elapsed,
                });
            }
        }
    }

    merged.map(|m| (m, hit_sources))
}

/// 带缓存的查词入口：先查缓存，再联网，最后可选大模型兜底。
pub async fn lookup_with_cache(
    db: &Db,
    word: &str,
    lang: &str,
    sources: &[DictSourceConfig],
    client: &reqwest::Client,
    cache_ttl_secs: i64,
    allow_llm: bool,
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
    if let Some((mut entry, srcs)) = lookup_multi(&key, lang, sources, client, &mut trace).await {
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
    if allow_llm {
        match llm_fallback.await {
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
                lookup_multi(&w, lang, &srcs, &cl, &mut tr)
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

    #[test]
    fn normalize_free_dictionary_shape() {
        let raw = serde_json::json!([{
            "word": "apple",
            "phonetic": "/ˈæp.əl/",
            "phonetics": [{"text": "/ˈæp.əl/"}, {"text": "", "audio": "https://a/b.mp3"}],
            "meanings": [{
                "partOfSpeech": "noun",
                "definitions": [{"definition": "a round fruit", "example": "I ate an apple."}],
                "synonyms": ["pome"]
            }]
        }]);
        let cfg = builtin::free_dictionary();
        let e = normalize(&raw, &cfg, "apple", "en");
        assert_eq!(e.word, "apple");
        assert!(!e.senses.is_empty());
        assert_eq!(e.senses[0].pos, "n.");
        assert_eq!(e.senses[0].definition, "a round fruit");
        assert!(e.phonetic.audio.contains("http"));
        assert!(e.related.contains(&"pome".to_string()));
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
