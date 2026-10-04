//! Tauri 命令层：前端可调用的全部后端接口。
//!
//! 命名约定：`cmd_` 前缀便于在主入口统一注册。
//! 所有命令统一返回 `Result<T, String>`，错误转成中文文案直接给用户看。

use crate::db;
use crate::dict;
use crate::llm;
use crate::models::*;
use crate::search;
use crate::srs::{self, build_plan, retention, Grade, PlanDay, ScheduleResult};
use crate::state::AppState;
use crate::timeutil;
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

/// 统一错误转换：把 anyhow 错误变成中文提示。
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ============================================================
// 一、系统与配置
// ============================================================

/// 应用基础信息（前端启动时拉取）。
#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub version: String,
    pub name: String,
    pub now: i64,
    pub now_text: String,
    pub today: String,
    pub greeting: String,
    pub data_dir: String,
    pub db_path: String,
}

#[tauri::command]
pub fn cmd_app_info(state: State<'_, Arc<AppState>>) -> Result<AppInfo, String> {
    let now = timeutil::now_ts();
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        name: "WordWise".to_string(),
        now,
        now_text: timeutil::now_text(),
        today: srs::day_key(now),
        greeting: timeutil::greeting().to_string(),
        data_dir: state.data_dir.display().to_string(),
        db_path: state.db.path().display().to_string(),
    })
}

/// 读取配置。
#[tauri::command]
pub fn cmd_get_config(state: State<'_, Arc<AppState>>) -> AppConfig {
    state.cfg()
}

/// 保存配置。
#[tauri::command]
pub fn cmd_save_config(state: State<'_, Arc<AppState>>, config: AppConfig) -> Result<(), String> {
    state
        .update_config(|c| {
            *c = config.clone();
        })
        .map_err(err)
}

/// 只更新记忆辅助开关（需求 3），前端设置面板高频调用。
#[tauri::command]
pub fn cmd_set_study_options(
    state: State<'_, Arc<AppState>>,
    options: StudyOptions,
) -> Result<AppConfig, String> {
    state
        .update_config(|c| c.study = options.clone())
        .map_err(err)?;
    Ok(state.cfg())
}

/// 更新 LM Studio 配置。
#[tauri::command]
pub fn cmd_set_llm_config(
    state: State<'_, Arc<AppState>>,
    llm: LlmConfig,
) -> Result<AppConfig, String> {
    state.update_config(|c| c.llm = llm.clone()).map_err(err)?;
    Ok(state.cfg())
}

// ============================================================
// 二、LM Studio 连接（需求：本地大模型能力）
// ============================================================

/// 检查本地模型服务状态。
#[tauri::command]
pub async fn cmd_llm_status(state: State<'_, Arc<AppState>>) -> Result<llm::LlmStatus, String> {
    let cfg = state.cfg();
    Ok(llm::status(&state.http, &cfg.llm).await)
}

/// 探测服务并自动选用第一个可用模型，写回配置。
#[tauri::command]
pub async fn cmd_llm_autoconnect(state: State<'_, Arc<AppState>>) -> Result<llm::LlmStatus, String> {
    let cfg = state.cfg();
    let st = llm::status(&state.http, &cfg.llm).await;
    if st.online && !st.active_model.is_empty() {
        let model = st.active_model.clone();
        state
            .update_config(|c| c.llm.model = model.clone())
            .map_err(err)?;
    }
    Ok(st)
}

/// AI 讲解单词（流式推送）。
///
/// 通过 `explain://delta` 事件把增量文本推给前端，实现打字机效果；
/// 命令本身返回完整文本，便于前端做兜底与缓存。
#[tauri::command]
pub async fn cmd_ai_explain(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    question: Option<String>,
) -> Result<String, String> {
    use tauri::Emitter;

    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());

    // 尽量带上词典释义，讲解更准确
    let entry = state
        .db
        .get_word(&word, &lang)
        .ok()
        .flatten()
        .unwrap_or_else(|| WordEntry::new(&word));

    let user_prompt = match &question {
        Some(q) if !q.trim().is_empty() => {
            format!(
                "单词：{}\n用户的追问：{}\n\n请针对该问题作答，若需要可结合该词的其他用法补充说明。",
                word, q
            )
        }
        _ => llm::explain_prompt(&entry, ""),
    };

    let app2 = app.clone();
    let word2 = word.clone();

    let full = llm::chat_stream(
        &state.http,
        &cfg.llm,
        &cfg.llm.system_prompt,
        &user_prompt,
        |d, kind| {
            // 忽略发送失败（前端可能已关闭面板）
            // kind 区分正文与思考过程，前端可分别渲染
            let channel = match kind {
                llm::DeltaKind::Content => "explain://delta",
                llm::DeltaKind::Reasoning => "explain://reasoning",
            };
            let _ = app2.emit(
                channel,
                serde_json::json!({ "word": word2, "delta": d }),
            );
        },
    )
    .await
    .map_err(err)?;

    let _ = app.emit(
        "explain://done",
        serde_json::json!({ "word": word, "text": full }),
    );
    Ok(full)
}

