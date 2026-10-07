//! 错题本增强：多维筛选 + 导出（需求 4）。
//!
//! 导出两种格式，各有各的用处：
//! - **Markdown**：给人看的。可以直接打印、贴进笔记，带释义/例句/错因统计。
//! - **CSV**：给机器看的。丢进 Excel 做统计，或导入 Anki 之类做后续复习。
//!   CSV 必须带 UTF-8 BOM，否则 Excel 打开是乱码（国内用户几乎都用 Excel）。

use crate::models::{StudyState, WordEntry};
use crate::srs::retention;
use crate::state::AppState;
use crate::timeutil;
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 一条错题（给前端列表用）。
#[derive(Debug, serde::Serialize)]
pub struct LeechRow {
    pub entry: WordEntry,
    pub state: StudyState,
    /// 记忆保持率 0~1
    pub retention: f64,
    /// 错误率 0~1
    pub error_rate: f64,
}

/// 一次导出的结果。
#[derive(Debug, serde::Serialize)]
pub struct ExportResult {
    pub path: String,
    pub count: usize,
    pub format: String,
}

/// 取词条：本地词库 → 词典缓存 → 占位空词条。
///
/// 错题本里可能有「只做过题、词条没落库」的词（例如在线词库临时导入的），
/// 这时不能整条丢掉，用只有单词名的空词条顶上，用户至少知道是哪个词。
pub(crate) fn load_entry(state: &AppState, word: &str, lang: &str) -> Option<WordEntry> {
    if let Ok(Some(e)) = state.db.get_word(word, lang) {
        return Some(e);
    }
    let now = timeutil::now_ts();
    if let Ok(Some(e)) = state.db.get_cached(word, lang, 7 * 24 * 3600, now) {
        return Some(e);
    }
    None
}

/// 带筛选的错题列表。
#[tauri::command(async)]
pub fn cmd_leech_query(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    min_wrong: Option<i64>,
    max_mastery: Option<i64>,
    min_error_rate: Option<f64>,
    order: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<LeechRow>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();

    let states = state
        .db
        .leech_states_filtered(
            &lang,
            min_wrong.unwrap_or(0).max(0),
            max_mastery.unwrap_or(100).clamp(0, 100),
            min_error_rate.unwrap_or(0.0).clamp(0.0, 1.0),
            order.as_deref().unwrap_or("wrong"),
            limit.unwrap_or(300).clamp(1, 2000),
        )
        .map_err(err)?;

    let mut out = Vec::with_capacity(states.len());
    for s in states {
        let entry = load_entry(&state, &s.word, &lang).unwrap_or_else(|| WordEntry::new(&s.word));
        let error_rate = s.error_rate();
        out.push(LeechRow {
            retention: retention(&s, now),
            error_rate,
            entry,
            state: s,
        });
    }
    Ok(out)
}

/// 错题本概览（顶部统计条）。
#[tauri::command(async)]
pub fn cmd_leech_summary(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
) -> Result<serde_json::Value, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let all = state.db.leech_states(&lang, 2000).map_err(err)?;
    let total = all.len();
    let total_wrong: i64 = all.iter().map(|s| s.wrong_count).sum();
    let avg_mastery = if total == 0 {
        0.0
    } else {
        all.iter().map(|s| s.mastery as f64).sum::<f64>() / total as f64
    };
    // 错 5 次以上的算「顽固词」——用户最该先啃的就是这几个
    let stubborn = all.iter().filter(|s| s.wrong_count >= 5).count();
    Ok(serde_json::json!({
        "total": total,
        "total_wrong": total_wrong,
        "stubborn": stubborn,
        "avg_mastery": (avg_mastery * 10.0).round() / 10.0,
        "dict_size": state.db.word_count(&lang).unwrap_or(0),
    }))
}

/// 批量移出强化队列。
#[tauri::command(async)]
pub fn cmd_leech_remove_many(
    state: State<'_, Arc<AppState>>,
    words: Vec<String>,
    lang: Option<String>,
) -> Result<usize, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let clean: Vec<String> = words.into_iter().filter(|w| !w.trim().is_empty()).collect();
    if clean.is_empty() {
        return Ok(0);
    }
    state.db.clear_leech_many(&clean, &lang).map_err(err)
}

/* ---------------- 导出 ---------------- */

/// CSV 字段转义。
///
/// 规则（RFC 4180）：含逗号/引号/换行时整体加引号，内部引号翻倍。
/// 释义里出现逗号是家常便饭（"v. 放弃，抛弃"），不转义整张表就错位。
fn csv_cell(s: &str) -> String {
    let t = s.replace('\r', " ").replace('\n', " ");
    if t.contains(',') || t.contains('"') || t.contains('\t') {
        format!("\"{}\"", t.replace('"', "\"\""))
    } else {
        t
    }
}

/// 把秒数转成「11 小时后」这种给人看的说法。
fn humanize_due(due_at: i64, now: i64) -> String {
    let d = due_at - now;
    if d <= 0 {
        return "已到期".into();
    }
    let h = d / 3600;
    if h < 1 {
        format!("{} 分钟后", (d / 60).max(1))
    } else if h < 24 {
        format!("{h} 小时后")
    } else {
        format!("{} 天后", h / 24)
    }
}

