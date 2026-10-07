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
    /// 本次讲解补充用到的**联网参考资料**（需求 5）。
    ///
    /// 本地词库查不到的例句 / 派生变形由模型补全，而这段补全的依据就是这里。
    /// 前端独立渲染成「来源」清单（点击在应用内打开），用户能自己核对
    /// 模型有没有编 —— 这比在正文里写「仅供参考」有用得多。
    ///
    /// 空列表序列化时会被跳过，前端拿到 `undefined` 即视为「本次没联网」。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web_refs: Vec<crate::search::websearch::WebResult>,
}

/// 判断这次讲解要不要联网补充资料（需求 5）。
///
/// 触发条件是「本地资料明显不够用」，而不是无条件联网，理由有两条：
///   1. 一次搜索是 0.5~2 秒的额外等待，而词库里的常用词条本来就很完整，
///      为了补一条用不上的资料让所有查词都变慢，不划算；
///   2. 联网检索有失败率（断网、被墙、引擎改版），能不发就不发。
///
/// 三种「不够用」：一条释义都没有（查不到）；有释义但**所有**义项都没有
/// 例句（最常见的「查不到例句」）；既没有变形也没有相关词（派生与词汇
/// 网络是需求 4/5 要补的重点）。
pub fn explain_needs_web(entry: &WordEntry) -> bool {
    if entry.senses.is_empty() {
        return true;
    }
    if entry.senses.iter().all(|s| s.examples.is_empty()) {
        return true;
    }
    entry.inflections.is_empty() && entry.related.is_empty()
}

/// 是否允许为讲解发起联网检索。
///
/// 拆成独立函数是为了让「用户关掉了在线搜索」这条边界**可被单测钉住**：
/// 它是用户可感知的隐私/流量承诺，不能因为将来某次重构被顺手绕过。
pub fn explain_web_allowed(cfg: &AppConfig, word: &str) -> bool {
    cfg.web_search_enabled && !word.trim().is_empty()
}

/// 抓取讲解用的联网资料。**尽力而为**：任何失败都返回空列表，
/// 绝不因为联网问题让用户看不到讲解。
async fn fetch_explain_refs(
    state: &AppState,
    cfg: &AppConfig,
    word: &str,
) -> Vec<crate::search::websearch::WebResult> {
    use crate::search::websearch::{self, SearchEngine};

    if !explain_web_allowed(cfg, word) {
        return Vec::new();
    }
    let word = word.trim();

    // 查询词带上「例句 / 用法」是一箭双雕：既让引擎优先给出词典类页面
    // （必应会挑出剑桥/牛津的结果），也顺手把「这个词怎么用」一起搜了。
    let query = format!("{} 释义 例句 用法", word);
    let engine = SearchEngine::parse(&cfg.search_engine);

    match websearch::web_search(&state.http(), &query, engine, 5).await {
        Ok(hits) => hits,
        Err(_) => Vec::new(),
    }
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

    // 需求 5：本地查不到例句 / 派生变形时，先联网抓一份**真实**资料，
    // 作为模型补充内容的唯一依据（见 EXPLAIN_WEB_REF_CONSTRAINT）。
    //
    // 追问不重复抓：主讲解那次已经抓过，而追问的上下文就是主讲解本身。
    let is_follow_up = question
        .as_deref()
        .map(|q| !q.trim().is_empty())
        .unwrap_or(false);
    let web_refs = if is_follow_up || !explain_needs_web(&entry) {
        Vec::new()
    } else {
        fetch_explain_refs(&state, &cfg, &word).await
    };

    let mut user_prompt = match &question {
        Some(q) if !q.trim().is_empty() => {
            format!(
                "单词：{}\n用户的追问：{}\n\n请针对该问题作答，若需要可结合该词的其他用法补充说明。",
                word, q
            )
        }
        _ => llm::explain_prompt(&entry, ""),
    };
    if !web_refs.is_empty() {
        let pairs: Vec<(String, String)> = web_refs
            .iter()
            .map(|r| (r.title.clone(), r.snippet.clone()))
            .collect();
        user_prompt = llm::with_web_refs(&user_prompt, &pairs);
    }

    // 提示词层：把语言要求注入 system（用户可编辑的人设不做改动，只追加）。
    //
    // 注入顺序有讲究：**语言约束必须留在最后** —— 它自称「最高优先级、
    // 覆盖上面所有冲突的要求」，只有真的排在末尾才成立。资料约束插在它之前。
    let mut system = cfg.llm.system_prompt.clone();
    if !web_refs.is_empty() {
        system = llm::with_web_ref_constraint(&system, &explain_lang);
    }
    let system = llm::with_lang_constraint(&system, &explain_lang);

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
        web_refs,
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
        // 批量导出**刻意不联网**：一次导出可能几十上百个词，等于拿搜索引擎
        // 当 API 刷，既慢又容易被限流。交互式讲解（cmd_ai_explain）才补资料。
        web_refs: Vec::new(),
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

/// 一个同族派生词（需求 5）。
#[derive(Debug, Clone, Serialize)]
pub struct Derivative {
    pub word: String,
    /// 词性，如 `n.`
    pub pos: String,
    /// 主要释义（一句话）
    pub definition: String,
}

/// 同族派生词：由词形规则生成候选，再**去本地词库确认**哪些真的存在。
///
/// 需求 5 的「相关词太少，缺少 happiness 等派生变形」就是靠它补的。
///
/// 为什么不用 AI / 不联网：这里的诉求是「把词库里已有的同族词捞出来」。
/// 用户词库里本来就同时收录了 happy 和 happiness，只是查 happy 时没人提到
/// happiness。走本地确认还有三个好处：瞬间返回、离线可用、**绝不编造** ——
/// 凭规则写出 happiness 很容易，但用户点下去发现查不到，比不显示更糟。
///
/// 上限 8 条：同族词是**补充信息**，塞满一屏就把释义挤没了。
#[tauri::command]
pub fn cmd_word_family(
    state: State<'_, Arc<AppState>>,
    word: String,
    lang: Option<String>,
) -> Result<Vec<Derivative>, String> {
    const MAX_FAMILY: usize = 8;

    let lang = lang
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| state.cfg().target_lang.clone());

    let cands = crate::morph::derivative_candidates(&word);
    if cands.is_empty() {
        return Ok(Vec::new());
    }
    let briefs = state.db.existing_word_briefs(&cands, &lang).map_err(err)?;

    // SQL 的 `IN` 不保证返回顺序，这里按**候选生成顺序**重排 —— 它是按
    // 「派生可能性」排的（先后缀、再变形还原），打乱后 happier 跑到
    // happiness 前面会显得毫无道理。
    let mut out: Vec<Derivative> = Vec::new();
    for c in &cands {
        if out.len() >= MAX_FAMILY {
            break;
        }
        if let Some((w, pos, def)) = briefs.iter().find(|(w, _, _)| w.eq_ignore_ascii_case(c)) {
            out.push(Derivative {
                word: w.clone(),
                pos: pos.clone(),
                definition: def.clone(),
            });
        }
    }
    Ok(out)
}

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

/// 目标语言侧最多取几个候选对应词回查词典。
///
/// 译义词有了crow、rook、raven 三个候选，全查一遍既慢又乱；
/// 取 3 个已经能覆盖「crow 或者其他能表示乌鸦的单词」这类需求。
const MAX_PAIRS: usize = 3;

