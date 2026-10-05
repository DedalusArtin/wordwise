//! 知识图谱命令（需求：独立于查词/翻译之外的「知识图谱」栏目）。
//!
//! 三条主线：
//! - `cmd_graph_build`：把词库里**已有**的关联信息抽成边（零算力，可反复跑，幂等）
//! - `cmd_graph_view` ：取一张子图给前端画（以某词为中心 N 跳，或全局热点图）
//! - `cmd_graph_expand`：让本地大模型对一个词做「发散」，把新边落库缓存

use crate::graph::{self, GraphData, GraphEdge, GraphNode};
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 全局视图最多画多少条边。
///
/// 前端是纯 DOM/SVG 力导向布局，没有 WebGL。边数上千时布局会明显掉帧，
/// 所以全局视图只取权重最高的这一批，剩下的靠「点词展开」按需看。
const GLOBAL_EDGE_LIMIT: i64 = 240;

/// 单个词展开时最多返回多少条边（含 1~2 跳）。
const FOCUS_EDGE_LIMIT: usize = 160;

/// 从词库重建指定语言的本地关系边。
///
/// 幂等：`upsert_edges` 用的是 `ON CONFLICT DO NOTHING`，重复跑不会
/// 覆盖 AI 发散出来的边，也不会产生重复行。
#[tauri::command]
pub fn cmd_graph_build(state: State<'_, Arc<AppState>>, lang: Option<String>) -> Result<usize, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = crate::timeutil::now_ts();

    // 一次捞全量词条：抽边要读 related / inflections，逐个查库就是 N 次
    // JSON 反序列化，几千词时会明显卡住。
    let rows = state
        .db
        .list_words(&lang, i64::MAX, 0)
        .map_err(err)?;

    let mut all: Vec<GraphEdge> = Vec::new();
    for r in &rows {
        all.extend(graph::extract_local_edges(&r.entry));
    }

    state.db.upsert_edges(&all, &lang, now).map_err(err)
}

/// 取一张子图。
///
/// - 传 `center`：以该词为中心做 N 跳 BFS（`depth` 默认 1，最大 2）
/// - 不传：全局视图，取连接度最高的一批边
#[tauri::command]
pub fn cmd_graph_view(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    center: Option<String>,
    depth: Option<usize>,
) -> Result<GraphData, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let (total_nodes, total_edges) = state.db.graph_counts(&lang).map_err(err)?;

    let center = center.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    let edges: Vec<GraphEdge> = match &center {
        Some(c) => {
            let depth = depth.unwrap_or(1).clamp(1, 2);
            let mut seen: HashSet<(String, String, String)> = HashSet::new();
            let mut out: Vec<GraphEdge> = Vec::new();
            let mut frontier: Vec<String> = vec![c.clone()];
            let mut visited: HashSet<String> = HashSet::new();
            visited.insert(c.clone());

            for _ in 0..depth {
                let mut next: Vec<String> = Vec::new();
                for w in &frontier {
                    // 入边 + 出边都要：派生关系有方向，只看一边会漏掉
                    // 「变形词 → 原形」这条回程。
                    let mut es = state.db.edges_from(w, &lang).map_err(err)?;
                    es.extend(state.db.edges_into(w, &lang).map_err(err)?);
                    for e in es {
                        let key = (e.src.clone(), e.dst.clone(), e.rel.clone());
                        if seen.insert(key) {
                            let other = if e.src == *w { e.dst.clone() } else { e.src.clone() };
                            if visited.insert(other.clone()) {
                                next.push(other);
                            }
                            out.push(e);
                        }
                        if out.len() >= FOCUS_EDGE_LIMIT {
                            break;
                        }
                    }
                    if out.len() >= FOCUS_EDGE_LIMIT {
                        break;
                    }
                }
                frontier = next;
                if frontier.is_empty() || out.len() >= FOCUS_EDGE_LIMIT {
                    break;
                }
            }
            out
        }
        None => state.db.all_edges(&lang, GLOBAL_EDGE_LIMIT).map_err(err)?,
    };

    // ---- 组装节点 ----
    let words = collect_words(&edges);
    let nodes = build_nodes(&state, &lang, &words, &edges);

    Ok(GraphData {
        nodes,
        edges,
        center,
        total_nodes,
        total_edges,
    })
}