/// 生成 Markdown 文本。
fn to_markdown(rows: &[LeechRow], lang_name: &str, now: i64) -> String {
    let mut s = String::new();
    let stamp = timeutil::now_text();
    s.push_str("# 错词本导出\n\n");
    s.push_str(&format!(
        "> 导出时间：{stamp} · 共 **{}** 个词 · 语言：{lang_name}\n\n",
        rows.len()
    ));
    s.push_str("按错误次数从多到少排列。建议先啃上面的顽固词。\n\n---\n\n");

    for (i, r) in rows.iter().enumerate() {
        let e = &r.entry;
        s.push_str(&format!("## {}. {}\n\n", i + 1, e.word));

        // 音标：中英日用不同的标签
        let ph = if !e.phonetic.us.trim().is_empty() {
            e.phonetic.us.clone()
        } else {
            e.phonetic.uk.clone()
        };
        if !ph.trim().is_empty() {
            s.push_str(&format!("- 音标：`{}`\n", ph.trim()));
        }

        if let Some(first) = e.senses.first() {
            let pos = if first.pos.trim().is_empty() {
                String::new()
            } else {
                format!("{} ", first.pos.trim())
            };
            s.push_str(&format!("- 释义：{}{}\n", pos, first.definition.trim()));
        }
        // 最多再列 2 条义项，避免导出文件过长
        for extra in e.senses.iter().skip(1).take(2) {
            s.push_str(&format!("- 又：{}\n", extra.definition.trim()));
        }
        if let Some(ex) = e.senses.first().and_then(|x| x.examples.first()) {
            if !ex.text.trim().is_empty() {
                let tr = if ex.translation.trim().is_empty() {
                    String::new()
                } else {
                    format!(" —— {}", ex.translation.trim())
                };
                s.push_str(&format!("- 例句：{}{}\n", ex.text.trim(), tr));
            }
        }
        if !e.mnemonic.trim().is_empty() {
            s.push_str(&format!("- 记忆法：{}\n", e.mnemonic.trim()));
        }

        s.push_str(&format!(
            "- 练习：错 **{}** 次 / 对 {} 次（错误率 {:.0}%）\n",
            r.state.wrong_count,
            r.state.correct_count,
            r.error_rate * 100.0
        ));
        s.push_str(&format!(
            "- 掌握度：{}% · 记忆保持率：{:.0}% · 下次复习：{}\n\n",
            r.state.mastery,
            r.retention * 100.0,
            humanize_due(r.state.due_at, now)
        ));
    }
    s
}

/// 生成 CSV 文本（带 BOM，保证 Excel 不乱码）。
fn to_csv(rows: &[LeechRow], now: i64) -> String {
    let mut s = String::from("\u{feff}");
    s.push_str("单词,语言,音标,词性,释义,例句,例句翻译,错误次数,正确次数,错误率,掌握度,记忆保持率,下次复习,下次复习时间\n");
    for r in rows {
        let e = &r.entry;
        let ph = if !e.phonetic.us.trim().is_empty() {
            &e.phonetic.us
        } else {
            &e.phonetic.uk
        };
        let (pos, def, ex, tr) = match e.senses.first() {
            Some(x) => (
                x.pos.clone(),
                x.definition.clone(),
                x.examples
                    .first()
                    .map(|y| y.text.clone())
                    .unwrap_or_default(),
                x.examples
                    .first()
                    .map(|y| y.translation.clone())
                    .unwrap_or_default(),
            ),
            None => Default::default(),
        };
        let due_text = timeutil::full_ts(r.state.due_at);
        let cells = [
            e.word.clone(),
            e.lang.clone(),
            ph.trim().to_string(),
            pos.trim().to_string(),
            def.trim().to_string(),
            ex.trim().to_string(),
            tr.trim().to_string(),
            r.state.wrong_count.to_string(),
            r.state.correct_count.to_string(),
            format!("{:.0}%", r.error_rate * 100.0),
            format!("{}%", r.state.mastery),
            format!("{:.0}%", r.retention * 100.0),
            humanize_due(r.state.due_at, now),
            due_text,
        ];
        s.push_str(
            &cells
                .iter()
                .map(|c| csv_cell(c))
                .collect::<Vec<_>>()
                .join(","),
        );
        s.push('\n');
    }
    s
}

/// 导出错题集。
///
/// `format` 取 `md`（默认）或 `csv`。不传 `path` 时写到数据目录下的
/// `exports/`，导完把完整路径回给前端，由前端提示并可一键打开所在文件夹。
#[tauri::command(async)]
pub fn cmd_leech_export(
    state: State<'_, Arc<AppState>>,
    format: Option<String>,
    path: Option<String>,
    lang: Option<String>,
    min_wrong: Option<i64>,
    max_mastery: Option<i64>,
    min_error_rate: Option<f64>,
    order: Option<String>,
) -> Result<ExportResult, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();
    let fmt = format.unwrap_or_else(|| "md".into()).to_lowercase();
    let fmt = if fmt == "csv" { "csv" } else { "md" };

    let rows = cmd_leech_query(
        state.clone(),
        Some(lang.clone()),
        min_wrong,
        max_mastery,
        min_error_rate,
        order,
        Some(2000),
    )?;
    if rows.is_empty() {
        return Err("当前筛选条件下没有错词可导出".into());
    }

    let lang_name = crate::translate::lang_name(&lang);
    let body = if fmt == "csv" {
        to_csv(&rows, now)
    } else {
        to_markdown(&rows, &lang_name, now)
    };

    let out = match path {
        Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => {
            let dir = state.data_dir.join("exports");
            std::fs::create_dir_all(&dir).ok();
            dir.join(format!(
                "错词本-{}-{}.{}",
                lang,
                crate::srs::day_key(now).replace('-', ""),
                fmt
            ))
        }
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&out, body).map_err(err)?;

    Ok(ExportResult {
        path: out.display().to_string(),
        count: rows.len(),
        format: fmt.to_string(),
    })
}
