//! 知识图谱：词与词的关系网络（需求：独立于查词/翻译之外的「知识图谱」栏目）。
//!
//! ## 关系数据从哪来
//!
//! 分两层，**本地优先、AI 按需补充**：
//!
//! ```text
//!   1. 本地抽取（零算力、零联网、立刻可用）
//!      WordEntry.related      → 相关词 / 同义 / 反义
//!      WordEntry.inflections  → 派生（词形变化）
//!   2. AI 发散（用户点「AI 发散」才跑，结果落库缓存）
//!      同义 / 反义 / 上义 / 下义 / 常见搭配
//! ```
//!
//! 为什么不做「导入时全量 AI 发散」：一个几千词的词库要跑几千次模型调用，
//! 低性能设备上能跑几个小时，而用户往往只看其中几十个词。按需发散 + 落库
//! 缓存，把算力花在真正被看过的词上。
//!
//! ## 边为什么要有 source 字段
//!
//! 本地抽的边是**确定的**（词典里写着），AI 发的边是**推测的**。前端用不同
//! 线型区分（实线 / 虚线），用户一眼能看出哪条是模型猜的，不会被误导。

use crate::models::{LlmConfig, WordEntry};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// 关系类型 → 中文名。前端图例与筛选列表直接用它，保证前后端同一份。
pub const RELS: [(&str, &str); 6] = [
    ("synonym", "同义"),
    ("antonym", "反义"),
    ("derived", "派生"),
    ("related", "相关"),
    ("hypernym", "上义"),
    ("hyponym", "下义"),
];

/// 关系码 → 中文名；未知关系原样返回，避免前端出现空白图例。
pub fn rel_label(rel: &str) -> String {
    RELS.iter()
        .find(|(c, _)| *c == rel)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| rel.to_string())
}

/// 全部关系码（供前端做筛选器）。
pub fn all_rels() -> Vec<serde_json::Value> {
    RELS.iter()
        .map(|(c, n)| serde_json::json!({ "code": c, "name": n }))
        .collect()
}

/// 图谱节点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub word: String,
    pub lang: String,
    /// 连接数（度数）。前端按它决定圆的半径 —— 连接越多越核心。
    #[serde(default)]
    pub degree: i32,
    /// 词库里是否有完整词条：决定点击节点能不能直接打开详情卡。
    /// `false` 时前端把节点画成空心，点击改为「查词」。
    #[serde(default)]
    pub in_dict: bool,
    /// 一句话释义，用于悬停提示。
    #[serde(default)]
    pub gloss: String,
    /// 掌握度 0~1（有学习记录时），前端按它染色。
    #[serde(default)]
    pub mastery: Option<f64>,
}

/// 图谱边。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub src: String,
    pub dst: String,
    /// 关系码，见 [`RELS`]
    pub rel: String,
    #[serde(default)]
    pub weight: f64,
    /// `local` = 词典里明确写的；`llm` = 模型推测的
    #[serde(default)]
    pub source: String,
}

/// 一张子图。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphData {
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// 中心词（环形布局用；全局图时为 None）
    #[serde(default)]
    pub center: Option<String>,
    /// 该语言下全库的节点/边总数，前端用来提示「当前显示的是局部」
    #[serde(default)]
    pub total_nodes: usize,
    #[serde(default)]
    pub total_edges: usize,
}

/* ---------------- 本地关系抽取 ---------------- */

/// 从词条的 `related` 里猜关系类型。
///
/// 各家词典源把同义/反义**混在同一个数组**里，有的还会写成
/// `syn: word` / `antonym: word` 这种带前缀的形式。这里做一次归一：
/// 能认出前缀就用前缀，认不出就归到「相关」—— 不硬猜，避免把反义词
/// 标成同义词（那比不标更误导）。
fn classify_related(raw: &str) -> (&'static str, String) {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    for (pfx, rel) in [
        ("syn:", "synonym"),
        ("synonym:", "synonym"),
        ("同义", "synonym"),
        ("ant:", "antonym"),
        ("antonym:", "antonym"),
        ("opposite:", "antonym"),
        ("反义", "antonym"),
        ("rel:", "related"),
        ("related:", "related"),
        ("see also:", "related"),
    ] {
        if lower.starts_with(pfx) || s.starts_with(pfx) {
            let rest = s[pfx.len()..].trim().trim_start_matches([':', '：', ' ']);
            if !rest.is_empty() {
                return (rel, rest.to_string());
            }
        }
    }
    ("related", s.to_string())
}

/// 词形是否像「一个词」。
///
/// 词典源里 related 偶尔会塞进整句话，那画到图上就是一个巨大的节点。
/// 只保留短词条（≤ 24 字符且不含句末标点）。
fn looks_like_word(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.chars().count() <= 24
        && !t.contains('.')
        && !t.contains('；')
        && !t.contains('。')
        && !t.contains('\n')
}