/// 从边集合里收集全部词。
fn collect_words(edges: &[GraphEdge]) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    for e in edges {
        set.insert(e.src.clone());
        set.insert(e.dst.clone());
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

/// 给节点补上度数 / 是否在词库 / 释义 / 掌握度。
fn build_nodes(state: &AppState, lang: &str, words: &[String], edges: &[GraphEdge]) -> Vec<GraphNode> {
    // 度数先用边算，省一次 SQL
    let mut deg: HashMap<&str, i32> = HashMap::new();
    for e in edges {
        *deg.entry(e.src.as_str()).or_insert(0) += 1;
        *deg.entry(e.dst.as_str()).or_insert(0) += 1;
    }

    // 掌握度一次性取回，避免逐词查库
    let mut mastery: HashMap<String, f64> = HashMap::new();
    if let Ok(states) = state.db.all_states(lang) {
        for s in states {
            mastery.insert(s.word, s.mastery as f64 / 100.0);
        }
    }

    let mut out = Vec::with_capacity(words.len());
    for w in words {
        let entry = state.db.get_word(w, lang).ok().flatten();
        let in_dict = entry.is_some();
        let gloss = entry
            .as_ref()
            .and_then(|e| e.senses.first())
            .map(|s| {
                let d = s.definition.trim();
                // 悬停提示放不下长释义，截断即可
                if d.chars().count() > 60 {
                    format!("{}…", d.chars().take(60).collect::<String>())
                } else {
                    d.to_string()
                }
            })
            .unwrap_or_default();

        out.push(GraphNode {
            word: w.clone(),
            lang: lang.to_string(),
            degree: *deg.get(w.as_str()).unwrap_or(&0),
            in_dict,
            gloss,
            mastery: mastery.get(w).copied(),
        });
    }
    out
}

/// AI 发散：让本地大模型给一个词补关联词，落库后返回新增边数。
///
/// 失败时给出**能看懂的原因**（模型没启动 / 没返回 JSON），而不是一句
/// 「操作失败」——用户在设置页才配的模型，这里正是最需要提示的地方。
#[tauri::command]
pub async fn cmd_graph_expand(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<usize, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let word = word.trim().to_string();
    if word.is_empty() {
        return Err("请先指定要发散的词".into());
    }

    // 已经记录过的邻居一并告诉模型，避免每次发散都返回同一批词
    let existing = state.db.edge_neighbors(&lang, &word).unwrap_or_default();

    let edges = graph::expand_with_llm(&state.http(), &cfg.llm, &word, &lang, &existing)
        .await
        .map_err(err)?;

    let now = crate::timeutil::now_ts();
    let n = state.db.upsert_edges(&edges, &lang, now).map_err(err)?;
    Ok(n)
}

/// 搜索图谱里的节点（图谱页搜索框）。
#[tauri::command]
pub fn cmd_graph_search(
    state: State<'_, Arc<AppState>>,
    q: String,
    lang: Option<String>,
) -> Result<Vec<String>, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let q = q.trim().to_string();
    if q.is_empty() {
        // 空查询给「度数最高的词」，等于一个「从这里开始」的推荐列表
        return Ok(state
            .db
            .top_degree_words(&lang, 20)
            .map_err(err)?
            .into_iter()
            .map(|(w, _)| w)
            .collect());
    }
    state.db.graph_search(&lang, &q, 30).map_err(err)
}

/// 关系类型清单（前端图例与筛选器直接用，保证与后端同一份）。
#[tauri::command]
pub fn cmd_graph_rels() -> Vec<serde_json::Value> {
    graph::all_rels()
}

/// 清空某语言的图谱（重新构建前的「重置」）。
#[tauri::command]
pub fn cmd_graph_clear(state: State<'_, Arc<AppState>>, lang: Option<String>) -> Result<usize, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    state.db.clear_edges(&lang).map_err(err)
}

/// 图谱概览统计（导航角标 / 空状态提示用）。
#[tauri::command]
pub fn cmd_graph_stats(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
) -> Result<serde_json::Value, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let (nodes, edges) = state.db.graph_counts(&lang).map_err(err)?;
    let top = state.db.top_degree_words(&lang, 12).map_err(err)?;
    Ok(serde_json::json!({
        "nodes": nodes,
        "edges": edges,
        "top": top.into_iter().map(|(w, d)| serde_json::json!({"word": w, "degree": d})).collect::<Vec<_>>(),
        "rel_count": graph::RELS.len(),
        // 词库为空时前端要提示「先导入词库」，而不是画一张空图
        "dict_size": state.db.word_count(&lang).unwrap_or(0),
    }))
}