/// 双向词条：取回**目标语言侧**的对应词及其完整词条。
///
/// 用户需求原文：
/// > 「这里我是中文转英文，应该下面详细介绍的是 crow 或者其他能表示乌鸦的单词」
/// > 「仿照有道词典两者都有，不然我输入英文的时候没有英文解释对吧，还有其他语言也要类似」
///
/// 流程：`翻译（源 → 目标）` → `拿主译文 + 候选译法` → `并发按目标语言查词典`。
///
/// 两端的操作都由 `dict::lookup_multi` 完成，所以对任意语言对都成立
/// （中→英拿到 crow 的英文详解，英→中拿到「现实」的中文详解），
/// 而不是为某一种语言写死的特例。
///
/// ★ 失败语义：**这里永远不返回 Err**。翻译失败、联网失败、目标语言里查不到，
/// 都只是「这次没有对应词条」，主词条是完整的。返回 Err 会让前端把它当成
/// 「查词失败」弹红字，那正是用户抱怨过的「明明查到了却报失败」。
#[tauri::command]
pub async fn cmd_lookup_pairs(
    state: State<'_, Arc<AppState>>,
    word: String,
    from: Option<String>,
    to: Option<String>,
    already_translated: Option<String>,
    already_alternatives: Option<Vec<String>>,
) -> Result<Vec<dict::PairEntry>, String> {
    let cfg = state.cfg();
    let word = word.trim().to_string();
    if word.is_empty() {
        return Ok(vec![]);
    }

    let from = from
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.source_lang.trim().to_string());
    let to = to
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.target_lang.trim().to_string());

    // 同语言就没什么「对应词」可言：主词条本身已经是目标语言的内容了
    if from.is_empty() || to.is_empty() || from == to {
        return Ok(vec![]);
    }

    // 1) 候选对应词。
    //
    // 前端往往**已经**为了「词级译文」翻过一次（有道接口限频很严，同一个
    // 656 字符内的词再来一次请求极易撞上 429/411）。所以允许把现成译文传进来，
    // 传了就直接用，绝不再打一次翻译接口；没传才自己去翻。
    let mut cands: Vec<String> = Vec::new();
    if let Some(t) = already_translated {
        for c in std::iter::once(t).chain(already_alternatives.unwrap_or_default()) {
            let c = c.trim().to_string();
            if !c.is_empty() && !cands.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
                cands.push(c);
            }
        }
    } else {
        match translate::translate_core(&state, &word, &from, &to, false).await {
            Ok(t) => {
                for c in std::iter::once(t.text).chain(t.alternatives) {
                    let c = c.trim().to_string();
                    if !c.is_empty() && !cands.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
                        cands.push(c);
                    }
                }
            }
            Err(e) => {
                eprintln!("[pairs] 取对应词失败（{} → {}）：{}", from, to, e);
                return Ok(vec![]);
            }
        }
    }
    cands.truncate(MAX_PAIRS);
    if cands.is_empty() {
        return Ok(vec![]);
    }

    // 3) 并发按目标语言回查词典。
    //
    // 串行跑会把「单次查询预算 × 候选数」累加成用户肉眼可见的等待
    // （3 个候选 × 8 秒 = 半分钟），所以这里一次性放出去。
    let proxy_active = !state.proxy_info().is_direct();
    let timeout = cfg.network.lookup_timeout_secs.clamp(3, 30);
    let http = state.http();
    let srcs = cfg.dict_sources.clone();

    let jobs = cands.iter().enumerate().map(|(i, c)| {
        let http = http.clone();
        let srcs = srcs.clone();
        let to = to.clone();
        let c = c.clone();
        async move {
            let mut tr = Vec::new();
            let got =
                dict::lookup_multi(&c, &to, &srcs, &http, timeout, 0, proxy_active, &mut tr).await;
            (i, c, got)
        }
    });

    let mut out: Vec<dict::PairEntry> = Vec::new();
    for (i, c, got) in futures_util::future::join_all(jobs).await {
        let Some((mut e, _)) = got else { continue };
        if !dict::is_meaningful(&e) {
            continue; // 翻译出了词，但目标语言词典里查不到有效内容
        }
        // 词头优先用词典自己给出的规范形式：翻译接口常常返回首字母大写的
        // 「Crow」，直接拿来当词头会让用户以为这是个专有名词。
        // 词典没给出时才退回译文原文。
        let headword = if e.word.trim().is_empty() {
            c.clone()
        } else {
            e.word.trim().to_string()
        };
        e.word = headword.clone();
        e.lang = to.clone();
        out.push(dict::PairEntry {
            word: headword,
            lang: to.clone(),
            entry: e,
            via: if i == 0 {
                "translation".to_string()
            } else {
                "alternative".to_string()
            },
        });
    }
    Ok(out)
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

    // 本地词库的模糊候选排最前。三个理由：
    //   ① 毫秒级、离线可用（在线联想在无网时是空的）；
    //   ② 都是用户「正在学 / 学过」的词，相关性天然更高；
    //   ③ 带编辑距离纠错 —— 用户拼写不准时（signifiance → significance）
    //      在线联想给的往往是「查不到」。
    // 语言按**源语言**过滤（输入的语言），而不是 target_lang ——
    // 后者用于在线联想（它需要知道要联想成哪国话）。
    let src = cfg.source_lang.trim();
    let local_lang = if src.is_empty() || src == crate::translate::AUTO { "" } else { src };
    let mut out: Vec<search::Suggestion> = state
        .db
        .fuzzy_candidates(&query, local_lang, 6)
        .unwrap_or_default()
        .into_iter()
        .map(|w| search::Suggestion {
            word: w,
            gloss: String::new(),
            source: "词库".into(),
        })
        .collect();

    let online = search::suggest(&state.http(), &query, &lang).await;
    for s in online {
        if out.len() >= 10 {
            break;
        }
        if !out.iter().any(|x| x.word.eq_ignore_ascii_case(&s.word)) {
            out.push(s);
        }
    }
    Ok(out)
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
pub fn cmd_open_url(
    state: State<'_, Arc<AppState>>,
    app: tauri::AppHandle,
    url: String,
    force_external: Option<bool>,
) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("只允许打开 http(s) 链接".into());
    }
    // 「链接在软件内打开」开关（设置 → 学习偏好）。分流收在**命令层**：
    // 前端所有调用点（权威辞书、词库资料源、库内链接…）无需各自判断，
    // 行为随开关全局切换。`force_external` 给「必须离开软件」的动作
    // （下载安装包更新）留一条直通系统浏览器的路 —— 应用内 WebView
    // 接不住安装包下载。
    if !force_external.unwrap_or(false) && state.cfg().study.open_links_in_app {
        return cmd_open_in_app(app, url);
    }
    open_in_browser(&app, u).map_err(err)
}

/// 在应用内打开网页（需求 11：在线搜索不再跳到系统浏览器）。
///
/// 与 `cmd_open_url` 的分工：`cmd_open_url` 一律弹系统浏览器（「权威词典」
/// 那排链接仍走它，行为不变）；本命令**桌面端**开应用内浏览窗口，**移动端**
/// 没有多窗口语义，回退到系统浏览器。白名单校验与 `cmd_open_url` 完全一致。
#[tauri::command]
pub fn cmd_open_in_app(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("只允许打开 http(s) 链接".into());
    }
    crate::webview::open_in_app(&app, u)
}

