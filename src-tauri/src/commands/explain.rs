/* ============================================================
   explain.rs —— AI 讲解存档（用户需求 C：「单词讲解结合词库，也算是存储，
   在词库中搜索」）

   为什么要有这一层：
   1. **讲解很贵**。一次讲解是一次完整的模型调用，用户关掉面板就没了，
      再点开同一个词又要重跑一遍。存下来，第二次就是本地读取、秒回。
   2. **讲解本身就是一份词条资料**。它往往比在线词典更贴合学习者
      （有词根词缀、记忆法、易混辨析），但它躺在聊天记录式的一段文字里，
      搜不到、点不开。落到 `explain_store` 后就能像词条一样被搜索、被管理。
   3. **可以一键并进词库**。把讲解正文交给模型整理成结构化 `WordEntry`
      （`llm::entry_from_explain`），写进 `words` 表，于是它就成了真正的词条，
      能进复习队列、能被背诵。

   表结构见 `db::ExplainRow`；主键是 (word, lang, explain_lang)——
   同一个词可以同时留中文讲解和英文讲解，互不覆盖。
   ============================================================ */

use crate::db::ExplainRow;
use crate::llm;
use crate::models::{StudyState, WordEntry};
use crate::state::AppState;
use crate::timeutil;
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    format!("{}", e)
}

/// 归一化查询键：与 `words` 表保持一致（小写 + 去首尾空白）。
///
/// 不归一化的话，「Apple」「apple」「APPLE」会各存一条存档，
/// 「在词库中搜索」就会冒出三条看起来一模一样的记录。
fn kw(w: &str) -> String {
    w.trim().to_lowercase()
}

fn lang_of(state: &AppState, lang: Option<String>) -> String {
    lang.filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| state.cfg().target_lang.clone())
}

fn explain_lang_of(state: &AppState, lang: Option<String>) -> String {
    lang.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        let l = state.cfg().explain_lang.trim().to_string();
        if l.is_empty() {
            "zh".to_string()
        } else {
            l
        }
    })
}

/// 保存（或更新）一条讲解存档。
///
/// 前端在流式讲解**正常结束**后调用它。之所以不让后端在流里直接落库：
/// 流式过程中用户可能中途关掉面板或换词，只有前端知道「这次讲解到底
/// 有没有正常结束」，落一条半截的讲解比不落更糟。
#[tauri::command]
pub fn cmd_save_explain(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    explain_lang: Option<String>,
    text: String,
    original: Option<String>,
    translated: Option<bool>,
) -> Result<ExplainRow, String> {
    let w = kw(&word);
    if w.is_empty() {
        return Err("单词不能为空".into());
    }
    let lang = lang_of(&state, lang);
    let explain_lang = explain_lang_of(&state, explain_lang);
    let now = timeutil::now_ts();

    let row = ExplainRow {
        word: w.clone(),
        lang: lang.clone(),
        explain_lang: explain_lang.clone(),
        text,
        original: original.unwrap_or_default(),
        translated: translated.unwrap_or(false),
        entry_json: String::new(),
        saved: false,
        updated_at: now,
    };
    state.db.save_explain(&row, now).map_err(err)?;

    // 读回：带上库里已有的 entry_json / saved，前端才能立刻显示「已并入词库」
    state
        .db
        .get_explain(&w, &lang, &explain_lang)
        .map_err(err)?
        .ok_or_else(|| "保存后读回失败".to_string())
}

/// 读取某词在指定讲解语言下的存档。
#[tauri::command]
pub fn cmd_get_explain(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    explain_lang: Option<String>,
) -> Result<Option<ExplainRow>, String> {
    state
        .db
        .get_explain(
            &kw(&word),
            &lang_of(&state, lang),
            &explain_lang_of(&state, explain_lang),
        )
        .map_err(err)
}