/// 非流式讲解（用于导出、批量生成）。
#[tauri::command]
pub async fn cmd_ai_explain_sync(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<String, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let entry = state
        .db
        .get_word(&word, &lang)
        .ok()
        .flatten()
        .unwrap_or_else(|| WordEntry::new(&word));
    let prompt = llm::explain_prompt(&entry, "");
    llm::chat(&state.http, &cfg.llm, &cfg.llm.system_prompt, &prompt)
        .await
        .map_err(err)
}

/// 用本地大模型生成结构化词条（离线兜底）。
#[tauri::command]
pub async fn cmd_ai_generate_entry(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<WordEntry, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    llm::generate_entry(&state.http, &cfg.llm, &word, &lang)
        .await
        .map_err(err)
}

// ============================================================
// 三、查词与搜索（需求 2、6）
// ============================================================

/// 查词：本地词库 → 缓存 → 多源联网 → 大模型兜底。
#[tauri::command]
pub async fn cmd_lookup(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    force_refresh: Option<bool>,
) -> Result<dict::LookupResult, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let word = word.trim().to_string();

    if word.is_empty() {
        return Err("请输入要查询的单词".into());
    }

    if force_refresh.unwrap_or(false) {
        // 强制刷新时清掉这个词的缓存
        state.db.clear_cache().ok();
    }

    let ttl = 7 * 24 * 3600; // 词义一周内基本不变
    let allow_llm = cfg.study.ai_explain;

    // 大模型兜底闭包：先尝试用 LLM 生成（离线可用）
    // 注意：这里克隆一份 word / lang 交给闭包，避免 move 之后主流程无法再借用。
    let http = state.http.clone();
    let llm_cfg = cfg.llm.clone();
    let llm_word = word.clone();
    let llm_lang = lang.clone();

    let fut = async move {
        if allow_llm {
            llm::generate_entry(&http, &llm_cfg, &llm_word, &llm_lang).await
        } else {
            anyhow::bail!("未启用本地大模型兜底")
        }
    };

    let result = dict::lookup_with_cache(
        &state.db,
        &word,
        &lang,
        &cfg.dict_sources,
        &state.http,
        ttl,
        allow_llm,
        fut,
    )
    .await
    .map_err(err)?;

    state.db.log_search(&result.word, timeutil::now_ts()).ok();
    Ok(result)
}

/// 输入联想。
#[tauri::command]
pub async fn cmd_suggest(
    state: State<'_, Arc<AppState>>,
    query: String,
    lang: Option<String>,
) -> Result<Vec<search::Suggestion>, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    Ok(search::suggest(&state.http, &query, &lang).await)
}

/// 综合搜索（候选 + 维基百科知识）。
#[tauri::command]
pub async fn cmd_search(
    state: State<'_, Arc<AppState>>,
    query: String,
    lang: Option<String>,
    with_wiki: Option<bool>,
) -> Result<search::SearchResult, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    search::search(&state.http, &query, &lang, with_wiki.unwrap_or(true))
        .await
        .map_err(err)
}

/// 维基百科摘要。
#[tauri::command]
pub async fn cmd_wiki(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<Option<search::WikiSummary>, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    Ok(search::wiki_summary(&state.http, &word, &lang).await)
}

/// 读取本地词库中的词。
#[tauri::command]
pub fn cmd_get_word(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<Option<WordEntry>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    state.db.get_word(&word, &lang).map_err(err)
}

/// 检索本地词库。
#[tauri::command]
pub fn cmd_search_words(
    state: State<'_, Arc<AppState>>,
    query: String,
    lang: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<db::WordRow>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    state
        .db
        .search_words(&lang, &query, limit.unwrap_or(50))
        .map_err(err)
}

