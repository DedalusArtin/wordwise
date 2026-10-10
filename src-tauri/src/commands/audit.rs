//! AI 词条自检（需求：每个词条纳入背词表前先自校一遍）。
//!
//! # 要解决什么
//!
//! 词库里的释义来自三处：第三方词表（mahavivo / KyleBing 等）、在线词典、
//! 本地大模型。它们各有各的错法：
//!   - 第三方词表用 `<` 当义项分隔符，整段 gloss 被当成一个义项塞进来，
//!     于是选项尾部拖一个杂散 `<`；
//!   - 释义与词性对不上（`n.` 后面挂动词释义）；
//!   - 例句不是例句（词典说明文字、或者干脆是别的词的句子）。
//! 这些错误在背诵时才会暴露，而用户背到时才发现问题，等于白背一轮。
//!
//! # 设计取舍
//!
//! - **只判不改、改完复检**：自检先给结论；能修就修，修完**再跑一轮**
//!   （`round + 1`），仍然不合格才标记 `rejected`。一次模型的意见不足以
//!   直接改掉用户的数据。
//! - **剔除 = 标记，不真删**：`rejected` 只写进 `words.audit_state`，
//!   出题队列据此跳过。真删会让用户「我几千个词怎么少了一批」无从查证。
//! - **全程留痕**：每一次自检都写 `ai_audit_log`（含改前改后 JSON、
//!   用的模型、耗时），同时打一行运行日志 —— 用户能回答
//!   「这个词为什么被剔了 / 被改成什么样了」。
//! - **模型不可用不算失败**：自检依赖本地大模型，没部署时命令返回明确
//!   错误让界面提示指路，绝不因为自检失败阻塞导入。

use crate::models::WordEntry;
use crate::state::AppState;
use crate::timeutil;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 自检结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditVerdict {
    /// 没问题
    Ok,
    /// 有问题但已修正（并复检通过）
    Fixed,
    /// 修不了：不是这个语言的词、或释义与词完全无关 → 标记剔除
    Rejected,
}

impl AuditVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditVerdict::Ok => "ok",
            AuditVerdict::Fixed => "fixed",
            AuditVerdict::Rejected => "rejected",
        }
    }
}

/// 一次自检的结果。
#[derive(Debug, Clone, Serialize)]
pub struct EntryAudit {
    pub word: String,
    pub lang: String,
    pub round: i32,
    pub verdict: AuditVerdict,
    /// 命中的问题项：spelling / pos / definition / example
    pub issues: Vec<String>,
    /// 人话说明，界面直接显示
    pub detail: String,
    /// 修正后的词条（verdict=fixed 时有值）
    pub corrected: Option<WordEntry>,
    pub model: String,
    pub elapsed_ms: i64,
}

/// 模型返回的原始结构（`corrected` 是完整词条）。
#[derive(Debug, Clone, Deserialize)]
struct RawAudit {
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    issues: Vec<String>,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    corrected: Option<WordEntry>,
}

