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
    /// 语言被书写系统判定纠正时的说明（需求 3）。
    ///
    /// 非空表示「顶部方向选择器里的源语言与输入内容不符」，前端应如实提示，
    /// 否则用户只会看到「选了日语却出来中文/英文」而不知道为什么。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang_note: Option<String>,
    /// 降级说明：**查询本身成功**，但有增强能力没参与。
    ///
    /// 用户需求原文：「只要任意一个词典能够正常连接并返回结果，就应当显示
    /// 查询成功，而不是仅因为 AI 服务器未开启就判定为失败」。
    /// 所以「本地大模型没开 / 超时 / 返回空」这类情况一律**不构成失败**，
    /// 只在这里留一句说明，前端以「提示条」形式展示，而不是「查询失败」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 结果是否走了降级路径（启发式解析 / 纯文本兜底 / 缺 AI 讲解）。
    ///
    /// 前端据此把「数据来源」一行标灰，提示用户结果可用但不完美。
    #[serde(default)]
    pub degraded: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceTrace {
    pub source: String,
    pub ok: bool,
    pub error: String,
    pub elapsed_ms: i64,
}

/// 双向词条的「另一条腿」：**目标语言侧**的对应词及其完整词条。
///
/// 为什么需要它（用户需求原文）：
/// > 「这里我是中文转英文，应该下面详细介绍的是 crow 或者其他能表示乌鸦的单词」
/// > 「仿照有道词典两者都有，不然我输入英文的时候没有英文解释对吧，还有其他语言也要类似」
///
/// 查「乌鸦」只能拿到中文释义，但用户真正要学的是「乌鸦用目标语言怎么说、
/// 那个词怎么读、怎么用」。所以这里必须**先翻译出对应词，再按目标语言查一次
/// 词典**，把完整的英文（或日文、法文…）词条拿回来。这条规则对任意
/// (源语言, 目标语言) 都成立，不限于中→英。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PairEntry {
    /// 对应词原文，如 `crow`
    pub word: String,
    /// 对应词的语言，如 `en`
    pub lang: String,
    /// 该词在目标语言下的完整词条
    pub entry: WordEntry,
    /// 这个对应词是怎么来的：
    /// - `translation` 主译文（翻译接口给出的首选译法）
    /// - `alternative` 其他候选译法
    pub via: String,
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

/// 书写系统**不含拉丁字母**的语言。
///
/// 这些语言与「纯拉丁字母的词」互斥 —— 一个词是 `reality`，它就绝不可能是
/// 中文 / 日文 / 韩文 / 俄文。反过来，英 / 法 / 德 / 西 之间**无法靠字形区分**
/// （都是拉丁字母），所以它们**不在**这张表里，必须尊重用户选择。
const NON_LATIN_LANGS: [&str; 9] =
    ["zh", "ja", "ko", "ru", "ar", "hi", "th", "el", "he"];

/// 该语言是否使用非拉丁书写系统（据此判断「拉丁字母词」与它互斥）。
pub fn is_non_latin_lang(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    let base = l.split(['-', '_']).next().unwrap_or("");
    NON_LATIN_LANGS.contains(&base)
}

/// 这个词是否**完全由拉丁字母构成**（含带变音符的拉丁扩展，如 `café`）。
///
/// 与 `detect_lang` 的区别：`detect_lang` 回答「是哪种语言」，对拉丁字母返回
/// `None`（判不出）；这个函数回答「**不可能是**哪些语言」，纯拉丁字母返回
/// `true` → 可以据此否掉中日韩俄等候选。
///
/// 数字、空格、连字符、撇号等非字母字符不影响判定（`"don't"` 仍是拉丁词）。
/// 一个字母都没有（如 `"123"`）返回 `false`，因为无从判断。
pub fn is_latin_only(word: &str) -> bool {
    let mut latin = false;
    for c in word.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => return false, // 假名
            0x1100..=0x11FF | 0xAC00..=0xD7AF => return false, // 谚文
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => return false, // 汉字
            0x0400..=0x04FF => return false,                   // 西里尔
            0x0370..=0x03FF => return false,                   // 希腊
            0x0E00..=0x0E7F => return false,                   // 泰文
            0x0590..=0x05FF => return false,                   // 希伯来
            0x0600..=0x06FF | 0x0750..=0x077F => return false, // 阿拉伯
            _ => {
                if c.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&(c as u32)) {
                    latin = true;
                }
            }
        }
    }
    latin
}

/// 渲染 URL 模板。
fn render_url(tpl: &str, word: &str, lang: &str, key: &str) -> String {
    tpl.replace("{word}", &urlencoding::encode(word))
        .replace("{lang}", lang)
        .replace("{key}", key)
}

/// 是否为 CJK 字符（汉字 / 假名 / 谚文）。
fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xAC00..=0xD7AF
    )
}

/// 像不像一个可以拿去查词的「词」。
///
/// 拉丁词只允许字母（含变音符，法/德/西都在用）、连字符与撇号；
/// CJK 词限制 1~8 个字。用来滤掉 "n."、"（见 also）" 这类噪声。
fn looks_like_related(w: &str) -> bool {
    if w.is_empty() || w.chars().count() > 30 {
        return false;
    }
    if w.chars().any(is_cjk) {
        return w.chars().count() <= 8 && w.chars().all(|c| is_cjk(c) || c.is_ascii_digit());
    }
    w.chars().next().is_some_and(|c| c.is_alphabetic())
        && w.chars()
            .all(|c| c.is_alphabetic() || c == '-' || c == '\'' || c == '\u{2019}')
}