/// 最近搜索记录。
#[tauri::command]
pub fn cmd_recent_searches(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    state.db.recent_searches(20).map_err(err)
}

// ============================================================
// 四、词库管理
// ============================================================

/// 分页读取词库。
#[tauri::command]
pub fn cmd_list_words(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<db::WordRow>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    state
        .db
        .list_words(&lang, limit.unwrap_or(100), offset.unwrap_or(0))
        .map_err(err)
}

/// 新增/更新一个词条到词库。
#[tauri::command]
pub fn cmd_add_word(state: State<'_, Arc<AppState>>, entry: WordEntry) -> Result<(), String> {
    let now = timeutil::now_ts();
    state.db.upsert_word(&entry, now).map_err(err)?;
    // 同时建立学习状态，让它进入复习队列
    let st = StudyState::new(&entry.word, &entry.lang, now);
    state.db.upsert_state(&st).map_err(err)
}

/// 批量导入单词（只要拼写，释义联网补全）。
#[tauri::command]
pub async fn cmd_import_words(
    state: State<'_, Arc<AppState>>,
    words: Vec<String>,
    lang: Option<String>,
    prefetch: Option<bool>,
) -> Result<ImportReport, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = timeutil::now_ts();

    let mut report = ImportReport::default();

    // 先落库建立学习状态，保证即使联网失败也能开始背
    for w in &words {
        let w = w.trim();
        if w.is_empty() || !search::is_plausible_word(w) {
            report.skipped += 1;
            continue;
        }
        let key = w.to_lowercase();
        if state.db.get_word(&key, &lang).map_err(err)?.is_some() {
            report.skipped += 1;
            continue;
        }
        let entry = WordEntry {
            word: key.clone(),
            lang: lang.clone(),
            ..Default::default()
        };
        state.db.upsert_word(&entry, now).map_err(err)?;
        state
            .db
            .upsert_state(&StudyState::new(&key, &lang, now))
            .map_err(err)?;
        report.added += 1;
    }

    // 可选：立即联网预热释义
    if prefetch.unwrap_or(true) {
        let keys: Vec<String> = words
            .iter()
            .map(|w| w.trim().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        report.prefetched = dict::prefetch(
            &state.db,
            &keys,
            &lang,
            &cfg.dict_sources,
            &state.http,
            4,
        )
        .await
        .unwrap_or(0);
    }

    Ok(report)
}

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub added: i64,
    pub skipped: i64,
    pub prefetched: usize,
}

/// 删除词。
#[tauri::command]
pub fn cmd_delete_word(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<(), String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    state.db.delete_word(&word, &lang).map_err(err)
}

/// 词库总量。
#[tauri::command]
pub fn cmd_word_count(state: State<'_, Arc<AppState>>, lang: Option<String>) -> Result<i64, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    state.db.word_count(&lang).map_err(err)
}

// ============================================================
// 五、背诵与调度（需求 1、4、5）
// ============================================================

/// 开始一轮背诵。
///
/// 出题顺序（需求 4）：
/// 1. 强化记忆词（常错词）优先
/// 2. 精确到期的词
/// 3. 若不够，用「熟练度最低的词」补足
/// 4. 若还不够，用词库中的新词补足（首次学习）
#[tauri::command]
pub fn cmd_start_session(
    state: State<'_, Arc<AppState>>,
    mode: Option<QuizMode>,
    size: Option<i64>,
    leech_only: Option<bool>,
    lang: Option<String>,
) -> Result<SessionInfo, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = timeutil::now_ts();
    let limit = size.unwrap_or(cfg.study.batch_size).clamp(1, 200);
    let mode = mode.unwrap_or_default();
    let leech_only = leech_only.unwrap_or(false);

    let mut picked: Vec<WordEntry> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    // 1) 到期 + 强化记忆词
    let due = if leech_only {
        state.db.leech_states(&lang, limit).map_err(err)?
    } else {
        state.db.due_states(&lang, now, limit).map_err(err)?
    };

    for st in &due {
        if let Some(e) = load_entry(&state, &st.word, &lang)? {
            if seen.insert(st.word.clone()) {
                picked.push(e);
            }
        }
    }

    // 2) 不够则补最弱的词
    if (picked.len() as i64) < limit {
        for st in state.db.weakest_states(&lang, limit * 3).map_err(err)? {
            if (picked.len() as i64) >= limit {
                break;
            }
            if !seen.contains(&st.word) {
                if let Some(e) = load_entry(&state, &st.word, &lang)? {
                    seen.insert(st.word.clone());
                    picked.push(e);
                }
            }
        }
    }

    // 3) 还不够则引入新词
    if (picked.len() as i64) < limit {
        let need = limit - picked.len() as i64;
        let rows = state
            .db
            .list_words(&lang, need * 2, 0)
            .map_err(err)?;
        for r in rows {
            if (picked.len() as i64) >= limit {
                break;
            }
            if !seen.contains(&r.word) {
                seen.insert(r.word.clone());
                picked.push(r.entry);
            }
        }
    }

    if picked.is_empty() {
        return Err("词库为空，请先在「词库」页导入单词".into());
    }

    let total = picked.len();

    // 写入会话
    {
        let mut s = state.session.write();
        s.queue = picked;
        s.index = 0;
        s.mode = Some(mode);
        s.correct = 0;
        s.wrong = 0;
        s.started_at = now;
        s.leech_only = leech_only;
    }

    Ok(SessionInfo {
        mode,
        total,
        index: 0,
        correct: 0,
        wrong: 0,
        leech_only,
    })
}

