//! Tauri 命令层：前端可调用的全部后端接口。
//!
//! 命名约定：`cmd_` 前缀便于在主入口统一注册。
//! 所有命令统一返回 `Result<T, String>`，错误转成中文文案直接给用户看。

pub mod books;
pub mod explain;
pub mod extra;
pub mod graph;
pub mod leech;
pub mod localllm;
pub mod maint;
pub mod plan;
pub mod storage;
pub mod translate;
pub mod tts;
pub mod update;

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
    let old_network = state.cfg().network;
    state
        .update_config(|c| {
            *c = config.clone();
        })
        .map_err(err)?;

    // 网络/代理配置变了就立刻重建 HTTP 客户端，省得让用户重启应用。
    let n = &config.network;
    let changed = n.enable_proxy != old_network.enable_proxy
        || n.proxy != old_network.proxy
        || n.use_system_proxy != old_network.use_system_proxy
        || n.no_proxy != old_network.no_proxy
        || n.timeout_secs != old_network.timeout_secs
        || n.connect_timeout_secs != old_network.connect_timeout_secs;
    if changed {
        if let Err(e) = state.reload_http() {
            log::warn!("代理配置已保存，但重建网络客户端失败：{}", e);
        }
    }
    Ok(())
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
    Ok(llm::status(&state.http(), &cfg.llm).await)
}

/// 探测服务并自动选用第一个可用模型，写回配置。
#[tauri::command]
pub async fn cmd_llm_autoconnect(state: State<'_, Arc<AppState>>) -> Result<llm::LlmStatus, String> {
    let cfg = state.cfg();
    let st = llm::status(&state.http(), &cfg.llm).await;
    if st.online && !st.active_model.is_empty() {
        let model = st.active_model.clone();
        state
            .update_config(|c| c.llm.model = model.clone())
            .map_err(err)?;
    }
    Ok(st)
}

