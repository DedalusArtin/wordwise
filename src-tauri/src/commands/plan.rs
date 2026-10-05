//! 复习计划页增强：到期复习清单（需求 2）。
//!
//! 「未来 14 天安排」回答的是「哪天有多少」，但用户真正要动手时问的是
//! 「**现在该背哪几个**」。所以这里补一个到期清单：把逾期和今天到期的词
//! 逐条列出来，点一下就能开背。

use crate::commands::leech::load_entry;
use crate::models::WordEntry;
use crate::state::AppState;
use crate::timeutil;
use std::sync::Arc;
use tauri::State;

/// 一条到期/逾期的词。
#[derive(Debug, serde::Serialize)]
pub struct DueWord {
    pub word: String,
    pub gloss: String,
    /// 到期时间戳
    pub due_at: i64,
    /// 给人看的时间，例如「今天 14:00」「逾期 3 天」
    pub due_label: String,
    pub overdue: bool,
    pub overdue_days: i64,
    pub mastery: i64,
    pub wrong_count: i64,
    pub is_leech: bool,
}

fn first_gloss(e: &WordEntry) -> String {
    match e.senses.first() {
        Some(s) => {
            // pos / definition 在 WordSense 里是 String（不是 Option），
            // 空值就是空串，直接拼再 trim 即可
            let t = format!("{} {}", s.pos, s.definition).trim().to_string();
            if t.is_empty() { "暂无释义".to_string() } else { t }
        }
        None => "暂无释义".to_string(),
    }
}

/// 到期复习清单：逾期最久的排最前，然后才是今天到期、明天到期。
#[tauri::command]
pub fn cmd_due_words(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<DueWord>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();
    let limit = limit.unwrap_or(60).clamp(1, 300);

    let states = state
        .db
        .due_states(&lang, now, limit)
        .map_err(|e| format!("读取到期词失败：{e}"))?;

    let mut out: Vec<DueWord> = states
        .into_iter()
        .map(|s| {
            let entry = load_entry(&state, &s.word, &lang);
            let gloss = entry.as_ref().map(first_gloss).unwrap_or_else(|| "暂无释义".to_string());
            // due_at 早于今天零点就算逾期，按整天数算，避免「昨晚 23:59 到期」被算成逾期 1 天
            let overdue = s.due_at < timeutil::today_start();
            let overdue_days = if overdue {
                timeutil::days_between(s.due_at, now).max(1)
            } else {
                0
            };
            let hhmm = timeutil::full_ts(s.due_at);
            let hhmm = hhmm.split(' ').nth(1).unwrap_or("").to_string();
            let due_label = if overdue {
                format!("逾期 {} 天", overdue_days)
            } else {
                format!("今天 {hhmm}")
            };
            DueWord {
                word: s.word,
                gloss,
                due_at: s.due_at,
                due_label,
                overdue,
                overdue_days,
                mastery: s.mastery,
                wrong_count: s.wrong_count,
                is_leech: s.is_leech,
            }
        })
        .collect();

    // 后端 due_states 是「先强化词、再按到期时间」，这对出题是对的，
    // 但列表给人看时「逾期 5 天」排在「今天到期」后面会很怪。
    // 这里改成：逾期越久越靠前 → 其次按到期时间 → 最后按强化词优先。
    out.sort_by(|a, b| {
        b.overdue_days
            .cmp(&a.overdue_days)
            .then(a.due_at.cmp(&b.due_at))
            .then(b.is_leech.cmp(&a.is_leech))
    });

    Ok(out)
}