/// 从内存/缓存/词库加载词条。
fn load_entry(
    state: &AppState,
    word: &str,
    lang: &str,
) -> Result<Option<WordEntry>, String> {
    if let Ok(Some(e)) = state.db.get_word(word, lang) {
        // 词库里只有占位记录（没有释义）时，尝试用缓存补全
        if !e.senses.is_empty() {
            return Ok(Some(e));
        }
    }
    let now = timeutil::now_ts();
    if let Ok(Some(e)) = state.db.get_cached(word, lang, 30 * 24 * 3600, now) {
        return Ok(Some(e));
    }
    // 兜底：返回只有词的占位条目，前端会异步联网补全
    state.db.get_word(word, lang).map_err(err)
}

/// 取当前题目（含干扰项）。
#[tauri::command]
pub fn cmd_current_question(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
) -> Result<Option<QuizCard>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let (entry, mode, index, is_leech, total, correct, wrong) = {
        let s = state.session.read();
        if !s.is_active() {
            return Ok(None);
        }
        let e = match s.current() {
            Some(e) => e.clone(),
            None => return Ok(None),
        };
        (
            e,
            s.mode.unwrap_or_default(),
            s.index,
            s.queue
                .get(s.index)
                .map(|x| {
                    state
                        .db
                        .get_state(&x.word, &lang)
                        .ok()
                        .flatten()
                        .map(|st| st.is_leech)
                        .unwrap_or(false)
                })
                .unwrap_or(false),
            s.total(),
            s.correct,
            s.wrong,
        )
    };
    let card = build_card(
        &state,
        &entry,
        mode,
        &lang,
        is_leech,
        CardProgress {
            index: index + 1,
            total,
            correct,
            wrong,
        },
    )?;
    Ok(Some(card))
}

/// 构造一道题：按模式决定题面与答案，并生成干扰项。
fn build_card(
    state: &AppState,
    entry: &WordEntry,
    mode: QuizMode,
    lang: &str,
    is_leech: bool,
    progress: CardProgress,
) -> Result<QuizCard, String> {
    let (prompt, answer) = match mode {
        QuizMode::EnToZh => {
            let a = entry.primary_definition();
            let a = if a.trim().is_empty() {
                "（暂无释义，点击查询）".to_string()
            } else {
                a
            };
            (entry.word.clone(), a)
        }
        QuizMode::ZhToEn => {
            let q = entry.primary_definition();
            let q = if q.trim().is_empty() {
                entry.word.clone()
            } else {
                q
            };
            (q, entry.word.clone())
        }
    };

    // 干扰项：从同语言词库随机取，保证不与正确答案重复
    let mut options: Vec<String> = Vec::new();
    let pool = state.db.random_words(lang, 30).map_err(err)?;
    for p in pool {
        if options.len() >= 3 {
            break;
        }
        let cand = match mode {
            QuizMode::EnToZh => p.primary_definition(),
            QuizMode::ZhToEn => p.word.clone(),
        };
        if cand.trim().is_empty() || cand == answer || options.contains(&cand) {
            continue;
        }
        options.push(cand);
    }

    Ok(QuizCard {
        prompt,
        answer,
        entry: entry.clone(),
        options,
        mode,
        is_leech,
        index: progress.index,
        total: progress.total,
        correct_count: progress.correct,
        wrong_count: progress.wrong,
    })
}