/// AI 讲解的返回值。
///
/// ★ 之所以从一个字符串变成结构体：前端需要**同时**拿到「最终展示文本」和
/// 「模型原始输出」，才能提供「查看原文」对照（需求 4）。只返回字符串的话，
/// 译文替换掉原文后就再也拿不回来了。
#[derive(Debug, Clone, Serialize)]
pub struct ExplainResult {
    pub word: String,
    /// 最终展示文本（已按 `explain_lang` 处理过）
    pub text: String,
    /// 模型原始输出（未翻译）
    pub original: String,
    /// 本次生效的讲解语言
    pub lang: String,
    /// 是否经过了后处理层的兜底翻译
    pub translated: bool,
    /// 兜底翻译失败时的提示（此时 `text == original`，前端应如实告知）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 解析本次要用的讲解语言：配置为空时退回中文。
fn explain_lang_of(cfg: &AppConfig) -> String {
    let l = cfg.explain_lang.trim();
    if l.is_empty() {
        "zh".to_string()
    } else {
        l.to_string()
    }
}

/// 后处理兜底：模型没按目标语言输出时，套翻译模板重译整段讲解。
///
/// 返回 `(最终文本, 是否翻译过, 提示)`。**任何失败都降级为「保留原文」**，
/// 宁可显示原文也不要因为一次翻译失败让用户看不到讲解。
async fn apply_explain_lang(
    state: &AppState,
    cfg: &AppConfig,
    text: String,
    explain_lang: &str,
) -> (String, bool, Option<String>) {
    if !cfg.explain_auto_translate {
        return (text, false, None);
    }
    if llm::text_matches_lang(&text, explain_lang) {
        return (text, false, None);
    }
    match llm::translate_markdown(
        &state.http(),
        &cfg.llm,
        &cfg.explain_translate_template,
        &text,
        explain_lang,
    )
    .await
    {
        Ok(t) if !t.trim().is_empty() => (t, true, None),
        Ok(_) => (text, false, Some("翻译返回了空内容，已保留原文".into())),
        Err(e) => (
            text,
            false,
            Some(format!("自动翻译失败，已保留原文（{}）", e)),
        ),
    }
}

/// AI 讲解单词（流式推送）。
///
/// 通过 `explain://delta` 事件把增量文本推给前端，实现打字机效果；
/// 命令本身返回完整结果，便于前端做兜底与「原文/译文」切换。
///
/// 分层说明（需求 1 / 2）：
///   1. **提示词层**：`with_lang_constraint` 把《输出语言》约束追加进 system，
///      让模型**直接**用目标语言输出 —— 零额外延迟，也保住了流式打字机效果。
///   2. **输出后处理层**：流式结束后做一次语言检测，模型没听话时套
///      [`llm::translate_markdown`] 的翻译模板重译整段并替换原文。
///      这一步保证「选了就一定生效」，代价是多一次模型调用。
#[tauri::command]
pub async fn cmd_ai_explain(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
    question: Option<String>,
) -> Result<ExplainResult, String> {
    use tauri::Emitter;

    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let explain_lang = explain_lang_of(&cfg);

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

    // 提示词层：把语言要求注入 system（用户可编辑的人设不做改动，只追加）
    let system = llm::with_lang_constraint(&cfg.llm.system_prompt, &explain_lang);

    let app2 = app.clone();
    let word2 = word.clone();

    let full = llm::chat_stream(&state.http(), &cfg.llm, &system, &user_prompt, |d, kind| {
        // 忽略发送失败（前端可能已关闭面板）
        // kind 区分正文与思考过程，前端可分别渲染
        let channel = match kind {
            llm::DeltaKind::Content => "explain://delta",
            llm::DeltaKind::Reasoning => "explain://reasoning",
        };
        let _ = app2.emit(channel, serde_json::json!({ "word": word2, "delta": d }));
    })
    .await
    .map_err(err)?;

    let original = full.clone();
    // 输出后处理层兜底
    let (text, translated, note) =
        apply_explain_lang(&state, &cfg, original.clone(), &explain_lang).await;

    let out = ExplainResult {
        word: word.clone(),
        text: text.clone(),
        original,
        lang: explain_lang,
        translated,
        note,
    };

    // 顺手存档（用户需求 C：讲解也算存储，能在词库里搜到）。
    //
    // 只在**首次讲解**时落库：追问（`question` 非空）的答案是对同一个词的
    // 补充说明，存进去会用「追问的答复」把主讲解覆盖掉，得不偿失。
    // 存档是尽力而为的，失败不该影响讲解本身，所以忽略错误。
    if question.as_deref().map(|q| q.trim().is_empty()).unwrap_or(true) {
        let now = timeutil::now_ts();
        let row = db::ExplainRow {
            word: word.trim().to_lowercase(),
            lang: lang.clone(),
            explain_lang: out.lang.clone(),
            text: out.text.clone(),
            original: out.original.clone(),
            translated: out.translated,
            entry_json: String::new(),
            saved: false,
            updated_at: now,
        };
        let _ = state.db.save_explain(&row, now);
    }

    // 前端据此整体替换（流式期间显示的是可能未经翻译的增量文本）
    let _ = app.emit("explain://done", serde_json::to_value(&out).unwrap_or_default());
    Ok(out)
}

/// 非流式讲解（用于导出、批量生成）。
#[tauri::command]
pub async fn cmd_ai_explain_sync(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<ExplainResult, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let explain_lang = explain_lang_of(&cfg);
    let entry = state
        .db
        .get_word(&word, &lang)
        .ok()
        .flatten()
        .unwrap_or_else(|| WordEntry::new(&word));
    let prompt = llm::explain_prompt(&entry, "");
    let system = llm::with_lang_constraint(&cfg.llm.system_prompt, &explain_lang);
    let original = llm::chat(&state.http(), &cfg.llm, &system, &prompt)
        .await
        .map_err(err)?;
    let (text, translated, note) =
        apply_explain_lang(&state, &cfg, original.clone(), &explain_lang).await;
    // 与流式版一致：顺手存档，失败不影响返回
    let now = timeutil::now_ts();
    let row = db::ExplainRow {
        word: word.trim().to_lowercase(),
        lang: lang.clone(),
        explain_lang: explain_lang.clone(),
        text: text.clone(),
        original: original.clone(),
        translated,
        entry_json: String::new(),
        saved: false,
        updated_at: now,
    };
    let _ = state.db.save_explain(&row, now);

    Ok(ExplainResult {
        word: word.clone(),
        text,
        original,
        lang: explain_lang,
        translated,
        note,
    })
}

/// 手动把一段文本翻成指定语言（前端在「切换讲解语言」时调用）。
///
/// 与 [`cmd_ai_explain`] 的区别：不重新生成讲解，只对**已有的原文**重译，
/// 所以切换语言几乎是秒回，也不会因为重生成而改变讲解内容。
#[derive(Debug, Clone, Serialize)]
pub struct TranslateResult {
    pub text: String,
    pub original: String,
    pub lang: String,
    pub translated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[tauri::command]
pub async fn cmd_translate_text(
    state: State<'_, Arc<AppState>>,
    text: String,
    lang: Option<String>,
) -> Result<TranslateResult, String> {
    let cfg = state.cfg();
    let explain_lang = lang
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| explain_lang_of(&cfg));
    let original = text.clone();
    let (out, translated, note) =
        apply_explain_lang(&state, &cfg, original.clone(), &explain_lang).await;
    Ok(TranslateResult {
        text: out,
        original,
        lang: explain_lang,
        translated,
        note,
    })
}

/// 切换 AI 讲解语言并**立即落盘**（需求 5：选择后即时生效、记住上次选择）。
#[tauri::command]
pub fn cmd_set_explain_lang(
    state: State<'_, Arc<AppState>>,
    lang: String,
) -> Result<String, String> {
    let lang = lang.trim().to_string();
    if lang.is_empty() {
        return Err("讲解语言不能为空".into());
    }
    state
        .update_config(|c| c.explain_lang = lang.clone())
        .map_err(err)?;
    Ok(lang)
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
    llm::generate_entry(&state.http(), &cfg.llm, &word, &lang)
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
    let word = word.trim().to_string();

    if word.is_empty() {
        return Err("请输入要查询的单词".into());
    }

    // ---- 语言判定（需求 3：整条链路必须一致） ----
    //
    // 优先级：
    //   1. 调用方显式传入的 lang（会话内临时指定）；
    //   2. 配置里的**源语言**（顶部方向选择器的左半边），`auto` 表示交给检测；
    //   3. 检测结果 —— 只在没被显式指定时才用。
    //
    // 这里刻意保留一条「以检测结果为准」的纠正规则：用户把方向选成
    // 「日语 → 中文」却又输入了中文词时，硬按日语去查只会拿到一堆
    // 英文解释（这正是用户报的「选日语却返回英语」）。所以当书写系统能
    // **强判定**（中日韩俄等）且与用户所选冲突时，以检测为准，并把这次
    // 纠正通过 `lang_note` 如实告诉用户。
    let cfg_src = cfg.source_lang.trim().to_string();
    let requested = lang
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if cfg_src.is_empty() || cfg_src == crate::translate::AUTO {
                None
            } else {
                Some(cfg_src.clone())
            }
        });

    let detected = crate::dict::detect_lang(&word).map(|s| s.to_string());
    let (lang, lang_note) = match (detected.clone(), requested.clone()) {
        // 没指定源语言：用检测结果，检测不出（纯拉丁字母）默认英语
        (Some(d), None) => (d, None),
        (None, None) => ("en".to_string(), None),
        // 指定了且与检测一致：正常
        (Some(d), Some(r)) if d == r => (d, None),
        // 指定了但检测是强判定且不一致：以检测为准并提示
        (Some(d), Some(r)) => {
            let note = format!(
                "检测到输入的是{}，已按{}查询（方向里的源语言是{}）",
                crate::translate::lang_name(&d),
                crate::translate::lang_name(&d),
                crate::translate::lang_name(&r)
            );
            (d, Some(note))
        }
        // 检测不出（英/法/德/西… 都是拉丁字母）：尊重用户选择
        (None, Some(r)) => {
            // 但有一条**脚本排他性**规则高于用户选择：纯拉丁字母的词不可能
            // 是中文 / 日文 / 韩文 / 俄文。典型现场 —— 方向选成「中文 → 英语」
            // 后输入 `reality`，硬按中文查会命中《现代汉语规范词典》这个中文
            // 专用源，释义全落空、音标还被标成「拼音」，也就是「明显是英文却
            // 识别成中文」。这里纠正为英语并如实说明。
            // 法语 / 德语 / 西语等拉丁语言之间无法靠字形区分，仍然尊重用户。
            if crate::dict::is_non_latin_lang(&r) && crate::dict::is_latin_only(&word) {
                let note = format!(
                    "检测到输入的是拉丁字母词汇，已按{}查询（方向里的源语言是{}）",
                    crate::translate::lang_name("en"),
                    crate::translate::lang_name(&r)
                );
                ("en".to_string(), Some(note))
            } else {
                (r, None)
            }
        }
    };

    if force_refresh.unwrap_or(false) {
        // 强制刷新时清掉这个词的缓存
        state.db.clear_cache().ok();
    }

    let ttl = 7 * 24 * 3600; // 词义一周内基本不变
    let allow_llm = cfg.study.ai_explain;

    // 大模型兜底闭包：先尝试用 LLM 生成（离线可用）
    // 注意：这里克隆一份 word / lang 交给闭包，避免 move 之后主流程无法再借用。
    let http = state.http();
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

    // 有代理时才去碰 wiktionary 这类国内直连不通的源
    let proxy_active = !state.proxy_info().is_direct();

    let result = dict::lookup_with_cache(
        &state.db,
        &word,
        &lang,
        &cfg.dict_sources,
        &state.http(),
        ttl,
        allow_llm,
        &cfg.network,
        proxy_active,
        fut,
    )
    .await
    .map_err(err)?;

    let mut result = result;
    result.lang_note = lang_note;

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
    Ok(search::suggest(&state.http(), &query, &lang).await)
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
    // 维基百科（wikipedia.org）在国内直连必然超时；没有代理时直接跳过，
    // 否则用户要为此白白多等 8~16 秒。
    let wiki_ok = with_wiki.unwrap_or(true) && !state.proxy_info().is_direct();
    search::search(&state.http(), &query, &lang, wiki_ok)
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
    if state.proxy_info().is_direct() {
        // 直连环境下 wikipedia.org 一定连不上，直接返回空而不是干等超时
        return Ok(None);
    }
    Ok(search::wiki_summary(&state.http(), &word, &lang).await)
}