/// 跨平台打开浏览器。
///
/// 走 tauri-plugin-opener，而不是自己 spawn 外部程序：
///  - Windows 要 `cmd /C start`，macOS 要 `open`，桌面 Linux 要 `xdg-open`；
///  - **Android 是 unix 但不是桌面 Linux**，上面这三个它一个都没有。
///    按 `cfg(all(unix, not(macos)))` 分派会落到 `xdg-open` 上，
///    在那里 spawn 一个不存在的程序 —— 表现是点了链接毫无反应。
///    插件在移动端走 Intent，这才是正确路径。
///
/// 可见性为 `pub(crate)`：桌面端的 `cmd_open_url` 与移动端的
/// `webview::open_in_app` 都要用它，逻辑本身未变。
pub(crate) fn open_in_browser(app: &tauri::AppHandle, url: &str) -> anyhow::Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>)?;
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
    // 会话类型：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽（需求 14）。
    kind: Option<String>,
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

    // 写入会话（按 kind 选槽位：背诵与复习互不覆盖）
    {
        let slot = state.session_slot(kind.as_deref());
        let mut s = slot.write();
        s.queue = picked;
        s.index = 0;
        s.mode = Some(mode);
        s.correct = 0;
        s.wrong = 0;
        s.started_at = now;
        s.leech_only = leech_only;
        s.def_lang = def_lang;
        // 新一轮不允许沿用上一轮的「答错回插」预算
        s.requeue_counts.clear();
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

/// 开始一轮**今日复习**（需求 14）。
///
/// 与 [`cmd_start_session`] 的区别（也是这轮需求的核心）：
/// - 队列**只由「今天到期（含逾期）」的词构成**，不掺新词、不补足到 `batch_size`。
///   入口按钮写的是「今天要复习 12 个」，点进去就必须是 12 个 —— 补足到 20
///   会让用户觉得数字是假的。
/// - 写入**独立的复习槽位** `state.review`，不会把正在进行的背诵会话清掉。
/// - `size` 只作为**截断上限**（`None` = 全部到期词），绝不用于补足。
/// - 一个到期的词都没有时返回 `total: 0`（前端据此提示「今天没有要复习的词」），
///   而不是报错 —— 「今天没到期」是正常状态。
#[tauri::command]
pub fn cmd_start_review_session(
    state: State<'_, Arc<AppState>>,
    // 复习不单独选模式，但**必须让调用方传**：原来是从背诵槽里读
    // `state.session.mode`，于是「界面选了拼写、复习却出看英选中」——
    // 题面与用户手上的模式对不上，答完还会按错的模式判分。
    mode: Option<QuizMode>,
    size: Option<usize>,
    lang: Option<String>,
    def_lang: Option<String>,
) -> Result<SessionInfo, String> {
    start_review_session_inner(&state, mode, size, lang, def_lang)
}

/// 只由「今天到期（含逾期）」构成的复习队列。
///
/// ★ **绝不补足**：到期的有几个就是几个。`size` 只是截断上限（`None` = 全部），
/// 与 [`cmd_start_session`] 会用新词/弱词补到 `batch_size` 的行为形成对比 ——
/// 复习入口上的数字必须和实际题目数一致。
fn build_review_queue(
    state: &AppState,
    lang: &str,
    size: Option<usize>,
) -> Result<Vec<WordEntry>, String> {
    // `None` 取一个「实际上等于无限」的上限（本地词库不可能有上万条今天到期）。
    const REVIEW_UNLIMITED: i64 = 10_000;
    let limit = size
        .map(|s| s as i64)
        .unwrap_or(REVIEW_UNLIMITED)
        .clamp(1, REVIEW_UNLIMITED);

    // 与 `cmd_due_words` 同一排序口径：逾期越久越靠前 → 其次按到期时间 → 最后强化词优先。
    // （`db.due_states` 内部只保证「强化词优先 + 到期时间」，这里补上「逾期天数」这一维，
    //   否则「逾期 5 天」会排在「今天到期」后面。）
    let now = timeutil::now_ts();
    let today_start = timeutil::today_start();
    let due = state.db.due_states(lang, now, limit).map_err(err)?;

    let mut ordered: Vec<(&StudyState, i64)> = due
        .iter()
        .map(|s| {
            // due_at 早于今天零点才算逾期，按整天算，避免「昨晚 23:59 到期」被算成逾期 1 天。
            let overdue_days = if s.due_at < today_start {
                timeutil::days_between(s.due_at, now).max(1)
            } else {
                0
            };
            (s, overdue_days)
        })
        .collect();
    ordered.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.0.due_at.cmp(&b.0.due_at))
            .then(b.0.is_leech.cmp(&a.0.is_leech))
    });

    let mut picked: Vec<WordEntry> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (st, _) in ordered {
        if seen.insert(st.word.clone()) {
            if let Some(e) = load_entry(state, &st.word, lang)? {
                picked.push(e);
            }
        }
    }
    Ok(picked)
}