/// 题目附带的本轮进度信息。
#[derive(Debug, Clone, Copy, Default)]
struct CardProgress {
    index: usize,
    total: usize,
    correct: i32,
    wrong: i32,
}

/// 提交答案 —— 驱动 SRS 调度（需求 4）。
#[tauri::command]
pub fn cmd_submit_answer(
    state: State<'_, Arc<AppState>>,
    word: Option<String>,
    grade: Grade,
    elapsed_ms: Option<i64>,
    lang: Option<String>,
) -> Result<AnswerResult, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = timeutil::now_ts();

    // 确定被作答的词：显式传入优先，否则用会话当前题
    let word = match word {
        Some(w) if !w.trim().is_empty() => w,
        _ => {
            let s = state.session.read();
            match s.current() {
                Some(e) => e.word.clone(),
                None => return Err("当前没有进行中的题目".into()),
            }
        }
    };

    let mode = state.session.read().mode.unwrap_or_default();
    let mode_str = match mode {
        QuizMode::EnToZh => "en_to_zh",
        QuizMode::ZhToEn => "zh_to_en",
    };

    // 取或建学习状态
    let mut st = state
        .db
        .get_state(&word, &lang)
        .map_err(err)?
        .unwrap_or_else(|| StudyState::new(&word, &lang, now));

    // 调度
    let result: ScheduleResult = srs::schedule(&mut st, grade, &cfg.srs, now);
    state.db.upsert_state(&result.state).map_err(err)?;
    state
        .db
        .log_review(
            &word,
            &lang,
            grade_str(grade),
            mode_str,
            now,
            elapsed_ms.unwrap_or(0),
        )
        .map_err(err)?;

    // 更新会话计数并推进
    let (finished, total, correct, wrong) = {
        let mut s = state.session.write();
        if grade.is_correct() {
            s.correct += 1;
        } else {
            s.wrong += 1;
        }
        s.index += 1;
        (s.index >= s.queue.len(), s.total(), s.correct, s.wrong)
    };

    // 取完整词条，用于答错时弹详情卡（需求 5）
    let entry = load_entry(&state, &word, &lang)?
        .unwrap_or_else(|| WordEntry::new(&word));

    // 在 result 被移入返回值之前先算好记忆强度，避免 move 后借用
    let retention_now = retention(&result.state, now);

    Ok(AnswerResult {
        word: word.clone(),
        grade: grade_str(grade).to_string(),
        correct: grade.is_correct(),
        entry,
        schedule: result,
        session_finished: finished,
        total,
        correct_count: correct,
        wrong_count: wrong,
        // 记忆强度，前端画进度环
        retention: retention_now,
    })
}

fn grade_str(g: Grade) -> &'static str {
    match g {
        Grade::Wrong => "wrong",
        Grade::Hard => "hard",
        Grade::Good => "good",
        Grade::Easy => "easy",
    }
}

/// 跳过当前题。
#[tauri::command]
pub fn cmd_skip(state: State<'_, Arc<AppState>>) -> Result<SessionInfo, String> {
    let mut s = state.session.write();
    s.index += 1;
    Ok(SessionInfo {
        mode: s.mode.unwrap_or_default(),
        total: s.total(),
        index: s.index,
        correct: s.correct,
        wrong: s.wrong,
        leech_only: s.leech_only,
    })
}

/// 结束当前会话。
#[tauri::command]
pub fn cmd_end_session(state: State<'_, Arc<AppState>>) -> Result<SessionInfo, String> {
    let s = state.session.read();
    let info = SessionInfo {
        mode: s.mode.unwrap_or_default(),
        total: s.total(),
        index: s.index,
        correct: s.correct,
        wrong: s.wrong,
        leech_only: s.leech_only,
    };
    drop(s);
    *state.session.write() = Default::default();
    Ok(info)
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub mode: QuizMode,
    pub total: usize,
    pub index: usize,
    pub correct: i32,
    pub wrong: i32,
    pub leech_only: bool,
}

#[derive(Debug, Serialize)]
pub struct AnswerResult {
    pub word: String,
    pub grade: String,
    pub correct: bool,
    pub entry: WordEntry,
    pub schedule: ScheduleResult,
    pub session_finished: bool,
    pub total: usize,
    pub correct_count: i32,
    pub wrong_count: i32,
    pub retention: f64,
}