/// 生成辞书跳转链接（需求 14）：本地没有的释义，一键去权威辞书查看。
#[tauri::command]
pub fn cmd_dict_links(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<Vec<search::DictLink>, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    Ok(search::dict_links(&word, &lang))
}

/// 用系统默认浏览器打开外部链接（辞书跳转用）。
#[tauri::command]
pub fn cmd_open_url(url: String) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("只允许打开 http(s) 链接".into());
    }
    open_in_browser(u).map_err(err)
}

/// 跨平台打开浏览器。
fn open_in_browser(url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
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
    // 前端（单词列表顶部的搜索框）不传 lang —— 语义是「跨语言搜」。
    // 若这里退化成当前学习语言，日语/中文词条的中文释义就永远搜不到。
    let lang = lang.filter(|l| !l.trim().is_empty() && l != "all");
    state
        .db
        .search_words(lang.as_deref(), &query, limit.unwrap_or(50))
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
            &state.http(),
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
    def_lang: Option<String>,
) -> Result<SessionInfo, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = timeutil::now_ts();
    let limit = size.unwrap_or(cfg.study.batch_size).clamp(1, 200);
    let mode = mode.unwrap_or_default();
    let leech_only = leech_only.unwrap_or(false);
    let def_lang = def_lang.unwrap_or_default();

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
        s.def_lang = def_lang;
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
    let (entry, mode, index, is_leech, total, correct, wrong, def_lang) = {
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
            s.def_lang.clone(),
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
        &def_lang,
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
    def_lang: &str,
) -> Result<QuizCard, String> {
    // 题面/答案里的「释义」统一走 def_lang：背日语词库时显示中文释义，
    // 而不是词典源顺手返回的英文解释。
    let def = || entry.definition_in(def_lang);
    let (prompt, answer) = match mode {
        QuizMode::EnToZh => {
            let a = def();
            let a = if a.trim().is_empty() {
                "（暂无释义，点击查询）".to_string()
            } else {
                a
            };
            (entry.word.clone(), a)
        }
        QuizMode::ZhToEn => {
            let q = def();
            let q = if q.trim().is_empty() {
                entry.word.clone()
            } else {
                q
            };
            (q, entry.word.clone())
        }
        // 拼写：题面给释义，答案是要拼的单词
        QuizMode::Spelling | QuizMode::ListenSpell => {
            let q = def();
            let q = if q.trim().is_empty() {
                "（暂无释义）".to_string()
            } else {
                q
            };
            (q, entry.word.clone())
        }
        // 例句选义：题面是挖空例句，答案是释义
        QuizMode::ExToZh => {
            let a = def();
            let a = if a.trim().is_empty() {
                "（暂无释义）".to_string()
            } else {
                a
            };
            let q = entry
                .first_example()
                .map(|(t, _)| mask_word(&t, &entry.word))
                .unwrap_or_else(|| entry.word.clone());
            (q, a)
        }
        // 例句识词：题面是挖空例句，答案是单词
        QuizMode::ExPickWord => {
            let q = entry
                .first_example()
                .map(|(t, _)| mask_word(&t, &entry.word))
                .unwrap_or_else(|| entry.definition_in(def_lang));
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
            QuizMode::EnToZh | QuizMode::ExToZh => p.definition_in(def_lang),
            _ => p.word.clone(),
        };
        if cand.trim().is_empty() || cand == answer || options.contains(&cand) {
            continue;
        }
        options.push(cand);
    }

    // 例句相关字段
    let (ex_raw, ex_trans) = entry.first_example().unwrap_or_default();
    let ex_masked = if ex_raw.is_empty() {
        String::new()
    } else {
        mask_word(&ex_raw, &entry.word)
    };

    // 拼写提示：初始只给出字母个数（用下划线占位），前端可再点「提示」逐步放开
    let spell_hint = if mode.is_typing() {
        entry
            .word
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { '_' })
            .collect::<Vec<_>>()
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        String::new()
    };

    Ok(QuizCard {
        prompt,
        answer,
        entry: entry.clone(),
        options,
        mode,
        is_leech,
        example_masked: ex_masked,
        example_raw: ex_raw,
        example_translation: ex_trans,
        spell_hint,
        audio: entry.phonetic.audio.clone(),
        index: progress.index,
        total: progress.total,
        correct_count: progress.correct,
        wrong_count: progress.wrong,
    })
}