/// 从一条词条里抽取本地关系边（零算力）。
pub fn extract_local_edges(entry: &WordEntry) -> Vec<GraphEdge> {
    let mut out: Vec<GraphEdge> = Vec::new();
    let me = entry.word.trim();
    if me.is_empty() {
        return out;
    }
    let lang = entry.lang.clone();

    for raw in &entry.related {
        let (rel, other) = classify_related(raw);
        if !looks_like_word(&other) || other.eq_ignore_ascii_case(me) {
            continue;
        }
        out.push(GraphEdge {
            src: me.to_string(),
            dst: other,
            rel: rel.to_string(),
            weight: 1.0,
            source: "local".into(),
        });
    }

    // 词形变化 → 派生关系。方向统一成「原形 → 变形」。
    for inf in &entry.inflections {
        let form = inf.form.trim();
        if !looks_like_word(form) || form.eq_ignore_ascii_case(me) {
            continue;
        }
        out.push(GraphEdge {
            src: me.to_string(),
            dst: form.to_string(),
            rel: "derived".into(),
            weight: 0.6,
            source: "local".into(),
        });
    }

    let _ = lang;
    out
}

/* ---------------- AI 发散 ---------------- */

/// 发散用的 system 提示词。
///
/// 要点：
/// - **必须同语种**。中文用户学英语时，模型很容易把反义词写成中文，
///   那样图上就出现一堆中文节点，与「英语词网络」完全脱节。
/// - 只要 JSON，不要解释 —— 推理模型尤其容易先写一段思考过程。
pub const EXPAND_SYSTEM: &str = r#"你是一个词汇关系抽取引擎。用户给出一个单词，请列出与它有关联的**同一语种**的单词，按关系分类。

只输出一个 JSON 对象，不要输出任何解释、思考过程、Markdown 围栏或多余文字。格式固定为：
{"synonym":[],"antonym":[],"hypernym":[],"hyponym":[],"collocation":[]}

规则：
1. 每个数组最多 5 项，没有就留空数组；
2. 每一项只能是**单个单词或固定短语**，不要带序号、释义、音标或括号注释；
3. 必须是**与输入单词相同语种**的词（输入是英文单词就输出英文单词，绝不翻译成中文）；
4. 不要重复输入单词本身，也不要列它的词形变化（那是另一个字段的事）；
5. 只输出 JSON 对象本身，第一个字符必须是 {，最后一个字符必须是 }。"#;

/// 构造发散用的 user 提示词。
pub fn expand_user_prompt(word: &str, lang_name: &str, existing: &[String]) -> String {
    let mut s = format!("单词：{word}\n语种：{lang_name}\n");
    if !existing.is_empty() {
        s.push_str(&format!(
            "已经记录过的关联词（不要重复输出）：{}\n",
            existing.join("、")
        ));
    }
    s.push_str("\n请输出 JSON。");
    s
}

/// 从模型回复里抠出 JSON 对象。
///
/// 容错三层：剥 ```json 围栏 → 截取第一个 `{` 到最后一个 `}` → 直接解析。
/// 小模型经常在 JSON 前后多写一句话，直接 `serde_json::from_str` 必失败。
pub fn parse_expand_json(raw: &str) -> Result<serde_json::Map<String, serde_json::Value>> {
    let text = crate::llm::strip_outer_fence(raw);
    let body = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => return Err(anyhow!("模型没有输出 JSON 对象")),
    };
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| anyhow!("发散结果不是合法 JSON：{e}"))?;
    v.as_object()
        .cloned()
        .ok_or_else(|| anyhow!("发散结果不是 JSON 对象"))
}

/// 把模型返回的 JSON 转成边。
///
/// 只认识 [`RELS`] 里出现过的键（synonym/antonym/hypernym/hyponym），
/// `collocation` 归到「搭配」。多余的键直接忽略，避免图上出现没有图例的边。
pub fn edges_from_expand(word: &str, map: &serde_json::Map<String, serde_json::Value>) -> Vec<GraphEdge> {
    let mut out = Vec::new();
    for (key, val) in map {
        let rel = match key.as_str() {
            "synonym" | "synonyms" => "synonym",
            "antonym" | "antonyms" => "antonym",
            "hypernym" | "hypernyms" => "hypernym",
            "hyponym" | "hyponyms" => "hyponym",
            _ => continue, // collocation 等暂不画进图，避免图过密
        };
        let Some(arr) = val.as_array() else { continue };
        for it in arr.iter().take(5) {
            let Some(w) = it.as_str() else { continue };
            // 模型偶尔会写成 "happy (高兴)" 这种带注释的形式
            let clean = w
                .split(['(', '（', '/', '|'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !looks_like_word(&clean) || clean.eq_ignore_ascii_case(word) {
                continue;
            }
            out.push(GraphEdge {
                src: word.to_string(),
                dst: clean,
                rel: rel.to_string(),
                // AI 推测的边权重低一档：布局时不会盖过词典里明确写的关系
                weight: 0.8,
                source: "llm".into(),
            });
        }
    }
    // 去重：同一对词可能同时出现在 synonym 和 hypernym 里
    out.sort_by(|a, b| (a.dst.clone(), a.rel.clone()).cmp(&(b.dst.clone(), b.rel.clone())));
    out.dedup_by(|a, b| a.dst == b.dst && a.rel == b.rel);
    out
}

/// 调模型做一次发散。
pub async fn expand_with_llm(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    word: &str,
    lang_code: &str,
    existing: &[String],
) -> Result<Vec<GraphEdge>> {
    let lang_name = crate::translate::lang_name(lang_code);
    let user = expand_user_prompt(word, &lang_name, existing);
    let raw = crate::llm::chat(client, cfg, EXPAND_SYSTEM, &user).await?;
    let map = parse_expand_json(&raw)?;
    let edges = edges_from_expand(word, &map);
    if edges.is_empty() {
        return Err(anyhow!("模型这次没有给出可用的关联词，稍后再试"));
    }
    Ok(edges)
}