/// 自检用的提示词。
///
/// 要求模型**只输出一个 JSON**：讲解可以任它发挥，自检必须能解析，
/// 解析不出来就只能整条判 rejected（宁可让用户手动处理，也不能把模型
/// 的闲聊当成修正写进词库）。
pub fn audit_prompt(entry: &WordEntry) -> String {
    let json = serde_json::to_string_pretty(entry).unwrap_or_else(|_| "{}".to_string());
    let senses: Vec<String> = entry
        .senses
        .iter()
        .take(6)
        .map(|s| format!("[{}] {}", s.pos, s.definition))
        .collect();
    format!(
        "你是英语词典的校对编辑。下面是一条词条数据（JSON）：\n\
         ```json\n{json}\n```\n\
         该词的义项如下：\n{senses}\n\n\
         请逐项检查：\n\
         1. spelling：词目拼写是否正确，是否与其释义、变形自洽；\n\
         2. pos：senses[].pos 的词性标注是否合法（n./v./adj./adv./prep./conj./pron./int. 等），\
         且与同一义项的释义是否匹配；\n\
         3. definition：中文释义是否准确、是否为简体中文、与该词性及该词是否相符，\
         是否残留 `<`、`/`、词性前缀之外的噪声；\n\
         4. example：examples[].text 是否为真实英文例句、是否包含该词；\
         examples[].translation 与英文原文是否对得上。\n\n\
         只输出一个 JSON 对象，不要任何解释文字，格式：\n\
         {{\"verdict\":\"ok|fixed|rejected\",\"issues\":[\"definition\"],\
         \"detail\":\"一句话说明问题\",\"corrected\":{{完整词条 JSON}}}}\n\
         规则：\n\
         - 全部没问题 → verdict=\"ok\"，不要给 corrected；\n\
         - 有问题但能修 → verdict=\"fixed\"，并在 corrected 里给出**完整修正后的词条**\
         （字段与输入一致，不要省略字段）；\n\
         - 修不了（词目根本不是该语言的词、释义与词完全无关）→ verdict=\"rejected\"。",
        json = json,
        senses = if senses.is_empty() {
            "（该词条没有义项）".to_string()
        } else {
            senses.join("\n")
        }
    )
}

fn parse_audit(raw: &str) -> Option<RawAudit> {
    let json = crate::llm::extract_json(raw);
    serde_json::from_str::<RawAudit>(&json)
        .or_else(|_| serde_json::from_str::<RawAudit>(raw))
        .ok()
}

/// 自检一个词条：判定 → 能修就修 → **修完再检一轮**。
///
/// 返回的是最终结论（`round` 标明实际跑了几轮）。
#[tauri::command]
pub async fn cmd_ai_audit_entry(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<EntryAudit, String> {
    let cfg = state.cfg();
    let lang = lang
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cfg.target_lang.clone());
    if !cfg.study.ai_explain || cfg.llm.base_url.trim().is_empty() {
        return Err("还没配置本地大模型，无法做词条自检（可在设置页「本地大模型」一键部署）".into());
    }

    let entry = state
        .db
        .get_word(&word, &lang)
        .map_err(err)?
        .ok_or_else(|| format!("词库里没有「{word}」这个词条"))?;

    audit_one(&state, &entry, 1).await
}

/// 跑一轮自检（不写库、不修正），供批量与复检复用。
async fn run_round(state: &Arc<AppState>, entry: &WordEntry, round: i32) -> Result<RawAudit, String> {
    let cfg = state.cfg();
    let started = std::time::Instant::now();
    let system = "你是严格的词典校对编辑，只输出 JSON，不要输出任何解释。".to_string();
    let raw = crate::llm::chat(
        &state.http(),
        &cfg.llm,
        &system,
        &audit_prompt(entry),
    )
    .await
    .map_err(err)?;
    let elapsed = started.elapsed().as_millis() as i64;
    let parsed = parse_audit(&raw).ok_or_else(|| {
        format!(
            "模型返回的内容不是一个可解析的校验结果（第 {round} 轮），已按不合格处理"
        )
    })?;
    log::debug!("AI 自检 {}/{} 第 {} 轮用时 {}ms", entry.word, entry.lang, round, elapsed);
    Ok(parsed)
}