/// 把句子里的目标词替换为 ____（大小写不敏感，兼顾词形变化）。
fn mask_word(sentence: &str, word: &str) -> String {
    let s = sentence;
    let w = word.trim();
    if w.is_empty() {
        return s.to_string();
    }
    let lower = s.to_lowercase();
    let wl = w.to_lowercase();
    if let Some(pos) = lower.find(&wl) {
        let mut out = String::with_capacity(s.len());
        out.push_str(&s[..pos]);
        out.push_str("____");
        out.push_str(&s[pos + wl.len()..]);
        return out;
    }
    // 词形变化：用词干前 3/4 再试一次
    if wl.len() > 4 {
        let stem = &wl[..(wl.len() * 3 / 4)];
        if let Some(pos) = lower.find(stem) {
            let end = (pos + stem.len() + 2).min(s.len());
            let mut out = String::with_capacity(s.len());
            out.push_str(&s[..pos]);
            out.push_str("____");
            out.push_str(&s[end..]);
            return out;
        }
    }
    s.to_string()
}

/// 供 `commands::extra` 复用的公开构造入口（需求 4 进阶模式）。
#[allow(clippy::too_many_arguments)]
pub fn build_card_public(
    state: &AppState,
    entry: &WordEntry,
    mode: QuizMode,
    lang: &str,
    is_leech: bool,
    index: usize,
    total: usize,
    correct: i32,
    wrong: i32,
    def_lang: &str,
) -> Result<QuizCard, String> {
    build_card(
        state,
        entry,
        mode,
        lang,
        is_leech,
        CardProgress { index, total, correct, wrong },
        def_lang,
    )
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
        QuizMode::Spelling => "spelling",
        QuizMode::ExToZh => "ex_to_zh",
        QuizMode::ExPickWord => "ex_pick_word",
        QuizMode::ListenSpell => "listen_spell",
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
    // 手动测试源时不做 needs_proxy 过滤、也不看 enabled：
    // 用户就是想确认这个源到底能不能用。
    let mut probe = source.clone();
    probe.enabled = true;
    let net_cfg = state.cfg().network;
    let hit = dict::lookup_multi(
        &test_word,
        &lang,
        std::slice::from_ref(&probe),
        &state.http(),
        net_cfg.lookup_timeout_secs.clamp(3, 30),
        0,
        true,
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