/// 查询某个词的学习状态与记忆强度。
#[tauri::command]
pub fn cmd_word_state(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<Option<WordStateView>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();
    let st = state.db.get_state(&word, &lang).map_err(err)?;
    Ok(st.map(|s| WordStateView {
        retention: retention(&s, now),
        interval_text: humanize_interval(s.interval_days),
        due_text: srs::humanize_due(s.due_at, now),
        state: s,
    }))
}

fn humanize_interval(days: f64) -> String {
    if days < 1.0 {
        format!("{:.0} 小时", (days * 24.0).max(1.0))
    } else {
        format!("{:.0} 天", days)
    }
}

#[derive(Debug, Serialize)]
pub struct WordStateView {
    pub state: StudyState,
    pub retention: f64,
    pub interval_text: String,
    pub due_text: String,
}

// ============================================================
// 六、统计与复习计划（需求 4）
// ============================================================

/// 学习总览。
#[tauri::command]
pub fn cmd_stats(state: State<'_, Arc<AppState>>, lang: Option<String>) -> Result<Stats, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();

    let total_words = state.db.word_count(&lang).map_err(err)?;
    let learned = state.db.learned_count(&lang).map_err(err)?;
    let mastered = state.db.mastered_count(&lang).map_err(err)?;
    let leeches = state.db.leech_count(&lang).map_err(err)?;
    let due_today = state.db.due_states(&lang, now, 9999).map_err(err)?.len() as i64;
    let (reviewed_today, correct_today, wrong_today) =
        state.db.today_counts(now).map_err(err)?;
    let streak_days = state.db.streak(now).map_err(err)?;

    let history = state
        .db
        .daily_history(14, now)
        .map_err(err)?
        .into_iter()
        .map(|(date, count, correct)| DayStat { date, count, correct })
        .collect();

    Ok(Stats {
        total_words,
        learned,
        mastered,
        leeches,
        due_today,
        reviewed_today,
        correct_today,
        wrong_today,
        streak_days,
        history,
    })
}

/// 未来 N 天复习计划（记忆周期表可视化）。
#[tauri::command]
pub fn cmd_review_plan(
    state: State<'_, Arc<AppState>>,
    days: Option<i64>,
    lang: Option<String>,
) -> Result<Vec<PlanDay>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();
    let states = state.db.all_states(&lang).map_err(err)?;
    Ok(build_plan(&states, days.unwrap_or(14).clamp(1, 90), now))
}