/// 复习会话的内部实现（不依赖 Tauri 状态，便于单测）。
fn start_review_session_inner(
    state: &AppState,
    mode: Option<QuizMode>,
    size: Option<usize>,
    lang: Option<String>,
    def_lang: Option<String>,
) -> Result<SessionInfo, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = timeutil::now_ts();
    let def_lang = def_lang.unwrap_or_default();

    let picked = build_review_queue(state, &lang, size)?;
    let total = picked.len();

    // 复习沿用调用方传进来的模式；没传才退回背诵槽当前的模式。
    // （回退只在「程序化发起复习、没带模式」时发生，正常 UI 一定会传。）
    let mode = mode.unwrap_or_else(|| state.session.read().mode.unwrap_or_default());

    // 写入**独立的复习槽位**：正在进行的背诵会话不受影响（需求 14）。
    {
        let mut s = state.review.write();
        s.queue = picked;
        s.index = 0;
        s.mode = Some(mode);
        s.correct = 0;
        s.wrong = 0;
        s.started_at = now;
        s.leech_only = false;
        s.def_lang = def_lang;
        s.requeue_counts.clear();
    }

    // ★ total 可以为 0：不报错，交给前端提示「今天没有要复习的词」。
    Ok(SessionInfo {
        mode,
        total,
        index: 0,
        correct: 0,
        wrong: 0,
        leech_only: false,
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
    // 兜底 1：返回只有词的占位条目，前端会异步联网补全
    if let Some(e) = state.db.get_word(word, lang).map_err(err)? {
        return Ok(Some(e));
    }
    // 兜底 2：★ **跨语言**兜底。
    //   调用方（`build_review_queue` / `cmd_start_session`）是拿 `study_state.lang`
    //   来取词条的，而这两者的语言未必一致（历史数据里确实存在：状态记在 en、
    //   词条只在 ja）。少了这一步，队列会「数得到但取不出」，最终静默变成空
    //   队列 —— 界面表现为按钮上的数字和自己的提示语互相打脸。
    state.db.get_word_any_lang(word).map_err(err)
}

/// 取当前题目（含干扰项）。
#[tauri::command]
pub fn cmd_current_question(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    // 会话类型：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽（需求 14）。
    kind: Option<String>,
) -> Result<Option<QuizCard>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let (entry, mode, index, is_leech, total, correct, wrong, def_lang) = {
        let s = state.session_slot(kind.as_deref()).read();
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

/// 把配置里的选择题选项个数收敛到合法范围 `2..=8`。
///
/// 抽成函数是为了「配置可以直接手改 / 被旧版本写坏」这层防护只有一个入口，
/// 也方便单测覆盖边界（0、1、9、255…）。
fn normalize_option_count(raw: u32) -> usize {
    raw.clamp(2, 8) as usize
}

/// 打乱一个切片（Fisher–Yates）。
///
/// 后端也要打乱一次，不能只靠前端：某些模式（如拼写）前端并不走 `options`
/// 渲染，若后端顺序固定，选项的排列就会长期一致，看起来像「答案总在第二项」。
/// 两处都打乱是**刻意**的，不是冗余。
///
/// 不引入 `rand` 依赖：用当前时间做种子跑 xorshift 即可，足够随机且不增加
/// 编译负担。
fn shuffle_in_place<T>(items: &mut [T]) {
    let n = items.len();
    if n <= 1 {
        return;
    }
    let mut seed = (timeutil::now_ts() as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add((std::process::id() as u64) ^ 0x9E37_79B9_7F4A_7C15);
    for i in (1..n).rev() {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let j = (seed.wrapping_mul(0x2545_F491_4F6C_DD1D) % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
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

    // 干扰项数量由 `option_count` 决定（含正确答案，clamp 到 2~8，需求 14）。
    // 干扰项取自**目标语言**的词库；不足时如实少给，绝不用同一个词重复填充 ——
    // 那会让用户在「同一个答案出现两次」的题上直接懵掉。
    // 最终不足 1 个干扰项时 `options` 为空，前端会走「只显示答案」的兜底路径。
    let want_options = normalize_option_count(state.cfg().study.option_count);
    let distractors_needed = want_options - 1;
    let mut options: Vec<String> = Vec::new();
    // 多取一些候选：池子里可能有空释义、与答案同义、以及正确答案自身。
    let pool_size = ((distractors_needed as i64) * 8).max(30);
    let pool = state.db.random_words(lang, pool_size).map_err(err)?;
    for p in pool {
        if options.len() >= distractors_needed {
            break;
        }
        // ★ 正确答案的词自己必须在池子里被排除：否则可能出现两个一模一样的选项。
        if p.word.eq_ignore_ascii_case(&entry.word) {
            continue;
        }
        let cand = match mode {
            QuizMode::EnToZh | QuizMode::ExToZh => p.definition_in(def_lang),
            _ => p.word.clone(),
        };
        // 空释义、与正确答案同文、以及彼此重复的干扰项都跳过（去重）。
        if cand.trim().is_empty() || cand == answer || options.contains(&cand) {
            continue;
        }
        options.push(cand);
    }
    shuffle_in_place(&mut options);

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

/// 答错的词回插到「当前位置 + 这么多题之后」（需求 14）。
///
/// 为什么是 3：紧挨着（gap=1）等于立刻重复，用户会以为程序卡了；隔得太远
/// （>`3`）这一轮往往已经结束，等于没回插。中间隔两题是体感上刚好的间隔。
const REQUEUE_GAP: usize = 3;

/// 同一个词每轮最多回插几次。
///
/// 上限 2 而不是无限：一个怎么都记不住的词如果一直回插，用户就会卡在它上面
/// 出不去（死亡循环）。回插两次仍然错，就交给下一轮 / 错词本去处理。
const MAX_REQUEUE_PER_WORD: u32 = 2;

/// 答错的词「本轮再轮到」：把它重新插回 `index + REQUEUE_GAP`。
///
/// 返回本轮是否真的回插了（没到上限、且当前题确实是这个词时才算）。
/// 抽成纯函数是为了能直接单测位置、上限与 `total` 增长，不必搭 Tauri 状态。
fn requeue_wrong(s: &mut crate::state::Session, word: &str) -> bool {
    let key = word.to_lowercase();
    let used = s.requeue_counts.get(&key).copied().unwrap_or(0);
    if used >= MAX_REQUEUE_PER_WORD {
        return false;
    }
    // 只有「当前题就是这个词」时才回插，避免把别的词错插进来
    // （前端理论上可能显式传 word，与队列当前题不一致时要小心）。
    let is_current = s
        .queue
        .get(s.index)
        .map(|e| e.word.eq_ignore_ascii_case(word))
        .unwrap_or(false);
    if !is_current {
        return false;
    }
    let Some(cur) = s.queue.get(s.index).cloned() else {
        return false;
    };
    // 剩余不足 gap 个就追加到队尾。
    let pos = (s.index + REQUEUE_GAP).min(s.queue.len());
    s.queue.insert(pos, cur);
    s.requeue_counts.insert(key, used + 1);
    true
}

/// 提交答案 —— 驱动 SRS 调度（需求 4）。
#[tauri::command]
pub fn cmd_submit_answer(
    state: State<'_, Arc<AppState>>,
    word: Option<String>,
    grade: Grade,
    elapsed_ms: Option<i64>,
    lang: Option<String>,
    // 会话类型：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽（需求 14）。
    kind: Option<String>,
) -> Result<AnswerResult, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let now = timeutil::now_ts();
    // 复习与背诵各自维护一个会话，互不干扰（需求 14）。
    let slot = state.session_slot(kind.as_deref());

    // 确定被作答的词：显式传入优先，否则用会话当前题
    let word = match word {
        Some(w) if !w.trim().is_empty() => w,
        _ => {
            let s = slot.read();
            match s.current() {
                Some(e) => e.word.clone(),
                None => return Err("当前没有进行中的题目".into()),
            }
        }
    };

    let mode = slot.read().mode.unwrap_or_default();
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

    // 更新会话计数并推进。
    //
    // ★ 答错的词本轮还要再轮到（需求 14）：把它重新插回 `index + gap`，
    //   而不是只把 `due_at` 提前 —— 后者在本轮根本不会再出现，前端那句
    //   「稍后会再考你一次」就成了误导。回插后 `total` 跟着增长，
    //   `index` 仍然「答一题进一格」（不回退进度条）。
    let (finished, total, correct, wrong, requeued) = {
        let mut s = slot.write();
        if grade.is_correct() {
            s.correct += 1;
        } else {
            s.wrong += 1;
        }

        // 答错的词本轮还要再轮到（需求 14）：见 `requeue_wrong` 的说明。
        let requeued = !grade.is_correct() && requeue_wrong(&mut s, &word);

        s.index += 1;
        (s.index >= s.queue.len(), s.total(), s.correct, s.wrong, requeued)
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
        // 本轮是否真的把该词回插了（前端据此显示「稍后会再考你一次」的准确文案）
        requeued,
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
pub fn cmd_skip(
    state: State<'_, Arc<AppState>>,
    // 会话类型：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽（需求 14）。
    kind: Option<String>,
) -> Result<SessionInfo, String> {
    let mut s = state.session_slot(kind.as_deref()).write();
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
pub fn cmd_end_session(
    state: State<'_, Arc<AppState>>,
    // 会话类型：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽（需求 14）。
    kind: Option<String>,
) -> Result<SessionInfo, String> {
    let slot = state.session_slot(kind.as_deref());
    let info = {
        let s = slot.read();
        SessionInfo {
            mode: s.mode.unwrap_or_default(),
            total: s.total(),
            index: s.index,
            correct: s.correct,
            wrong: s.wrong,
            leech_only: s.leech_only,
        }
    };
    // 只清空被指定的槽位：结束复习不该把背诵会话一起清掉。
    *slot.write() = Default::default();
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
    /// 本轮是否真的把答错的词回插（需求 14）；新增字段，老前端忽略即可。
    pub requeued: bool,
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

// ============================================================
// 六之二、学习目标（需求 15：设目标 + 完成后庆祝 + 加量提示）
// ============================================================

/// 目标模式是否合法。
fn valid_goal_mode(m: &str) -> bool {
    matches!(m, "off" | "days" | "per_day")
}

/// "YYYY-MM-DD" 两个日期相差几天（`today` 晚于 `date` 时为正）。解析失败按 0。
fn days_since(date: &str, today: &str) -> i64 {
    let p = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    match (p(date), p(today)) {
        (Some(a), Some(b)) => (b - a).num_days(),
        _ => 0,
    }
}

/// 计划里**每天要吃掉多少新词**（不含当天到期的复习量）。
///
/// ★ 为什么把它单独抽出来：`eta_date` 的分母必须是这个数，而不是 `today_done`。
///   用「今天已背了多少」当分母，第一天只背了几个词就会外推出几百天 ——
///   实测用户库：剩余 5056 词、当天只背了 7 个，算出 5056/7 ≈ 723 天，
///   界面于是写「预计完成 2028-09-29」，而用户设的目标明明白白是「30 天背完」。
///   两个数字当众打架，和「按钮写着 1 个词、点进去说一个都没有」是同一类病。
///
///   今天背多背少只说明「今天达没达标」，不足以推翻整个计划；拿单日样本去做
///   除法，开背第一天必然给出一个荒唐的年份。计划的速率才是稳定的基准。
fn planned_new_per_day(mode: &str, remaining: u32, days_left: u32, per_day: u32) -> u32 {
    match mode {
        "days" => {
            let dl = days_left.max(1) as f64;
            ((remaining as f64) / dl).ceil() as u32
        }
        "per_day" => per_day,
        _ => 0,
    }
}

/// 按给定速率背完 `remaining` 个词还需要的天数（纯函数，便于单测）。
fn eta_days_needed(remaining: u32, rate: u32) -> i64 {
    if remaining == 0 || rate == 0 {
        return 0;
    }
    ((remaining as f64) / (rate as f64)).ceil() as i64
}

/// 今天应完成多少（纯函数，公式见注释）。
///
/// - `days`：`ceil(remaining / max(1, days_left)) + due_today`
/// - `per_day`：`per_day + due_today`
/// - `off`：`0`
/// - `finished`（没有新词了）：只保留 `due_today`，忽略加量。
/// - 未完成时叠加当天的临时加量 `extra`。
fn today_target_for(
    mode: &str,
    remaining: u32,
    days_left: u32,
    per_day: u32,
    due_today: u32,
    finished: bool,
    extra: u32,
) -> u32 {
    if finished {
        return due_today;
    }
    // 与 `eta_date` 共用同一个速率函数：今日目标与预计完成日必须同源，
    // 否则两边口径一分叉，界面又会出现「目标是 30 天、完成日却是两年后」。
    let base = match mode {
        // days_left==0（已到/已超期）时用 max(1) 兜底，避免除零。
        "days" | "per_day" => {
            planned_new_per_day(mode, remaining, days_left, per_day).saturating_add(due_today)
        }
        _ => 0,
    };
    base.saturating_add(extra)
}

/// 是否该提示「轮次复习压力会升高」。
///
/// 两条任一命中即 high：
///   ① 今天目标量 > 日均预期词量（`goal_per_day`，默认 30）的 **3 倍**；
///   ② 今天到期量 > 未来 7 天**日均**复习量的 **3 倍**。
/// 取 3 倍而不是 1.5 倍：1.5 倍在正常波动下几乎天天触发，提示会变成噪音；
/// 3 倍才是「这一天会明显难受」的量级。
fn pressure_is_high(
    today_target: u32,
    per_day_basis: u32,
    due_today: u32,
    review_load_7d: u32,
) -> bool {
    let daily_basis = per_day_basis.max(1) as f64;
    let avg_due = review_load_7d as f64 / 7.0;
    (today_target as f64) > daily_basis * 3.0 || (avg_due >= 1.0 && (due_today as f64) > 3.0 * avg_due)
}

/// 计算学习目标视图（`cmd_study_goal` / `cmd_set_study_goal` / `cmd_extra_study` 共用）。
///
/// `today_target` 的公式（用户原话「根据我们的公式计算同步复习之前的单词」）：
/// - `days`    ：`ceil(remaining / max(1, days_left)) + due_today`
///   —— 每天要背的新词，**加上**当天到期的复习量。背新词必然带来同步复习，
///      用户要的就是这本总账；只报新词数会让目标显得轻松、实际做不完。
/// - `per_day` ：`per_day + due_today`
/// - `off`     ：`0`
/// - 已全部学完（`finished`）时只保留 `due_today`（没有新词可背了）。
/// - 下限 0；未完成时再叠加「多背一点」的临时加量（仅当天有效）。
fn compute_study_goal(
    state: &AppState,
    book_id: Option<String>,
    lang: Option<String>,
) -> Result<StudyGoal, String> {
    let cfg = state.cfg();
    let study = cfg.study.clone();
    let now = timeutil::now_ts();
    let today = srs::day_key(now);
    let today_date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d").ok();

    // 目标词库：参数优先，其次配置里的 `goal_book_id`；空串 = 全部词库。
    let book_id = match book_id {
        Some(b) if !b.trim().is_empty() => b.trim().to_string(),
        _ => study.goal_book_id.trim().to_string(),
    };

    // 有效语言：优先按**词库自身**的语言（与 `cmd_words_in_book` 口径一致），
    // 否则用传入语言 / 当前学习语言。
    let (eff_lang, book_name) = if book_id.is_empty() {
        (
            lang.unwrap_or_else(|| cfg.target_lang.clone()),
            "全部词库".to_string(),
        )
    } else {
        let wb = state.db.get_wordbook(&book_id).map_err(err)?;
        let l = wb
            .as_ref()
            .map(|b| b.lang.clone())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| lang.unwrap_or_else(|| cfg.target_lang.clone()));
        let n = wb.map(|b| b.name).unwrap_or_else(|| book_id.clone());
        (l, n)
    };

    let (total, learned) = if book_id.is_empty() {
        (
            state.db.word_count(&eff_lang).map_err(err)?.max(0) as u32,
            state.db.learned_count(&eff_lang).map_err(err)?.max(0) as u32,
        )
    } else {
        let (t, l, _) = state
            .db
            .book_progress(&book_id, &eff_lang, now)
            .map_err(err)?;
        (t.max(0) as u32, l.max(0) as u32)
    };
    let remaining = total.saturating_sub(learned);
    // total==0 的「空词库」不算「已完成」，否则会误报庆祝。
    let finished = total > 0 && remaining == 0;

    let due_today = state
        .db
        .due_states(&eff_lang, now, 10_000)
        .map_err(err)?
        .len() as u32;
    // 按**词**去重：同一个词今天答 5 次只算 1 个（用户说的是「背完多少个词」）。
    let today_done = state
        .db
        .today_reviewed_words(Some(&eff_lang), now)
        .map_err(err)?
        .max(0) as u32;

    // 未来 7 天预计复习总量（含今天的逾期）。
    let review_load_7d = {
        let states = state.db.all_states(&eff_lang).map_err(err)?;
        let plan = build_plan(&states, 7, now);
        let mut sum: i64 = plan.iter().map(|d| d.count).sum();
        if let Some(d0) = plan.first() {
            sum += d0.overdue;
        }
        sum.max(0) as u32
    };

    // 距目标完成日还剩几天（`days` 模式才有意义）。
    let days_left = if study.goal_mode == "days" {
        let elapsed = if study.goal_started_at.is_empty() {
            0
        } else {
            days_since(&study.goal_started_at, &today).max(0)
        };
        (study.goal_days as i64 - elapsed).max(0) as u32
    } else {
        0
    };

    // 今天「多背一点」的临时加量（只有日期对上才算数，跨天自动失效）。
    let extra = if study.goal_extra_date == today {
        study.goal_extra_today
    } else {
        0
    };

    let today_target = today_target_for(
        &study.goal_mode,
        remaining,
        days_left,
        study.goal_per_day,
        due_today,
        finished,
        extra,
    );

    let today_remaining = today_target.saturating_sub(today_done);

    let pressure = if pressure_is_high(today_target, study.goal_per_day, due_today, review_load_7d)
    {
        "high"
    } else {
        "normal"
    }
    .to_string();

    // 预计完成日：按**计划的速率**外推，不是按今天的瞬时节奏。
    //
    // ★ 改这里的原因（用户实测反馈：设了「30 天背完」，界面却写「2028 年」）：
    //   旧公式是 `remaining / today_done` —— 拿「今天已背了几个」当整本书的速率。
    //   今天才背 7 个就外推出 723 天，而单日样本的噪声极大（刚打开软件、只背了
    //   两三分钟都会触发），第一天必然得出荒唐的年份。
    //   今天背多背少只影响「今天达没达标」，不该把完成日拖到两年后。
    //
    //   没有目标（`off`）时**不编造**：既然没说要多久背完，就不存在「预计完成日」
    //   这个基准，返回空串让前端整条不显示（与调试模式假数据同一口径）。
    let eta_date = if remaining == 0 {
        today.clone()
    } else {
        let rate = planned_new_per_day(
            &study.goal_mode,
            remaining,
            days_left,
            study.goal_per_day,
        );
        match today_date {
            Some(d) if rate > 0 => (d + chrono::Duration::days(eta_days_needed(remaining, rate)))
                .format("%Y-%m-%d")
                .to_string(),
            _ => String::new(),
        }
    };

    let on_track = study.goal_mode == "off" || finished || today_done >= today_target;

    Ok(StudyGoal {
        mode: study.goal_mode.clone(),
        // per_day 模式下 days 报 0，days 模式下 per_day 报 0 —— 前端据此渲染。
        days: if study.goal_mode == "days" { study.goal_days } else { 0 },
        per_day: if study.goal_mode == "per_day" { study.goal_per_day } else { 0 },
        book_id: book_id.clone(),
        book_name,
        total_words: total,
        learned,
        remaining,
        today_target,
        today_done,
        today_remaining,
        eta_date,
        days_left,
        due_today,
        review_load_7d,
        pressure,
        on_track,
        finished,
    })
}

/// 读取当前学习目标（前端据此渲染进度环与庆祝弹窗）。
#[tauri::command]
pub fn cmd_study_goal(
    state: State<'_, Arc<AppState>>,
    book_id: Option<String>,
    lang: Option<String>,
) -> Result<StudyGoal, String> {
    compute_study_goal(&state, book_id, lang)
}

/// 设置学习目标（保存后立刻回传最新 goal，前端一次往返就够）。
#[tauri::command]
pub fn cmd_set_study_goal(
    state: State<'_, Arc<AppState>>,
    mode: String,
    days: Option<u32>,
    per_day: Option<u32>,
    book_id: Option<String>,
) -> Result<StudyGoal, String> {
    let mode = mode.trim().to_string();
    if !valid_goal_mode(&mode) {
        return Err(format!("目标模式「{mode}」不认识，只能是 off / days / per_day"));
    }
    // 无论当前模式用不用得到，显式传入的值都要校验，避免脏数据被静默保存。
    if let Some(d) = days {
        if !(1..=3650).contains(&d) {
            return Err("目标天数必须在 1~3650 天之间".to_string());
        }
    }
    if let Some(p) = per_day {
        if !(1..=1000).contains(&p) {
            return Err("每天词量必须在 1~1000 之间".to_string());
        }
    }
    // 当前模式必需的值缺省时，用配置里的现值兜底并同样校验。
    if mode == "days" {
        let d = days.unwrap_or_else(|| state.cfg().study.goal_days);
        if !(1..=3650).contains(&d) {
            return Err("目标天数必须在 1~3650 天之间".to_string());
        }
    }
    if mode == "per_day" {
        let p = per_day.unwrap_or_else(|| state.cfg().study.goal_per_day);
        if !(1..=1000).contains(&p) {
            return Err("每天词量必须在 1~1000 之间".to_string());
        }
    }

    let today = srs::day_key(timeutil::now_ts());
    state
        .update_config(|c| {
            let was_off = c.study.goal_mode == "off";
            c.study.goal_mode = mode.clone();
            if let Some(d) = days {
                c.study.goal_days = d;
            }
            if let Some(p) = per_day {
                c.study.goal_per_day = p;
            }
            if let Some(b) = book_id.clone() {
                c.study.goal_book_id = b;
            }
            // 只在**首次**从 off 切到非 off 时写下开始日期：
            // 之后每次改模式都重置的话，「已经过去几天」会被清零，
            // 目标永远从今天重新开始，用户永远完不成。
            if was_off && mode != "off" {
                c.study.goal_started_at = today.clone();
            }
            // 关掉目标时把当天的临时加量一并清掉。
            if mode == "off" {
                c.study.goal_extra_today = 0;
                c.study.goal_extra_date = String::new();
            }
        })
        .map_err(err)?;

    compute_study_goal(&state, None, None)
}

/// 「多背一点」：临时把今天的目标抬高 `extra` 个词（仅当天有效）。
///
/// 被多背的词**必须进入复习轮**：这一点由 `cmd_submit_answer` → `srs::schedule`
/// 保证（答完就会写入未来的 `due_at`），返回的 `pressure` 供前端提示
/// 「加量会抬高轮次复习压力」。
#[tauri::command]
pub fn cmd_extra_study(
    state: State<'_, Arc<AppState>>,
    extra: u32,
    book_id: Option<String>,
    lang: Option<String>,
) -> Result<StudyGoal, String> {
    if !(1..=500).contains(&extra) {
        return Err("加量必须是 1~500 之间的数字".to_string());
    }
    let today = srs::day_key(timeutil::now_ts());
    state
        .update_config(|c| {
            // 同一天多次点「多背一点」应累加；跨天则从这笔重新开始。
            if c.study.goal_extra_date == today {
                c.study.goal_extra_today = c.study.goal_extra_today.saturating_add(extra);
            } else {
                c.study.goal_extra_today = extra;
                c.study.goal_extra_date = today.clone();
            }
        })
        .map_err(err)?;
    compute_study_goal(&state, book_id, lang)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Session;

    /// 建一个独立的临时 AppState（连内存/临时目录，不碰用户数据）。
    fn state(tag: &str) -> Arc<AppState> {
        let p = std::env::temp_dir().join(format!("wordwise-cmd-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        AppState::new(p).expect("建测试状态")
    }

    fn session_of(words: &[&str]) -> Session {
        Session {
            queue: words.iter().map(|w| WordEntry::new(*w)).collect(),
            ..Default::default()
        }
    }

    /* ---------------- 需求 5：AI 联网补充的触发条件 ---------------- */

    fn entry_with(senses: usize, examples: usize, inflections: usize, related: usize) -> WordEntry {
        let mut e = WordEntry::new("happy");
        for i in 0..senses {
            let mut s = Sense {
                pos: "adj.".into(),
                definition: format!("释义{}", i),
                ..Default::default()
            };
            for j in 0..examples {
                s.examples.push(Example {
                    text: format!("ex{}", j),
                    translation: String::new(),
                });
            }
            e.senses.push(s);
        }
        for i in 0..inflections {
            e.inflections.push(Inflection {
                label: "比较级".into(),
                form: format!("form{}", i),
            });
        }
        for i in 0..related {
            e.related.push(format!("rel{}", i));
        }
        e
    }

    #[test]
    fn web_refs_only_when_local_data_thin() {
        // 一条释义都没有（本地查不到）→ 联网
        assert!(explain_needs_web(&entry_with(0, 0, 0, 0)));
        // 有释义但整条词条一个例句都没有 → 联网（这是最常见的「查不到例句」）
        assert!(explain_needs_web(&entry_with(2, 0, 3, 3)));
        // 有释义有例句，但既无变形也无相关词 → 联网补「派生与词汇网络」
        assert!(explain_needs_web(&entry_with(2, 1, 0, 0)));
    }

    #[test]
    fn web_refs_skipped_when_local_data_rich() {
        // 释义 + 例句 + 变形俱全：本地数据够用，不该为它多等一次搜索
        assert!(!explain_needs_web(&entry_with(2, 1, 3, 0)));
        // 只有相关词、没有变形，也算够用（相关词已能撑起词汇网络那一段）
        assert!(!explain_needs_web(&entry_with(2, 2, 0, 5)));
    }

    #[test]
    fn web_refs_disabled_by_config() {
        // 关掉「在线搜索」的用户不该在讲解时偷偷联网 —— 这条约束是
        // 用户可感知的隐私/流量边界，必须在取资料前就短路。
        let st = state("web-refs-off");
        let mut cfg = st.cfg();
        cfg.web_search_enabled = true;
        assert!(explain_web_allowed(&cfg, "happy"));
        // 空词条名没法搜，同样不发请求
        assert!(!explain_web_allowed(&cfg, "   "));

        cfg.web_search_enabled = false;
        assert!(!explain_web_allowed(&cfg, "happy"));
    }

    /* ---------------- 需求 15：today_target 公式 ---------------- */

    #[test]
    fn today_target_days_mode() {
        // ceil(100/10)=10 加今日复习 5 = 15
        assert_eq!(today_target_for("days", 100, 10, 30, 5, false, 0), 15);
        // days_left=0（已到/已超期）边界：剩余的全部算今天，兜底除零
        assert_eq!(today_target_for("days", 100, 0, 30, 5, false, 0), 105);
        // remaining=0 但非 finished（空词库）→ 0 + due
        assert_eq!(today_target_for("days", 0, 10, 30, 5, false, 0), 5);
        // finished（有词且全学完）→ 只保留 due，忽略加量
        assert_eq!(today_target_for("days", 0, 10, 30, 5, true, 7), 5);
        // 未完成时叠加「多背一点」的临时加量
        assert_eq!(today_target_for("days", 100, 10, 30, 5, false, 7), 22);
    }

    #[test]
    fn today_target_per_day_and_off() {
        assert_eq!(today_target_for("per_day", 500, 0, 20, 3, false, 0), 23);
        assert_eq!(today_target_for("per_day", 0, 0, 20, 3, true, 9), 3);
        assert_eq!(today_target_for("off", 500, 10, 20, 3, false, 0), 0);
        // off + 加量：仍然按加量给出目标（用户就是想多背）
        assert_eq!(today_target_for("off", 500, 10, 20, 3, false, 4), 4);
        // 下限 0
        assert_eq!(today_target_for("off", 0, 0, 0, 0, false, 0), 0);
    }

    /* ---------------- 预计完成日（eta）：分母必须是计划速率 ---------------- */

    #[test]
    fn planned_rate_is_the_eta_basis_not_todays_sample() {
        // 用户的真实场景：整本书剩 5056 词、目标「30 天背完」。
        // 计划速率 = ceil(5056/30) = 169 词/天。
        assert_eq!(planned_new_per_day("days", 5056, 30, 30), 169);
        // 按计划速率外推：ceil(5056/169) = 30 天 —— 与「30 天目标」自洽。
        assert_eq!(eta_days_needed(5056, 169), 30);

        // ★ 反面用例（本 bug 的原型）：旧公式用「今天已背了多少」当分母，
        //   今天只背 7 个就外推出 723 天 → 界面显示「2028 年」。
        //   现在无论 today_done 是几，都不参与 eta 的计算。
        assert_eq!(eta_days_needed(5056, 7), 723); // 这个数本身没错，错在拿它当 eta
        assert!(eta_days_needed(5056, 169) <= 30); // 正确口径下不会超出目标天数

        // per_day 模式：速率就是用户设的每日量
        assert_eq!(planned_new_per_day("per_day", 5056, 0, 30), 30);
        assert_eq!(eta_days_needed(5056, 30), 169);
    }

    #[test]
    fn eta_never_invents_a_date_without_a_goal() {
        // off 模式没有「多久背完」这个基准 → 速率 0 → 不编造完成日
        assert_eq!(planned_new_per_day("off", 5056, 30, 30), 0);
        assert_eq!(eta_days_needed(5056, 0), 0);
        // 速率为 0 时，无论剩多少都不该产出日期
        assert_eq!(eta_days_needed(1, 0), 0);
        // 已经背完 → 0 天（今天就算完成）
        assert_eq!(eta_days_needed(0, 169), 0);
        // days_left=0（已到/已超期）：速率兜底为 remaining，1 天背完
        assert_eq!(planned_new_per_day("days", 100, 0, 30), 100);
        assert_eq!(eta_days_needed(100, 100), 1);
    }

    #[test]
    fn today_target_and_eta_share_one_rate_function() {
        // 同一组入参下，今日目标里的「新词摊派」必须等于 planned_new_per_day，
        // 否则今日目标说 169、完成日却按别的速率算，两边又会打架。
        let planned = planned_new_per_day("days", 100, 10, 30);
        assert_eq!(planned, 10);
        assert_eq!(today_target_for("days", 100, 10, 30, 5, false, 0), planned + 5);
        assert_eq!(today_target_for("per_day", 500, 0, 20, 3, false, 0), 23);
    }

    /* ---------------- 需求 15：pressure 阈值 ---------------- */

    #[test]
    fn pressure_threshold_judgement() {
        // 31 远低于 30*3 → normal
        assert!(!pressure_is_high(31, 30, 0, 0));
        // 恰好等于 3 倍不算超，越过才算
        assert!(!pressure_is_high(90, 30, 0, 0), "等于 3 倍不应判 high");
        assert!(pressure_is_high(91, 30, 0, 0));
        // 1.5 倍绝不触发（否则提示会变成噪音）
        assert!(!pressure_is_high(45, 30, 0, 0));
        // 复习端：7 天日均 10，今天到期 31 > 30 → high；30 不触发
        assert!(pressure_is_high(0, 30, 31, 70));
        assert!(!pressure_is_high(0, 30, 30, 70));
        // 没有历史复习量（日均 < 1）时不因今天到期而误报
        assert!(!pressure_is_high(0, 30, 5, 0));
    }

    /* ---------------- 需求 14：选项个数 clamp ---------------- */

    #[test]
    fn option_count_clamped_to_2_8() {
        assert_eq!(normalize_option_count(0), 2);
        assert_eq!(normalize_option_count(1), 2);
        assert_eq!(normalize_option_count(2), 2);
        assert_eq!(normalize_option_count(4), 4);
        assert_eq!(normalize_option_count(8), 8);
        assert_eq!(normalize_option_count(9), 8);
        assert_eq!(normalize_option_count(1000), 8);
    }

    /* ---------------- 需求 14：错题回插 ---------------- */

    #[test]
    fn wrong_requeue_inserts_at_gap_and_grows_total() {
        let mut s = session_of(&["a", "b", "c", "d", "e"]);
        s.index = 1; // 当前是 b
        let total_before = s.total();
        assert!(requeue_wrong(&mut s, "b"));
        assert_eq!(s.total(), total_before + 1, "total 必须跟着增长");
        assert_eq!(s.index, 1, "回插本身不推进 index（推进由调用方 +1）");
        assert_eq!(s.queue[1].word, "b", "当前题位置不变");
        assert_eq!(s.queue[1 + REQUEUE_GAP].word, "b", "回插到 index+gap");
        // 键按小写归一
        assert_eq!(s.requeue_counts.get("b"), Some(&1));
    }

    #[test]
    fn wrong_requeue_appends_to_tail_when_near_end() {
        let mut s = session_of(&["a", "b"]);
        s.index = 1;
        assert!(requeue_wrong(&mut s, "b"));
        assert_eq!(s.queue.len(), 3);
        assert_eq!(s.queue[2].word, "b", "剩余不足 gap 个时追加到队尾");
    }

    #[test]
    fn wrong_requeue_capped_at_two_per_word() {
        let mut s = session_of(&["a", "b", "c", "d", "e"]);
        s.index = 1; // b
        assert!(requeue_wrong(&mut s, "b"));
        // 第二次：当前题挪到刚才插入的位置
        s.index = 4;
        assert!(requeue_wrong(&mut s, "b"));
        assert_eq!(s.requeue_counts.get("b"), Some(&2));
        // 第三次：到达上限，不再回插
        s.index = s.queue.iter().position(|e| e.word == "b").unwrap();
        let len = s.queue.len();
        assert!(!requeue_wrong(&mut s, "b"), "每个词每轮最多回插 2 次");
        assert_eq!(s.queue.len(), len);
    }

    #[test]
    fn wrong_requeue_ignores_non_current_word() {
        let mut s = session_of(&["a", "b", "c"]);
        s.index = 0; // 当前是 a
        assert!(!requeue_wrong(&mut s, "zzz"));
        assert!(!requeue_wrong(&mut s, "c"));
        assert_eq!(s.total(), 3);
        assert!(s.requeue_counts.is_empty());
    }

    /* ---------------- 需求 14：会话槽位分离 ---------------- */

    #[test]
    fn review_session_does_not_clobber_study_session() {
        let st = state("slots");
        // 造一个进行中的背诵会话
        {
            let mut s = st.session_slot(None).write();
            s.queue = vec![WordEntry::new("study1")];
            s.index = 0;
            s.def_lang = "zh".into();
        }
        // 开复习（空库 → total 0，但不报错）
        let info = start_review_session_inner(&st, None, None, Some("en".into()), None).unwrap();
        assert_eq!(info.total, 0, "今天没有要复习的词时返回 total=0，而不是报错");
        // 背诵槽原封不动
        let s = st.session_slot(None).read();
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.current().unwrap().word, "study1");
        // 复习是另一个槽位
        assert!(st.session_slot(Some("review")).read().queue.is_empty());
    }

    #[test]
    fn unknown_kind_falls_back_to_study_slot() {
        let st = state("kind-fallback");
        assert!(std::ptr::eq(
            st.session_slot(Some("nonsense")),
            st.session_slot(Some("study")),
        ));
        assert!(!std::ptr::eq(
            st.session_slot(Some("review")),
            st.session_slot(None),
        ));
    }

    /* ---------------- 需求 14：复习队列不补足 ---------------- */

    #[test]
    fn review_queue_is_not_padded_to_batch_size() {
        let st = state("review-nofill");
        st.update_config(|c| c.study.batch_size = 20).unwrap();
        let now = timeutil::now_ts();
        let entries = vec![
            WordEntry::new("aa"),
            WordEntry::new("bb"),
            WordEntry::new("cc"),
        ];
        st.db.bulk_upsert_words(&entries, now).unwrap();
        // 只有两个词到期
        st.db.upsert_state(&StudyState::new("aa", "en", now)).unwrap();
        st.db.upsert_state(&StudyState::new("bb", "en", now)).unwrap();

        let q = build_review_queue(&st, "en", None).unwrap();
        assert_eq!(q.len(), 2, "到期的有几个就是几个，绝不补足到 batch_size");
        // size 只用于截断上限
        assert_eq!(build_review_queue(&st, "en", Some(1)).unwrap().len(), 1);
        // 没有到期词 → 空队列（不报错）
        assert!(build_review_queue(&st, "ja", None).unwrap().is_empty());
    }

    /// 复习会话必须**采用调用方传来的模式**，不能悄悄沿用背诵槽的模式。
    ///
    /// 这条是回归测试：原来的实现从 `state.session.mode` 读模式，
    /// 于是「界面切到拼写、复习却出看英选中」，题面与用户手上的模式对不上，
    /// 答完还会按错的模式判分 —— 现象是「我明明在拼写，它却让我选释义」。
    #[test]
    fn review_session_honours_caller_mode() {
        let st = state("review-mode");
        let now = timeutil::now_ts();
        st.db
            .bulk_upsert_words(&[WordEntry::new("aa")], now)
            .unwrap();
        st.db.upsert_state(&StudyState::new("aa", "en", now)).unwrap();

        // 背诵槽里放一个别的模式，用来证明复习**没有**读它
        st.session_slot(None).write().mode = Some(QuizMode::Spelling);

        let info = start_review_session_inner(
            &st,
            Some(QuizMode::ZhToEn),
            None,
            Some("en".into()),
            Some("zh".into()),
        )
        .unwrap();
        assert_eq!(info.total, 1, "一个到期词就出一题");
        assert_eq!(info.mode, QuizMode::ZhToEn, "复习用的是调用方传的模式");
        let s = st.session_slot(Some("review")).read();
        assert_eq!(s.mode, Some(QuizMode::ZhToEn));
        assert_eq!(s.def_lang, "zh", "释义语言也要写进复习槽");
        // 背诵槽的模式一个字没动
        assert_eq!(st.session_slot(None).read().mode, Some(QuizMode::Spelling));
    }
}