/// 完整的一次自检流程：判定 → 修正 → 复检 → 落日志与标记。
async fn audit_one(
    state: &Arc<AppState>,
    entry: &WordEntry,
    round: i32,
) -> Result<EntryAudit, String> {
    let now = timeutil::now_ts();
    let before_json = serde_json::to_string(entry).unwrap_or_default();
    let started = std::time::Instant::now();

    let first = match run_round(state, entry, round).await {
        Ok(v) => v,
        Err(e) => {
            // 模型不可用 / 返回不可解析：**标记未决而不是判死**，
            // 并把原因写进日志，下一次批量自检还会捞到它。
            let row = crate::db::AuditRow {
                batch_id: String::new(),
                word: entry.word.clone(),
                lang: entry.lang.clone(),
                round,
                verdict: "rejected".into(),
                issues_json: "[]".into(),
                detail: e.clone(),
                before_json: before_json.clone(),
                after_json: before_json.clone(),
                model: state.cfg().llm.model.clone(),
                elapsed_ms: started.elapsed().as_millis() as i64,
                at: now,
            };
            let _ = state.db.log_audit(&row);
            return Err(e);
        }
    };

    let mut verdict = match first.verdict.as_str() {
        "ok" => AuditVerdict::Ok,
        "rejected" => AuditVerdict::Rejected,
        _ => AuditVerdict::Fixed,
    };
    let mut issues = first.issues.clone();
    let mut detail = first.detail.clone();
    let mut corrected = first.corrected.clone();
    let mut after_json = before_json.clone();

    // 修正：写回词条后**再检一轮**，仍然不合格就标记剔除。
    if verdict == AuditVerdict::Fixed {
        if let Some(fix) = corrected.as_ref() {
            // ★ 只接受「同一个词、同一个语言」的修正。模型偶尔会把词目也改掉
            //   （比如自作主张纠正成另一个词），那等于把 A 词的数据写成 B 词。
            let sane = !fix.word.trim().is_empty()
                && fix.word.trim().eq_ignore_ascii_case(entry.word.trim())
                && fix.lang == entry.lang;
            if sane {
                let json = serde_json::to_string(fix).unwrap_or_default();
                if state
                    .db
                    .update_word_entry_json(&entry.word, &entry.lang, &json)
                    .unwrap_or(false)
                {
                    after_json = json;
                } else {
                    detail = format!("{}（修正写回失败，未改库）", detail);
                    verdict = AuditVerdict::Rejected;
                }
            } else {
                detail = format!("模型给的修正换了词目或语言，已拒绝采纳：{}", detail);
                verdict = AuditVerdict::Rejected;
                corrected = None;
            }
        } else {
            // 说有问题却不给修正 → 按不合格处理，别留个「待修」悬着
            detail = format!("{}（模型未给出修正内容）", detail);
            verdict = AuditVerdict::Rejected;
        }

        if verdict == AuditVerdict::Fixed {
            let fresh = state
                .db
                .get_word(&entry.word, &entry.lang)
                .ok()
                .flatten()
                .unwrap_or_else(|| entry.clone());
            match run_round(state, &fresh, round + 1).await {
                Ok(second) => {
                    verdict = match second.verdict.as_str() {
                        "ok" | "fixed" => AuditVerdict::Fixed,
                        _ => AuditVerdict::Rejected,
                    };
                    if !second.issues.is_empty() {
                        issues = second.issues.clone();
                    }
                    if !second.detail.is_empty() {
                        detail = format!("{}；复检：{}", detail, second.detail);
                    }
                }
                Err(e) => {
                    verdict = AuditVerdict::Rejected;
                    detail = format!("{}；复检未通过：{}", detail, e);
                }
            }
        }
    }

    let elapsed = started.elapsed().as_millis() as i64;
    let row = crate::db::AuditRow {
        batch_id: String::new(),
        word: entry.word.clone(),
        lang: entry.lang.clone(),
        round,
        verdict: verdict.as_str().into(),
        issues_json: serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()),
        detail: detail.clone(),
        before_json,
        after_json,
        model: state.cfg().llm.model.clone(),
        elapsed_ms: elapsed,
        at: now,
    };
    let _ = state.db.log_audit(&row);
    let _ = state
        .db
        .set_audit_state(&entry.word, &entry.lang, verdict.as_str(), now);

    Ok(EntryAudit {
        word: entry.word.clone(),
        lang: entry.lang.clone(),
        round,
        verdict,
        issues,
        detail,
        corrected,
        model: state.cfg().llm.model.clone(),
        elapsed_ms: elapsed,
    })
}