/// 错词本（需求 5）。
#[tauri::command]
pub fn cmd_leech_list(
    state: State<'_, Arc<AppState>>,
    limit: Option<i64>,
    lang: Option<String>,
) -> Result<Vec<LeechItem>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let now = timeutil::now_ts();
    let states = state.db.leech_states(&lang, limit.unwrap_or(100)).map_err(err)?;

    let mut out = Vec::new();
    for s in states {
        let entry = load_entry(&state, &s.word, &lang)?.unwrap_or_else(|| WordEntry::new(&s.word));
        out.push(LeechItem {
            retention: retention(&s, now),
            entry,
            state: s,
        });
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct LeechItem {
    pub entry: WordEntry,
    pub state: StudyState,
    pub retention: f64,
}

/// 把词移出强化记忆队列。
#[tauri::command]
pub fn cmd_clear_leech(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<(), String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    if let Some(mut st) = state.db.get_state(&word, &lang).map_err(err)? {
        st.is_leech = false;
        st.correct_count += 1; // 手动确认视为一次正反馈
        st.wrong_count = (st.wrong_count - 1).max(0);
        st.recompute_mastery();
        state.db.upsert_state(&st).map_err(err)?;
    }
    Ok(())
}

// ============================================================
// 七、词典源管理（需求 6）
// ============================================================

#[tauri::command]
pub fn cmd_get_sources(state: State<'_, Arc<AppState>>) -> Vec<DictSourceConfig> {
    state.cfg().dict_sources
}

/// 保存词典源配置（支持自定义小语种 API）。
#[tauri::command]
pub fn cmd_save_sources(
    state: State<'_, Arc<AppState>>,
    sources: Vec<DictSourceConfig>,
) -> Result<(), String> {
    state
        .update_config(|c| c.dict_sources = sources.clone())
        .map_err(err)?;
    state.db.save_sources(&sources).map_err(err)
}

/// 测试单个词典源是否可用（配置面板的「测试连接」按钮）。
#[tauri::command]
pub async fn cmd_test_source(
    state: State<'_, Arc<AppState>>,
    source: DictSourceConfig,
    word: Option<String>,
) -> Result<SourceTestResult, String> {
    let test_word = word.unwrap_or_else(|| "hello".to_string());
    let lang = source
        .langs
        .first()
        .cloned()
        .unwrap_or_else(|| "en".to_string());

    let mut trace = Vec::new();
    let started = std::time::Instant::now();
    let hit = dict::lookup_multi(
        &test_word,
        &lang,
        std::slice::from_ref(&source),
        &state.http,
        &mut trace,
    )
    .await;
    let elapsed = started.elapsed().as_millis() as i64;

    match hit {
        Some((entry, _)) => Ok(SourceTestResult {
            ok: true,
            message: format!(
                "连接成功，解析出 {} 个义项、{} 个变形",
                entry.senses.len(),
                entry.inflections.len()
            ),
            sample: Some(entry),
            elapsed_ms: elapsed,
        }),
        None => {
            let reason = trace
                .first()
                .map(|t| t.error.clone())
                .unwrap_or_else(|| "未返回可解析的内容".to_string());
            Ok(SourceTestResult {
                ok: false,
                message: reason,
                sample: None,
                elapsed_ms: elapsed,
            })
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SourceTestResult {
    pub ok: bool,
    pub message: String,
    pub sample: Option<WordEntry>,
    pub elapsed_ms: i64,
}

/// 重置为内置词典源。
#[tauri::command]
pub fn cmd_reset_sources(state: State<'_, Arc<AppState>>) -> Result<Vec<DictSourceConfig>, String> {
    let defaults = dict::builtin::default_sources();
    state
        .update_config(|c| c.dict_sources = defaults.clone())
        .map_err(err)?;
    state.db.save_sources(&defaults).map_err(err)?;
    Ok(defaults)
}

/// 清空词典缓存。
#[tauri::command]
pub fn cmd_clear_cache(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.db.clear_cache().map_err(err)
}

// ============================================================
// 八、数据导入导出
// ============================================================

/// 导出全部数据。
#[tauri::command]
pub fn cmd_export(state: State<'_, Arc<AppState>>, path: Option<String>) -> Result<String, String> {
    let data = state.db.export_all().map_err(err)?;
    let json = serde_json::to_string_pretty(&data).map_err(err)?;

    let out = match path {
        Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => state
            .data_dir
            .join(format!("wordwise-backup-{}.json", srs::day_key(timeutil::now_ts()))),
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&out, json).map_err(err)?;
    Ok(out.display().to_string())
}

/// 导入备份（词库 + 学习进度）。
#[tauri::command]
pub fn cmd_import(state: State<'_, Arc<AppState>>, path: String) -> Result<ImportReport, String> {
    let text = std::fs::read_to_string(&path).map_err(err)?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(err)?;
    let now = timeutil::now_ts();
    let mut report = ImportReport::default();

    if let Some(words) = v.get("words").and_then(|x| x.as_array()) {
        for w in words {
            if let Ok(e) = serde_json::from_value::<WordEntry>(w.clone()) {
                if state.db.upsert_word(&e, now).is_ok() {
                    report.added += 1;
                } else {
                    report.skipped += 1;
                }
            }
        }
    }
    if let Some(states) = v.get("states").and_then(|x| x.as_array()) {
        for s in states {
            if let Ok(st) = serde_json::from_value::<StudyState>(s.clone()) {
                state.db.upsert_state(&st).ok();
            }
        }
    }
    Ok(report)
}

/// 生成一份内置示例词库，让用户开箱即用。
#[tauri::command]
pub fn cmd_seed_demo(state: State<'_, Arc<AppState>>, lang: Option<String>) -> Result<i64, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang.clone());
    let now = timeutil::now_ts();
    let seed = crate::seed::demo_words(&lang);
    let n = seed.len() as i64;
    state.db.bulk_upsert_words(&seed, now).map_err(err)?;
    // 同时建立学习状态
    for e in &seed {
        if state.db.get_state(&e.word, &lang).map_err(err)?.is_none() {
            state
                .db
                .upsert_state(&StudyState::new(&e.word, &lang, now))
                .map_err(err)?;
        }
    }
    Ok(n)
}