/// 列出某个词的全部讲解存档（可能同时有中文版和英文版）。
#[tauri::command]
pub fn cmd_list_explains(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<Vec<ExplainRow>, String> {
    state
        .db
        .explains_of(&kw(&word), &lang_of(&state, lang))
        .map_err(err)
}

/// 搜索讲解存档：**同时匹配词与讲解正文**。
///
/// 词库页顶部的搜索框会把它并进来，这样「讲解过但还没导入成词条」的内容
/// 也能被搜出来 —— 否则用户会以为讲解白讲了。
#[tauri::command]
pub fn cmd_search_explains(
    state: State<'_, Arc<AppState>>,
    query: String,
    lang: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<ExplainRow>, String> {
    // 与 `cmd_search_words` 同语义：不传 lang 就是「跨语言搜」
    let lang = lang.filter(|l| !l.trim().is_empty() && l != "all");
    state
        .db
        .search_explains(lang.as_deref(), &query, limit.unwrap_or(50))
        .map_err(err)
}

/// 删除一条讲解存档。
#[tauri::command]
pub fn cmd_delete_explain(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    explain_lang: Option<String>,
) -> Result<usize, String> {
    state
        .db
        .delete_explain(
            &kw(&word),
            &lang_of(&state, lang),
            &explain_lang_of(&state, explain_lang),
        )
        .map_err(err)
}

/// 清空讲解存档。`keep_saved = true` 时保留已并入词库的那些。
#[tauri::command]
pub fn cmd_clear_explains(
    state: State<'_, Arc<AppState>>,
    keep_saved: Option<bool>,
) -> Result<usize, String> {
    state
        .db
        .clear_explains(keep_saved.unwrap_or(true))
        .map_err(err)
}

/// 讲解存档数量（数据库面板展示用）。
#[tauri::command]
pub fn cmd_explain_count(state: State<'_, Arc<AppState>>) -> Result<i64, String> {
    state.db.explain_count().map_err(err)
}

/// 把一条 AI 讲解**并入词库**：整理成结构化词条 → 写进 `words` 表 → 进复习队列。
///
/// 这是「讲解也是一种存储」的落点：讲解从"一段聊天文字"变成"一个可背的词条"。
///
/// `text` 可选：传了就用手上的这段（用户还没保存也能直接并入），
/// 不传就读存档。整理用的模型输出会回写进存档的 `entry_json`，
/// 下次再点「并入词库」就能直接读，不必再问一次模型。
#[tauri::command]
pub async fn cmd_explain_to_entry(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    explain_lang: Option<String>,
    text: Option<String>,
) -> Result<WordEntry, String> {
    let cfg = state.cfg();
    let w = kw(&word);
    if w.is_empty() {
        return Err("单词不能为空".into());
    }
    let lang = lang_of(&state, lang);
    let explain_lang = explain_lang_of(&state, explain_lang);

    let markdown = match text.filter(|t| !t.trim().is_empty()) {
        Some(t) => t,
        None => state
            .db
            .get_explain(&w, &lang, &explain_lang)
            .map_err(err)?
            .map(|r| {
                if r.text.trim().is_empty() {
                    r.original
                } else {
                    r.text
                }
            })
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| "还没有这个词的讲解存档，请先生成 AI 讲解".to_string())?,
    };

    let entry = llm::entry_from_explain(&state.http(), &cfg.llm, &w, &lang, &markdown)
        .await
        .map_err(err)?;

    // 一条像样的释义都没有 → 宁可报错也不污染词库
    if entry.senses.iter().all(|s| s.definition.trim().is_empty()) {
        return Err("模型没能从讲解里整理出可用释义，已放弃写入词库".into());
    }

    let now = timeutil::now_ts();
    state.db.upsert_word(&entry, now).map_err(err)?;
    // 同步建立学习状态，让新词条立刻进入复习队列（与 cmd_add_word 一致）
    let st = StudyState::new(&entry.word, &entry.lang, now);
    state.db.upsert_state(&st).map_err(err)?;

    // 回写：标记「已并入词库」并记下生成的词条
    let entry_json = serde_json::to_string(&entry).unwrap_or_default();
    state
        .db
        .set_explain_saved(&w, &lang, &explain_lang, &entry_json, now)
        .map_err(err)?;

    Ok(entry)
}