/// 批量自检：扫「还没检过 / 检了没过」的词，逐个跑并广播进度。
///
/// 参照后台增强（`enrich.rs`）的做法：有停止开关、每词之间让出时间，
/// 避免把本地显存/CPU 打满导致界面卡死。
#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditBatchReport {
    pub batch_id: String,
    pub scanned: usize,
    pub ok: usize,
    pub fixed: usize,
    pub rejected: usize,
    pub failed: usize,
}

static STOP: AtomicBool = AtomicBool::new(false);
static DONE: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub async fn cmd_ai_audit_batch(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    limit: Option<usize>,
) -> Result<AuditBatchReport, String> {
    use tauri::Emitter;

    let cfg = state.cfg();
    let lang = lang
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cfg.target_lang.clone());
    if !cfg.study.ai_explain || cfg.llm.base_url.trim().is_empty() {
        return Err("还没配置本地大模型，无法做词条自检（可在设置页「本地大模型」一键部署）".into());
    }

    let words = state
        .db
        .pick_words_to_audit(&lang, limit.unwrap_or(50).min(500))
        .map_err(err)?;
    let batch_id = format!("b{}", timeutil::now_ts());
    let mut rep = AuditBatchReport {
        batch_id: batch_id.clone(),
        scanned: words.len(),
        ..Default::default()
    };

    STOP.store(false, Ordering::Relaxed);
    DONE.store(0, Ordering::Relaxed);
    TOTAL.store(words.len() as u64, Ordering::Relaxed);

    for (word, wlang) in words {
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        let Some(entry) = state.db.get_word(&word, &wlang).ok().flatten() else {
            continue;
        };
        match audit_one(&state, &entry, 1).await {
            Ok(a) => {
                match a.verdict {
                    AuditVerdict::Ok => rep.ok += 1,
                    AuditVerdict::Fixed => rep.fixed += 1,
                    AuditVerdict::Rejected => rep.rejected += 1,
                }
                let _ = app.emit(
                    "ai-audit://progress",
                    serde_json::json!({
                        "batchId": batch_id,
                        "done": DONE.fetch_add(1, Ordering::Relaxed) + 1,
                        "total": TOTAL.load(Ordering::Relaxed),
                        "word": a.word,
                        "verdict": a.verdict.as_str(),
                        "detail": a.detail,
                    }),
                );
            }
            Err(_) => {
                rep.failed += 1;
            }
        }
        // 每个词之间喘口气：本地模型是独占显存的资源，
        // 连着打会把界面卡住（后台增强用的是 10 秒，这里略短）
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }

    log::info!(
        "AI 自检批次 {} 完成：扫 {} 条，通过 {} / 修正 {} / 剔除 {} / 失败 {}",
        batch_id,
        rep.scanned,
        rep.ok,
        rep.fixed,
        rep.rejected,
        rep.failed
    );
    Ok(rep)
}

/// 停止当前批量自检。
#[tauri::command(async)]
pub fn cmd_ai_audit_stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// 批量自检进度。
#[tauri::command(async)]
pub fn cmd_ai_audit_status() -> serde_json::Value {
    serde_json::json!({
        "done": DONE.load(Ordering::Relaxed),
        "total": TOTAL.load(Ordering::Relaxed),
    })
}

/// 查自检日志（可按批次或词过滤）。
#[tauri::command(async)]
pub fn cmd_ai_audit_log(
    state: State<'_, Arc<AppState>>,
    batch_id: Option<String>,
    word: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<crate::db::AuditRow>, String> {
    state
        .db
        .audit_log(batch_id.as_deref(), word.as_deref(), limit.unwrap_or(200))
        .map_err(err)
}

/// 采纳某次自检给出的修正（界面在日志面板里点「采纳」时调用）。
#[tauri::command]
pub async fn cmd_ai_audit_apply(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<EntryAudit, String> {
    let cfg = state.cfg();
    let lang = lang
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cfg.target_lang.clone());
    let entry = state
        .db
        .get_word(&word, &lang)
        .map_err(err)?
        .ok_or_else(|| format!("词库里没有「{word}」这个词条"))?;
    audit_one(&state, &entry, 1).await
}