/// 把一条「相关词」拆成一个个真正的词。
///
/// 为什么必须拆：不同词典源对 synonyms 的封装差别极大 —— 有的是数组，
/// 有的把整串塞进**一个**元素里（`"desert, abandon, leave"`，甚至空格连排
/// 的 `"desert abandon"`）。不拆的话前端只画出一个巨大的 chip，点下去拿
/// 整串去查词必然查不到 —— 用户看到的就是「相关词没分开、点相关词查不出东西」。
fn split_related(raw: &str, lang: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let seps = [',', ';', '，', '；', '、', '/', '|', '\n', '\r', '\t', '·'];
    for part in raw.split(|c: char| seps.contains(&c)) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        // 拉丁/非 CJK 串里还可能有空格连排，按空格再拆一次；
        // CJK 串里的空格往往是「词组」的一部分，保留原样。
        let pieces: Vec<&str> = if lang == "en" || !p.chars().any(is_cjk) {
            p.split_whitespace().collect()
        } else {
            vec![p]
        };
        for w in pieces {
            let w = w.trim_matches(|c: char| {
                matches!(c, '.' | '。' | ',' | '，' | '(' | ')' | '[' | ']' | '"' | '\'')
            });
            if looks_like_related(w) && !out.iter().any(|x| x == w) {
                out.push(w.to_string());
            }
        }
    }
    out
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
    // ★ 入库前清洗（P4 音标污染）：jsonpath 会把领域标签抓进音标字段
    //   （实测 fault 的 uk/us = "[地质]"）。规则按语言分流，见
    //   `clean_phonetic_value` —— 清洗后为空的值宁可留空（前端音标行会
    //   退回纯发音按钮），也不入库现形。
    e.phonetic.uk = crate::models::clean_phonetic_value(
        &uks.iter()
            .find(|s| !s.trim().is_empty() && s.contains('/'))
            .or_else(|| uks.iter().find(|s| !s.trim().is_empty()))
            .cloned()
            .unwrap_or_default(),
        lang,
    );
    e.phonetic.us = crate::models::clean_phonetic_value(
        &uss.iter()
            .find(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_default(),
        lang,
    );
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

    // 相关词：先按分隔符拆成一个个真正的词，再去重
    if !m.related.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for raw_rel in crate::dict::jsonpath::query_list(raw, &m.related) {
            for w in split_related(&raw_rel, lang) {
                if w != word && seen.insert(w.clone()) {
                    e.related.push(w);
                }
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
///
/// 这是**匹配式**（`FieldMapping` 精确取值）的合格线。达不到不一定是失败，
/// 见下面的 `is_presentable`。
///
/// 暴露给命令层的原因：`cmd_lookup_pairs`（双向词条）要拿它筛掉
/// 「翻译出了候选词，但候选词在目标语言里查不到东西」的空结果。
pub fn is_meaningful(e: &WordEntry) -> bool {
    !e.senses.is_empty() || !e.phonetic.uk.is_empty() || !e.related.is_empty()
}

/// 启发式结果在 `extra` 里的标记键。
///
/// `is_presentable` 用它区分「启发式真的捞到了东西」和「只是返回了个空壳」。
const HEURISTIC_TAG: &str = "heuristic";

/// 「有内容可展示」——比 `is_meaningful` 宽松一档。
///
/// 为什么需要两档判定：用户明确要求「只要任意一个词典能够正常连接并返回
/// 结果，就应当显示查询成功」。映射没对上（上游改了字段名）只是**解析方式**
/// 的问题，不该被报成「查询失败」。所以只要启发式/纯文本兜底捞到了释义、
/// 音标或记忆法，就算查询成功，只是把结果标记为「降级」。
fn is_presentable(e: &WordEntry) -> bool {
    is_meaningful(e)
        || !e.mnemonic.trim().is_empty()
        || (e.extra.contains_key(HEURISTIC_TAG) && !e.senses.is_empty())
}

// ---------------------------------------------------------------
// 启发式解析（需求：升级查词方式，支持「匹配式」与「启发式」两种策略）
// ---------------------------------------------------------------

/// 键名归类：决定某个键下面的字符串该当释义、例句、音标还是相关词看。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Def,
    Example,
    Phonetic,
    Related,
    Other,
}

fn norm_key(k: &str) -> String {
    k.trim()
        .to_lowercase()
        .replace(['_', '-', ' ', '.'], "")
}

/// 按键名判断它装的是什么。
///
/// 刻意**不**收录 `text` / `content` / `value` 这类过于通用的名字：
/// 它们在全国性网站导航里到处都是，收进来会把「隐私政策」当成释义灌进词条。
/// 真正需要它们的场景（有道的 `#text`）靠「父键已经指明是 def」的继承规则覆盖。
fn key_kind(k: &str) -> KeyKind {
    let n = norm_key(k);
    if n.is_empty() {
        return KeyKind::Other;
    }
    const DEF: &[&str] = &[
        "def",
        "defs",
        "definition",
        "definitions",
        "meaning",
        "meanings",
        "trans",
        "translation",
        "translations",
        "explain",
        "explains",
        "explanation",
        "gloss",
        "glossary",
        "sense",
        "senses",
        "cn",
        "zh",
        "chinese",
        "interpretation",
        "paraphrase",
        "释义",
        "解释",
        "意思",
        "翻译",
    ];
    const EX: &[&str] = &[
        "example",
        "examples",
        "sent",
        "sentence",
        "sentences",
        "eg",
        "egs",
        "sample",
        "samples",
        "usage",
        "例句",
    ];
    const PH: &[&str] = &[
        "phonetic",
        "phonetics",
        "usphone",
        "ukphone",
        "phone",
        "pron",
        "pronunciation",
        "ph",
        "ipa",
        "pinyin",
        "accent",
        "音标",
        "读音",
    ];
    const REL: &[&str] = &[
        "synonym",
        "synonyms",
        "syno",
        "syn",
        "antonym",
        "antonyms",
        "anto",
        "ant",
        "rel",
        "related",
        "relation",
        "similar",
        "同义词",
        "反义词",
        "相关词",
    ];
    if DEF.contains(&n.as_str()) {
        return KeyKind::Def;
    }
    if EX.contains(&n.as_str()) {
        return KeyKind::Example;
    }
    if PH.contains(&n.as_str()) {
        return KeyKind::Phonetic;
    }
    if REL.contains(&n.as_str()) {
        return KeyKind::Related;
    }
    // 第二档：**前缀 / 子串**匹配，覆盖 `phonetic_symbol`、`english_definitions`
    // 这类复合键名（各源命名习惯差别很大，光靠精确表覆盖不全）。
    //
    // 这里的关键是「宁可漏判，不可误判」：像 `default`、`define` 这种以
    // `def` 开头的键名极容易出现在配置类字段上，一旦误判成释义，界面就会
    // 冒出「zh-CN」「true」这种莫名其妙的"释义"。所以只对**语义明确**的
    // 长词根做子串匹配，绝不用 `def` / `ph` / `rel` 这类短词根去 contains。
    if n.contains("phonetic") || n.starts_with("pronunc") || n.contains("soundmark") {
        return KeyKind::Phonetic;
    }
    if n.contains("synonym") || n.contains("antonym") {
        return KeyKind::Related;
    }
    if n.contains("sentence") || n.contains("example") {
        return KeyKind::Example;
    }
    if n.contains("definition")
        || n.contains("meaning")
        || n.contains("translation")
        || n.contains("explanation")
    {
        return KeyKind::Def;
    }
    KeyKind::Other
}

/// 像不像音标。带 `/.../` 或 `[...]` 的强特征优先，其次看是否含 IPA 专有符号。
fn looks_like_phonetic(t: &str) -> bool {
    let n = t.chars().count();
    if n == 0 || n > 60 || t.contains(' ') {
        return false;
    }
    if n >= 3
        && ((t.starts_with('/') && t.ends_with('/')) || (t.starts_with('[') && t.ends_with(']')))
    {
        return true;
    }
    const IPA: &[char] = &[
        'ə', 'ɪ', 'ʊ', 'ɔ', 'ɑ', 'ʌ', 'ɜ', 'ʃ', 'ʒ', 'θ', 'ð', 'ŋ', 'æ', 'ɒ', 'ɹ', 'ɡ', 'ˈ', 'ˌ',
        'ː',
    ];
    t.chars().any(|c| IPA.contains(&c))
}

/// 像不像一个「释义」。返回优先级（越小越靠前），`None` 表示直接丢弃。
fn def_rank(t: &str) -> Option<u8> {
    let n = t.chars().count();
    if n == 0 || n > 300 {
        return None;
    }
    if t.starts_with("http") || t.contains("://") {
        return None; // 链接
    }
    if t.starts_with('{') || t.starts_with('[') {
        return None; // JSON 片段
    }
    if t.contains('<') || t.contains('>') {
        return None; // 没剥干净的标签
    }
    if !t.chars().any(|c| c.is_alphabetic()) {
        return None; // 纯数字 / 纯符号
    }
    if n > 40 && !t.contains(' ') && !t.chars().any(is_cjk) {
        return None; // 超长无空格的拉丁串：多半是 token / hash
    }
    Some(if n <= 60 { 0 } else { 1 })
}

/// 像不像一个「例句」（比释义长，通常是完整语句）。
fn is_sentence_like(t: &str) -> bool {
    let n = t.chars().count();
    if n < 6 || n > 260 {
        return false;
    }
    if t.contains("://") || t.contains('<') || t.contains('>') {
        return false;
    }
    t.contains(' ') || t.chars().any(is_cjk)
}

/// 启发式抽取的中间结果。
#[derive(Default)]
struct HeurCollect {
    /// (优先级, 原始顺序, 文本)——原始顺序用于稳定排序，保证同优先级时保留遍历顺序
    defs: Vec<(u8, usize, String)>,
    examples: Vec<String>,
    phonetics: Vec<String>,
    related: Vec<String>,
}

impl HeurCollect {
    fn is_empty(&self) -> bool {
        self.defs.is_empty()
            && self.examples.is_empty()
            && self.phonetics.is_empty()
            && self.related.is_empty()
    }
}

/// 递归遍历任意 JSON，按「键名归属 + 内容特征」收集可用信息。
///
/// 不用 `FieldMapping` 的原因：那条路要求字段名、层级完全对上，上游一改版就
/// 全盘失效。这里反过来——**不看结构看内容**：音标长得像 `/.../`，
/// 释义挂在 def/meaning/translation 这类键下，例句是较长的自然语句。
fn collect_value(v: &serde_json::Value, hint: KeyKind, depth: usize, lang: &str, out: &mut HeurCollect) {
    if depth > 8 {
        return;
    }
    match v {
        serde_json::Value::String(s) => {
            let t = strip_markup(s);
            if t.is_empty() {
                return;
            }
            match hint {
                KeyKind::Def => {
                    if let Some(r) = def_rank(&t) {
                        let seq = out.defs.len();
                        out.defs.push((r, seq, t));
                    }
                }
                KeyKind::Example => {
                    if is_sentence_like(&t) {
                        out.examples.push(t);
                    }
                }
                KeyKind::Phonetic => {
                    if looks_like_phonetic(&t) {
                        out.phonetics.push(t);
                    }
                }
                KeyKind::Related => {
                    out.related.extend(split_related(&t, lang));
                }
                KeyKind::Other => {
                    // 键名不认识：只认「强特征」的音标。
                    // 释义不给默认通道——否则整页导航、版权声明都会被当成释义。
                    if looks_like_phonetic(&t) {
                        out.phonetics.push(t);
                    }
                }
            }
        }
        serde_json::Value::Array(a) => {
            for it in a {
                collect_value(it, hint, depth + 1, lang, out);
            }
        }
        serde_json::Value::Object(o) => {
            for (k, val) in o {
                let kk = key_kind(k);
                // 子键不认识时继承父键的语义（`def: {"#text": "..."}` 这类要靠它）
                let kk = if kk == KeyKind::Other { hint } else { kk };
                collect_value(val, kk, depth + 1, lang, out);
            }
        }
        _ => {}
    }
}

/// 把启发式收集结果组装成词条。什么都没捞到时返回**不带标记**的空壳，
/// 让调用方知道「这个源确实没给出可用内容」。
fn build_heuristic_entry(
    mut c: HeurCollect,
    cfg: &DictSourceConfig,
    word: &str,
    lang: &str,
) -> WordEntry {
    let mut e = WordEntry::new(word);
    e.lang = lang.to_string();
    e.source = cfg.id.clone();

    if c.is_empty() {
        return e;
    }

    // 音标：优先带 /.../ 或 [...] 的强特征项，其余作为第二读音
    c.phonetics.dedup();
    if let Some(p) = c
        .phonetics
        .iter()
        .find(|p| p.starts_with('/') || p.starts_with('['))
        .cloned()
    {
        e.phonetic.uk = p;
        if let Some(second) = c.phonetics.iter().find(|p| **p != e.phonetic.uk).cloned() {
            e.phonetic.us = second;
        }
    } else if let Some(p) = c.phonetics.first().cloned() {
        e.phonetic.uk = p;
    }

    // 释义：短而像词典释义的排前面，最多留 8 条
    c.defs
        .sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    c.defs.dedup_by(|a, b| a.2 == b.2);
    for (_, _, d) in c.defs.iter().take(8) {
        e.senses.push(Sense {
            pos: String::new(),
            definition: d.clone(),
            examples: Vec::new(),
        });
    }

    // 例句：最多 3 条，全挂到第一个义项上（没有义项就丢弃，避免「有例句没释义」的怪状态）
    if !e.senses.is_empty() {
        let ex: Vec<Example> = c
            .examples
            .iter()
            .take(3)
            .map(|t| Example {
                text: t.clone(),
                translation: String::new(),
            })
            .collect();
        if !ex.is_empty() {
            e.senses[0].examples = ex;
        }
    }

    // 相关词去重
    let mut seen = std::collections::BTreeSet::new();
    for w in c.related {
        if w != word && seen.insert(w.clone()) {
            e.related.push(w);
        }
    }
    e.related.truncate(20);

    // 变形兜底：只有英语有规则推导，且没有释义时不必推（推出来也没地方展示）
    if e.inflections.is_empty() && lang == "en" && !e.senses.is_empty() {
        e.inflections = crate::dict::inflect::derive(&e.word, "", &[]);
    }

    e.extra
        .insert(HEURISTIC_TAG.to_string(), serde_json::json!("1"));
    e
}

/// 启发式解析：`mapping`（匹配式）对不上时，尽力从任意 JSON 里捞东西。
pub fn normalize_heuristic(
    raw: &serde_json::Value,
    cfg: &DictSourceConfig,
    word: &str,
    lang: &str,
) -> WordEntry {
    let mut c = HeurCollect::default();
    collect_value(raw, KeyKind::Other, 0, lang, &mut c);
    build_heuristic_entry(c, cfg, word, lang)
}

/// ASCII 大小写不敏感的子串查找，返回**原串**里的字节下标。
///
/// 为什么不用 `to_lowercase()` 后再 `find()`：`to_lowercase` 可能改变字节长度
/// （如 `İ` → `i̇`），拿小写副本里的下标去切原串会切在 UTF-8 字符中间直接 panic。
/// HTML 标签名都是 ASCII，逐字节比较既能大小写不敏感，又保证下标落点合法。
fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() || from > h.len() - n.len() {
        return None;
    }
    let mut i = from;
    while i + n.len() <= h.len() {
        if h[i..i + n.len()].eq_ignore_ascii_case(n) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 剥掉 `<script>` / `<style>` 这类**块级**标签及其内容。
fn strip_tag_blocks(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript", "svg", "template"] {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        let mut cursor = 0usize;
        loop {
            let Some(a) = find_ci(&s, &open, cursor) else { break };
            let Some(b) = find_ci(&s, &close, a) else {
                s.truncate(a);
                break;
            };
            let end = b + close.len();
            s.replace_range(a..end, " ");
            cursor = a + 1;
        }
    }
    s
}

/// 这些标签之后应该换行，避免把整页文字粘成一行。
fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "br" | "p"
            | "div"
            | "li"
            | "ul"
            | "ol"
            | "tr"
            | "td"
            | "th"
            | "dd"
            | "dt"
            | "dl"
            | "hr"
            | "pre"
            | "table"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "blockquote"
            | "figcaption"
            | "caption"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    )
}

/// 剥掉剩下的所有标签，并把块级标签换成换行，尽量保住「一行一条」的结构。
fn strip_tags_keep_lines(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut buf = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                buf.clear();
            }
            '>' => {
                in_tag = false;
                // 取标签名：去掉收尾斜杠与属性（`</div class="x">` 里也可能带属性）
                let name = buf
                    .trim()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/')
                    .to_ascii_lowercase();
                if is_block_tag(&name) {
                    out.push('\n');
                }
                buf.clear();
            }
            _ if in_tag => buf.push(ch),
            _ => out.push(ch),
        }
    }
    out
}

/// 解开常见 HTML 实体。不做完整实体表——词典页面里出现的就这么几个。
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == '&' {
            let mut j = i + 1;
            while j < bytes.len() && j - i <= 12 && bytes[j] != ';' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == ';' {
                let ent: String = bytes[i + 1..j].iter().collect();
                let rep = match ent.as_str() {
                    "nbsp" => Some(' '),
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    _ => ent
                        .strip_prefix('#')
                        .and_then(|d| d.parse::<u32>().ok())
                        .and_then(char::from_u32),
                };
                if let Some(c) = rep {
                    out.push(c);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 把 HTML（或半 HTML）响应体粗略转成纯文本，一行一条。
///
/// 不做真正的 DOM 解析：词典页面结构千差万别、还天天改版，目标只是
/// 「把标签和脚本扔掉，剩下的按行分析」，把这份不确定性交给上层的内容特征判定。
pub fn html_to_text(html: &str) -> String {
    decode_entities(&strip_tags_keep_lines(&strip_tag_blocks(html)))
}

/// 页面样板文字（导航、版权、按钮）。这些在**任何**页面上都有，
/// 一旦被当成释义，用户会看到「苹果 = Privacy Policy」这种荒唐结果。
///
/// 单词类条目要求「整行完全相等」，多词短语 / 中文短语才允许 `contains`：
/// 否则 `help`（帮助）这种正经词条的释义会被误杀。
const BOILERPLATE: &[&str] = &[
    "privacy policy",
    "privacy",
    "terms of service",
    "terms of use",
    "terms",
    "cookie policy",
    "cookies",
    "all rights reserved",
    "copyright",
    "sign in",
    "sign up",
    "signin",
    "signup",
    "log in",
    "login",
    "logout",
    "register",
    "subscribe",
    "newsletter",
    "follow us",
    "contact us",
    "about us",
    "share this",
    "advertisement",
    "ad choices",
    "please enable javascript",
    "verify you are human",
    "page not found",
    "access denied",
    "sitemap",
    "feedback",
    "无权限",
    "版权所有",
    "隐私政策",
    "服务条款",
    "联系我们",
    "关于我们",
    "免责声明",
    "意见反馈",
    "广告",
];

fn is_boilerplate(t: &str) -> bool {
    let l = t.trim().to_lowercase();
    let l = l.trim_end_matches(|c: char| {
        matches!(c, '.' | '!' | '。' | '！' | ':' | '：' | ';' | '；' | '-' | '|')
    });
    BOILERPLATE.iter().any(|b| {
        if b.contains(' ') || b.chars().any(|c| !c.is_ascii()) {
            l.contains(b)
        } else {
            l == *b
        }
    })
}

/// 纯文本 / HTML 响应的启发式抽取。
///
/// 有些词典源压根不返回 JSON，直接吐一个 HTML 页面。以前这种响应会被
/// `get_json` 判为「不是合法 JSON」整条丢弃，用户看到的就是「连得上却查不出」。
///
/// 但这条退路必须**保守**：纯文本没有键名可依据，任何一行都可能只是页面导航。
/// 所以加了两道闸：
/// 1. **整页必须出现查询词**——词典页必然在标题/正文里写出这个词；
///    而通用错误页、登录页不会，直接挡掉大半个误判面；
/// 2. 样板文字（导航/版权/按钮）一律丢弃。
///
/// 仍然捞错也不怕：结果会带 `HEURISTIC_TAG` 标记被标成「降级」，前端会如实提示。
pub fn normalize_from_text(body: &str, cfg: &DictSourceConfig, word: &str, lang: &str) -> WordEntry {
    let text = html_to_text(body);

    // 闸门 1：整页必须提到查询词
    let w = word.trim().to_lowercase();
    if w.is_empty() || !text.to_lowercase().contains(&w) {
        return build_heuristic_entry(HeurCollect::default(), cfg, word, lang);
    }

    let mut c = HeurCollect::default();
    for line in text.lines() {
        let t = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() || is_boilerplate(&t) {
            continue;
        }
        if looks_like_phonetic(&t) {
            c.phonetics.push(t);
            continue;
        }
        if let Some(r) = def_rank(&t) {
            if t.chars().count() > 60 && is_sentence_like(&t) {
                c.examples.push(t);
            } else {
                let seq = c.defs.len();
                c.defs.push((r, seq, t));
            }
        }
    }
    build_heuristic_entry(c, cfg, word, lang)
}

/// 「匹配式 → 启发式 → 纯文本」三级解析阶梯。
///
/// 拿到响应体后**不能**只试一次映射就判死：映射失配只是「我不认识这个结构」，
/// 不等于「这个源没有结果」。三级都拿不到才算这个源真的没结果。
fn pick_entry(
    json: Option<&serde_json::Value>,
    body: Option<&str>,
    cfg: &DictSourceConfig,
    word: &str,
    lang: &str,
) -> Option<WordEntry> {
    if let Some(v) = json {
        let strict = normalize(v, cfg, word, lang);
        if is_meaningful(&strict) {
            return Some(strict);
        }
        let loose = normalize_heuristic(v, cfg, word, lang);
        if is_presentable(&loose) {
            return Some(loose);
        }
    }
    if let Some(b) = body {
        let loose = normalize_from_text(b, cfg, word, lang);
        if is_presentable(&loose) {
            return Some(loose);
        }
    }
    None
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

    // POST：请求体是 JSON，响应基本也是 JSON，不做文本兜底
    if cfg.method.eq_ignore_ascii_case("POST") {
        // 目标语言：非中文的源语言统一翻译为中文，中文则翻译为英文
        let target = if lang == "zh" { "en" } else { "zh" };
        let resp = net::post_json(
            client, &url, &cfg.headers, &cfg.api_key, word, lang, target, timeout_secs,
        )
        .await;
        let elapsed = started.elapsed().as_millis() as i64;
        return match resp {
            Ok(v) => match pick_entry(Some(&v), None, cfg, word, lang) {
                Some(e) => (Some(e), String::new(), elapsed),
                None => (None, "返回结构无法解析出有效释义".into(), elapsed),
            },
            Err(e) => (None, e.to_string(), elapsed),
        };
    }

    // GET：用「JSON 或原文」版本取回，以便对 HTML / 半 JSON 响应做启发式兜底
    let resp = net::get_json_or_text(
        client,
        &url,
        &cfg.headers,
        &cfg.api_key,
        timeout_secs,
        retries,
    )
    .await;
    let elapsed = started.elapsed().as_millis() as i64;
    match resp {
        Ok((json, body)) => match pick_entry(json.as_ref(), Some(&body), cfg, word, lang) {
            Some(e) => (Some(e), String::new(), elapsed),
            None => (None, "返回结构无法解析出有效释义".into(), elapsed),
        },
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
                // ★ 用 `merge_from_source` 而不是 `merge_from`：后者对 senses
                // 是「先到先得」，会把慢一步返回的**另一种语言**的释义整段丢掉。
                // 现场后果就是「查英文词永远只有中文释义、没有英文解释」。
                match &mut merged {
                    Some(m) => m.merge_from_source(e),
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

    // 合并顺序是按「源优先级」走的，而优先级最高的恰是英英源，
    // 于是英文释义会排在中文前面。而结果卡 / 详情卡 / 学习卡片的默认释义
    // 都假设「第一条是母语释义」，这里统一纠正回来。
    if let Some(m) = merged.as_mut() {
        m.order_senses_local_first();
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
    //
    // ★ 命中**不等于**万事大吉：导入的词库实测有一半词条没有音标
    //   （9908 行里 4993 行 phonetic 全空），而命中后不再走在线源，
    //   这些词的音标就永远缺着 —— 界面上只剩一个孤零零的喇叭按钮。
    //   所以命中后先做一次「尽力而为」的音标补全（见
    //   [`enrich_wordbook_phonetic`]），补上了就回写词库，
    //   同一个词一生只发一次请求。
    if let Ok(Some(mut e)) = db.get_word(&key, lang) {
        enrich_wordbook_phonetic(db, client, net_cfg, &mut e).await;
        return Ok(LookupResult {
            word: e.word.clone(),
            lang: e.lang.clone(),
            entry: e,
            sources: vec!["本地词库".into()],
            from_cache: true,
            from_llm: false,
            trace: vec![],
            lang_note: None,
            note: None,
            degraded: false,
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
            lang_note: None,
            note: None,
            degraded: false,
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
        // 启发式解析出来的词条要如实标记为「降级」，前端据此提示数据来源
        let degraded = entry.extra.contains_key(HEURISTIC_TAG);
        // 降级结果**不写 7 天缓存**：启发式靠猜，猜错一次就会把这个错结果
        // 固定下来一周，用户改都改不掉。宁可每次重新解析（很便宜）。
        if !degraded {
            db.cache_entry(&entry, &srcs.join(","), now).ok();
        }
        return Ok(LookupResult {
            word: entry.word.clone(),
            lang: entry.lang.clone(),
            entry,
            sources: srcs,
            from_cache: false,
            from_llm: false,
            trace,
            lang_note: None,
            note: degraded.then(|| {
                "该词的释义由启发式解析得到（词典源返回结构与预设映射不一致），字段可能不完整。"
                    .to_string()
            }),
            degraded,
        });
    }

    // 4) 本地大模型兜底（可选，离线也能用）
    //
    // ★ 关键设计（用户需求）：**AI 是否可用，绝不能决定这次查询的成败**。
    //
    // 本地大模型只是「前面三层都没结果时的最后一根稻草」。它没启用、超时、
    // 返回空，都只意味着「这次没有 AI 兜底」，而不是「查询失败」。
    // 改之前这里各分支直接 `bail!`，于是用户被明确告知
    // 「请启动 LM Studio」—— 明明词典都连得上，界面却写「查询失败」，
    // 把归因彻底甩给了 AI。
    //
    // 现在：兜底失败只记一句 `llm_note`，最终失败文案一律归因于**词典源**。
    let llm_note: Option<String> = if allow_llm {
        const LLM_FALLBACK_TIMEOUT_SECS: u64 = 45;
        // 本地模型的超时是 `llm.timeout_secs`（默认 120 秒）。在线词典全挂时
        // 界面会在这段时间里一直转圈、毫无进展提示，看起来就像「卡死了」。
        // 给兜底单独压一个上限。
        let r = match tokio::time::timeout(
            std::time::Duration::from_secs(LLM_FALLBACK_TIMEOUT_SECS),
            llm_fallback,
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(anyhow::anyhow!(
                "本地大模型 {} 秒内未返回结果",
                LLM_FALLBACK_TIMEOUT_SECS
            )),
        };
        match r {
            Ok(mut entry) => {
                entry.word = if entry.word.trim().is_empty() {
                    key.clone()
                } else {
                    entry.word
                };
                entry.lang = lang.to_string();
                entry.source = "lmstudio".into();
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
                        lang_note: None,
                        note: None,
                        degraded: false,
                    });
                }
                Some("本地大模型返回内容为空，已忽略这次兜底。".to_string())
            }
            Err(e) => Some(format!("本地大模型未参与本次查询（{}）。", e)),
        }
    } else {
        Some("本地大模型未启用，本次未使用 AI 兜底。".to_string())
    };

    // ---- 走到这里：本地词库 / 词典缓存 / 在线词典 / AI 兜底 全都没给出结果 ----
    //
    // 这才是真正的「查不到」。文案要点：
    //   1. 主语是**词典源**，因为用户能看到「很多词典都连上了」，得解释清是哪种情况；
    //   2. AI 只作为**可选补充**出现在括号里，不构成失败原因；
    //   3. 附上诊断摘要（全超时 / 全解析失败 / 没启用源），省得用户瞎猜。
    let ok_cnt = trace.iter().filter(|t| t.ok).count();
    let parse_cnt = trace
        .iter()
        .filter(|t| !t.ok && (t.error.contains("解析") || t.error.contains("有效结果")))
        .count();
    let hint = if trace.is_empty() {
        "当前没有启用支持该语言的词典源，可到「设置 → 词典源」启用".to_string()
    } else if ok_cnt > 0 {
        format!("{ok_cnt} 个词典源有响应，但都未给出可用释义")
    } else if parse_cnt > 0 {
        format!("{parse_cnt} 个词典源有响应但未解析出释义（返回结构与预设映射不一致）")
    } else {
        "各词典源均未在超时时间内响应，可到「设置 → 网络与代理」检测网络 / 配置代理".to_string()
    };

    let mut msg = format!("未找到「{word}」的释义：{hint}。");
    if let Some(n) = &llm_note {
        msg.push_str(&format!("（{n}）"));
    }
    msg.push_str(" 可点击「AI 讲解」让本地模型直接讲解这个词。");
    anyhow::bail!(msg)
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

/// 哪些语言的词库词条值得补音标。
///
/// 有道的 `basic.phonetic` 对拉丁语言给的是 IPA 音标（正是缺的那块）；
/// 对中/日/韩给的是拼音/假名/罗马音，语义不同且这些语言大多有自己的
/// 读音字段，先不混进来。
fn phonetic_gap_lang(lang: &str) -> bool {
    matches!(
        lang,
        "en" | "fr" | "de" | "es" | "ru" | "pt" | "it"
    )
}

/// 词库词条缺音标时的在线补全（尽力而为，任何失败都静默跳过）。
///
/// 为什么放这里：词库命中后 [`lookup_with_cache`] 不再走在线源，
/// 导入词条里那 49% 没有音标的词就永远缺着 —— 用户看到的查词结果
/// 只剩一个喇叭按钮。补全成功就**回写词库**（[`crate::db::Db::update_word_phonetic`]），
/// 同一个词一生只发一次请求。
///
/// 三条保险，保证它永远不会变成「查词变慢」的元凶：
///   1. 有道内部的限频检查是**直接拒绝**而不是等待 —— 撞上间隔就放弃，
///      后台增强引擎或下次查询自然会再补；
///   2. 外面再套一个 2 秒硬超时 —— 网络不通时宁可这条词没音标，
///      也不能让「本地词库秒回」退化成「等一个注定失败的 HTTP」；
///   3. 任何错误都被吞掉 —— 音标是增强信息，主结果不受影响。
async fn enrich_wordbook_phonetic(
    db: &Db,
    client: &reqwest::Client,
    net: &crate::models::NetworkConfig,
    e: &mut WordEntry,
) {
    if !e.phonetic.uk.trim().is_empty() || !e.phonetic.us.trim().is_empty() {
        return; // 已有音标，无事可做
    }
    if !phonetic_gap_lang(&e.lang) || e.word.trim().is_empty() {
        return;
    }
    let fut = crate::translate::youdao_translate(client, net, &e.lang, "zh", &e.word);
    let Ok(res) = tokio::time::timeout(std::time::Duration::from_secs(2), fut).await else {
        return;
    };
    let Ok(res) = res else { return };
    let Some(b) = res.dict else { return };
    let ph = b.phonetic.trim();
    if ph.is_empty() {
        return;
    }
    // 有道这里只给一个音标（不分英美）→ 放进 us 位，
    // 与「词典源音标大多先给美音」的既有呈现一致
    e.phonetic.us = ph.to_string();
    // 回写失败无妨：本次返回的 entry 已经带上了音标
    let _ = db.update_word_phonetic(&e.word, &e.lang, "", ph, "");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::builtin;
    use crate::models::FieldMapping;

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

    /// 脚本排他性：纯拉丁字母的词不可能是中日韩俄等语言。
    ///
    /// 这是「reality 被识别成中文」的守门测试 —— 方向选成中文后输入英文词，
    /// 若没有这条规则就会去问《现代汉语规范词典》，释义落空、音标被标成「拼音」。
    #[test]
    fn latin_only_rejects_non_latin_scripts() {
        // 拉丁字母（含变音符）→ true
        assert!(is_latin_only("reality"));
        assert!(is_latin_only("apple"));
        assert!(is_latin_only("café"));
        assert!(is_latin_only("naïve"));
        assert!(is_latin_only("don't"));       // 撇号不影响判定
        assert!(is_latin_only("hello world")); // 空格同理
        assert!(is_latin_only("well-known"));

        // 非拉丁书写系统 → false
        assert!(!is_latin_only("开心"));
        assert!(!is_latin_only("たべる"));
        assert!(!is_latin_only("カタカナ"));
        assert!(!is_latin_only("먹다"));
        assert!(!is_latin_only("привет"));
        assert!(!is_latin_only("مرحبا"));
        assert!(!is_latin_only("สวัสดี"));
        assert!(!is_latin_only("γράμμα"));

        // 一个字母都没有 → 无从判断，不能当成拉丁词
        assert!(!is_latin_only("123"));
        assert!(!is_latin_only(""));
        assert!(!is_latin_only("---"));
    }

    /// 非拉丁语言表：只拦「字形上不可能」的语言，拉丁语言之间不拦。
    #[test]
    fn non_latin_lang_table_is_exclusive_only_where_provable() {
        for l in ["zh", "ja", "ko", "ru", "ar", "hi", "th", "el", "he"] {
            assert!(is_non_latin_lang(l), "{} 是非拉丁书写系统，应判为互斥", l);
        }
        // 英/法/德/西 等都是拉丁字母，无法靠字形排除，必须交回用户选择
        for l in ["en", "fr", "de", "es", "it", "pt", "vi", "tr", "nl"] {
            assert!(!is_non_latin_lang(l), "{} 是拉丁书写系统，不该被拦截", l);
        }
        // 大小写与区域码
        assert!(is_non_latin_lang("ZH"));
        assert!(is_non_latin_lang("zh-CN"));
        assert!(is_non_latin_lang("zh_TW"));
        assert!(!is_non_latin_lang(""));
    }

    /// 两个判定组合起来，才构成「reality + 中文方向 → 纠正为英语」的依据。
    #[test]
    fn latin_word_conflicts_with_chinese_direction() {
        // 会触发纠正：用户选了中文，但词是纯拉丁字母
        assert!(is_non_latin_lang("zh") && is_latin_only("reality"));
        // 不触发：词本身是中文
        assert!(!(is_non_latin_lang("zh") && is_latin_only("现实")));
        // 不触发：用户选法语时不纠正 —— 拉丁语言之间字形无法区分
        assert!(!(is_non_latin_lang("fr") && is_latin_only("realite")));
    }

    /// 源代码里 `sourceLabel` 的 id 映射键必须与内置源的 id 一一对应，
    /// 否则界面上会直接露出 `youdao-newhh` 这种原始 id。
    /// 这里只做「内置源 id 有中文名可查」的守门（前端映射表见 ui.js）。
    #[test]
    fn builtin_source_ids_are_stable() {
        let ids: Vec<String> = builtin::default_sources().iter().map(|s| s.id.clone()).collect();
        for expect in ["free-dictionary", "wiktionary", "youdao-suggest",
                       "youdao-jsonapi", "youdao-newhh"] {
            assert!(ids.iter().any(|i| i == expect), "内置源 id 变了：{}", expect);
        }
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

    /// 相关词必须**逐个拆开**：词典源常把整串同义词塞在一个元素里，
    /// 不拆的话前端只画一个大 chip，点下去拿整串查词必然查不到。
    #[test]
    fn related_words_are_split_apart() {
        assert_eq!(split_related("desert, abandon, leave", "en"),
                   vec!["desert", "abandon", "leave"]);
        // 空格连排也要拆
        assert_eq!(split_related("desert abandon", "en"),
                   vec!["desert", "abandon"]);
        // 中文顿号 / 分号
        assert_eq!(split_related("高兴、愉快；快乐", "zh"),
                   vec!["高兴", "愉快", "快乐"]);
        // 噪声要滤掉
        assert_eq!(split_related("n. (见 also) 放弃", "zh"), Vec::<String>::new());
        // 单元素数组也不能原样当一个词
        assert_eq!(split_related("run|running", "en"), vec!["run", "running"]);
    }

    // ================================================================
    // 查词判定：AI 可用性不得决定成败（用户需求 D）
    // ================================================================

    /// 本地词库命中时，**即使本地大模型完全不可用**，也必须返回「查询成功」。
    ///
    /// 回归保护：改之前第 ④ 步（AI 兜底）失败会 `anyhow::bail!` 掉整条查询，
    /// 前端于是显示「查询失败，请启动 LM Studio」—— 可用户明明有本地词条，
    /// 而且明明「很多词典都连得上」。归因被甩给了 AI，这就是用户报的问题。
    #[tokio::test]
    async fn ai_being_off_cannot_turn_a_wordbook_hit_into_a_failure() {
        let p = std::env::temp_dir().join(format!("wordwise-dict-ai-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let db = Db::open(&p).expect("打开测试库");

        let mut e = WordEntry::new("apple");
        e.lang = "en".into();
        e.senses.push(Sense {
            pos: "n.".into(),
            definition: "苹果".into(),
            examples: vec![],
        });
        db.upsert_word(&e, 0).expect("写入本地词条");

        let client = reqwest::Client::new();
        let net_cfg = crate::models::NetworkConfig::default();
        // AI 兜底明确失败（等价于「没开 LM Studio」），且不给任何在线源
        let r = lookup_with_cache(
            &db,
            "Apple", // 注意大写：还要顺带验证小写归一化能命中
            "en",
            &[],
            &client,
            3600,
            true,
            &net_cfg,
            false,
            async { anyhow::bail!("未启用本地大模型兜底") },
        )
        .await
        .expect("词库命中就应当成功，AI 不可用不算失败");

        assert_eq!(r.sources, vec!["本地词库".to_string()]);
        assert!(!r.from_llm);
        assert!(!r.degraded, "词库命中不该被标成降级");
        assert!(r.note.is_none(), "词库命中不该冒出 AI 相关提示");
    }

    /// 全链路都没结果时，报错文案必须**归因于词典源**，不能把锅甩给 AI。
    #[tokio::test]
    async fn total_miss_blames_dict_sources_not_the_ai() {
        let p = std::env::temp_dir().join(format!("wordwise-dict-miss-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let db = Db::open(&p).expect("打开测试库");
        let client = reqwest::Client::new();
        let net_cfg = crate::models::NetworkConfig::default();

        let err = lookup_with_cache(
            &db,
            "zzzznotaword",
            "en",
            &[], // 没有任何源 → trace 为空 → 提示「没启用源」
            &client,
            3600,
            true,
            &net_cfg,
            false,
            async { anyhow::bail!("连不上本地服务") },
        )
        .await
        .expect_err("确实查不到时应当返回 Err");
        let msg = err.to_string();

        assert!(msg.contains("词典源"), "失败说明应当以词典源为主语：{msg}");
        assert!(
            !msg.contains("请检查网络或启动 LM Studio"),
            "不该把失败归因成「没启动 AI 服务器」：{msg}"
        );
        // AI 只能作为「可选补充」出现在括号里
        assert!(msg.contains("本地大模型未参与本次查询"), "应当如实说明 AI 未参与：{msg}");
    }

    // ================================================================
    // 查词升级：匹配式 + 启发式双策略
    // ================================================================

    /// 结构认得（mapping 对得上）时走匹配式，不能被启发式抢走，也不该标降级。
    #[test]
    fn strict_mapping_wins_over_heuristic() {
        let raw = serde_json::json!({
            "query": "apple",
            "data": {"entries": [{"type": "n.", "explain": "苹果"}]}
        });
        let cfg = builtin::youdao_suggest();
        let e = pick_entry(Some(&raw), Some("{}"), &cfg, "apple", "en").expect("应当解析出词条");
        assert_eq!(e.senses[0].definition, "苹果");
        assert!(
            !e.extra.contains_key(HEURISTIC_TAG),
            "匹配式命中不该被标记为降级"
        );
    }

    /// 上游改了字段名、mapping 完全对不上时，启发式要能捞出释义，而不是判失败。
    #[test]
    fn heuristic_rescues_unknown_shape() {
        // 字段名全是这套源**从未见过**的：只能靠「内容长得像什么」判断
        let raw = serde_json::json!({
            "result": {
                "head": {"phonetic_symbol": "/ˈæp.əl/"},
                "glossary": [
                    {"definition": "a round fruit with red or green skin"},
                    {"definition": "苹果"}
                ],
                "sample_sentences": ["I ate an apple."]
            }
        });
        // mapping 全空 = 模拟「配置完全没覆盖这个结构」
        let cfg = DictSourceConfig {
            mapping: FieldMapping::default(),
            ..builtin::youdao_suggest()
        };

        // 先确认匹配式**确实**拿不到东西（否则这个测试就没意义了）
        assert!(
            !is_meaningful(&normalize(&raw, &cfg, "apple", "en")),
            "前提：匹配式在这个结构上应当失配"
        );

        let e = pick_entry(Some(&raw), None, &cfg, "apple", "en").expect("启发式应当捞出内容");
        assert!(e.senses.len() >= 2, "应当抽到两条释义：{:?}", e.senses);
        assert!(e.senses.iter().any(|s| s.definition.contains("苹果")));
        assert_eq!(e.phonetic.uk, "/ˈæp.əl/");
        assert_eq!(e.senses[0].examples.len(), 1);
        assert!(
            e.extra.contains_key(HEURISTIC_TAG),
            "启发式命中必须标记为降级"
        );
        assert!(is_presentable(&e));
    }

    /// 源直接返回 HTML 页面（不是 JSON）时，也要能从纯文本里捞出释义。
    #[test]
    fn html_response_yields_definitions_instead_of_failure() {
        let html = r#"<html><head><title>apple</title>
        <style>.a{color:red}</style></head>
        <body>
          <div class="pr">/ˈæp.əl/</div>
          <ul><li>a round fruit with red or green skin</li><li>苹果</li></ul>
          <script>var x = "not a definition at all";</script>
        </body></html>"#;
        let cfg = builtin::youdao_suggest();

        // json=None 表示响应体不是合法 JSON（get_json_or_text 的 (None, body) 分支）
        let e = pick_entry(None, Some(html), &cfg, "apple", "en").expect("文本兜底应当捞出内容");
        assert!(
            e.senses.iter().any(|s| s.definition.contains("苹果")),
            "应当从 <li> 里抽出释义：{:?}",
            e.senses
        );
        assert_eq!(e.phonetic.uk, "/ˈæp.əl/");
        assert!(is_presentable(&e));
        assert!(
            !e.senses.iter().any(|s| s.definition.contains("not a definition")),
            "script/style 里的内容必须被丢掉"
        );
    }

    /// 页面导航、版权声明这类噪声不能被当成释义（启发式的最大风险点）。
    #[test]
    fn heuristic_rejects_navigation_noise() {
        let raw = serde_json::json!({
            "nav": {"items": ["Privacy Policy", "Terms of Service", "About Us"]},
            "text": "All rights reserved.",
            "value": true,
            "defaultLocale": "zh-CN"
        });
        let cfg = DictSourceConfig {
            mapping: FieldMapping::default(),
            ..builtin::youdao_suggest()
        };
        let e = normalize_heuristic(&raw, &cfg, "apple", "en");
        assert!(e.senses.is_empty(), "导航文字不该变成释义：{:?}", e.senses);
        assert!(!is_presentable(&e), "什么都没捞到时不该声称有内容");

        // 同理，纯 HTML 页面里的噪声也不该混进来
        let t = normalize_from_text("<p>Privacy Policy</p><p>All rights reserved.</p>", &cfg, "apple", "en");
        assert!(!is_presentable(&t), "噪声页不该被判为「有内容」：{:?}", t.senses);

        // 闸门：页面根本没提这个词，整页都不可信
        let t2 = normalize_from_text("<p>a round fruit with red skin</p>", &cfg, "apple", "en");
        assert!(!is_presentable(&t2), "整页没提到 apple，不该当成 apple 的释义");

        // 提到词、但样板文字仍要被剥掉
        let t3 = normalize_from_text(
            "<h1>apple</h1><p>Privacy Policy</p><p>苹果</p>",
            &cfg,
            "apple",
            "en",
        );
        assert!(t3.senses.iter().any(|s| s.definition.contains("苹果")));
        assert!(!t3.senses.iter().any(|s| s.definition.contains("Privacy")));
    }

    /// `is_presentable` 比 `is_meaningful` 松一档：只有音标也算「有内容」。
    #[test]
    fn presentable_is_looser_than_meaningful() {
        let mut e = WordEntry::new("apple");
        assert!(!is_meaningful(&e) && !is_presentable(&e));

        // 只捞到音标：够「可展示」，但够不上「有释义」
        e.phonetic.uk = "/ˈæp.əl/".into();
        assert!(is_meaningful(&e), "音标本身就是 is_meaningful 的合格线之一");

        // 只有记忆法：不算 meaningful，但算 presentable
        let mut m = WordEntry::new("apple");
        m.mnemonic = "a + pple".into();
        assert!(!is_meaningful(&m));
        assert!(is_presentable(&m));

        // 空壳 + 只有 raw：两者都不算
        let mut raw_only = WordEntry::new("apple");
        raw_only
            .extra
            .insert("raw".into(), serde_json::json!("{}"));
        assert!(!is_presentable(&raw_only));
    }

    /// 音标识别：只看「长得像不像」，不依赖字段名。
    #[test]
    fn phonetic_shape_detection() {
        assert!(looks_like_phonetic("/ˈæp.əl/"));
        assert!(looks_like_phonetic("[ə'plaʊd]"));
        // 裸音标靠 IPA 专有符号，也能认出来
        assert!(looks_like_phonetic("ˈæpəl"));
        // 普通词、超长串、带空格的都不算
        assert!(!looks_like_phonetic("apple"));
        assert!(!looks_like_phonetic("kāi xīn"));
        assert!(!looks_like_phonetic("/very long string with spaces inside/"));
    }

    /// `defaultLocale` 这类以 `def` 开头的键名不能被误判成释义。
    #[test]
    fn key_kind_does_not_overmatch_short_roots() {
        assert_eq!(key_kind("def"), KeyKind::Def);
        assert_eq!(key_kind("english_definitions"), KeyKind::Def);
        assert_eq!(key_kind("phonetic_symbol"), KeyKind::Phonetic);
        assert_eq!(key_kind("sample_sentences"), KeyKind::Example);
        assert_eq!(key_kind("synonyms"), KeyKind::Related);
        // 危险项：default / define / ph 之外的短词根不参与子串匹配
        assert_eq!(key_kind("defaultLocale"), KeyKind::Other);
        assert_eq!(key_kind("phone_number"), KeyKind::Other);
        assert_eq!(key_kind("nav"), KeyKind::Other);
    }

    /// HTML 剥标签不能把整页文字粘成一行，也不能切坏 UTF-8。
    #[test]
    fn html_to_text_keeps_line_structure() {
        let t = html_to_text("<div>第一行</div><div>第二行</div>");
        assert!(t.contains("第一行"));
        assert!(t.contains('\n'), "块级标签应当产生换行：{t:?}");

        // 大小写混写的 script 也要丢掉
        let t2 = html_to_text("<SCRIPT>x=1</SCRIPT>after");
        assert!(!t2.contains("x=1"));
        assert!(t2.contains("after"));

        // 实体解码
        assert_eq!(html_to_text("<p>a&nbsp;b &amp; c</p>").trim(), "a b & c");
    }
}
