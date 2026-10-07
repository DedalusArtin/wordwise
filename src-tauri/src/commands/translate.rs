//! 翻译相关命令（需求 1-5）。
//!
//! 三级翻译链路（见 [`crate::translate`] 的模块说明）：
//! 本地缓存 → 有道在线 → 本地大模型兜底。
//!
//! 另外提供「AI 增强」：AI 释义与语境说明、多版本译文、自动润色、例句生成。
//! 这些是**解释性内容**，语言跟随 [`crate::models::AppConfig::explain_lang`]
//! （默认中文），而不是目标语言 —— 用户学日语时，解释仍该用母语说。

use crate::state::AppState;
use crate::timeutil;
use crate::translate::{self, TranslateResult};
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 翻译缓存有效期：30 天。译文的时效性远低于词义，可以存久一点。
const CACHE_TTL: i64 = 30 * 24 * 3600;
/// 单次翻译允许的最大长度。有道接口对超长文本不友好，也防误贴整本书。
const MAX_LEN: usize = 5000;

/// 组装缓存 key。
fn cache_key(from: &str, to: &str, text: &str) -> String {
    format!("{from}|{to}|{text}")
}

/// 翻译一段文本（需求 2 / 3 / 5）。
///
/// `from` 传 [`translate::AUTO`] 或留空表示自动检测；`to` 留空用配置里的目标语言。
/// `force` 为真时跳过缓存（用户点「重新翻译」）。
#[tauri::command]
pub async fn cmd_translate(
    state: State<'_, Arc<AppState>>,
    text: String,
    from: Option<String>,
    to: Option<String>,
    force: Option<bool>,
) -> Result<TranslateResult, String> {
    let cfg = state.cfg();

    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(TranslateResult::default());
    }
    if text.chars().count() > MAX_LEN {
        return Err(format!("一次最多翻译 {} 个字符", MAX_LEN));
    }

    let from = from
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let s = cfg.source_lang.trim();
            if s.is_empty() {
                translate::AUTO.to_string()
            } else {
                s.to_string()
            }
        });
    let to = to
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.target_lang.clone());

    // 源语言与目标语言相同 → 不必翻译（用户误把方向选成一样）
    if from == to {
        return Ok(TranslateResult {
            source: text.clone(),
            text: text.clone(),
            from: from.clone(),
            to: to.clone(),
            engine: "none".into(),
            note: Some("源语言与目标语言相同，没有可翻译的内容".into()),
            ..Default::default()
        });
    }

    translate_core(&state, &text, &from, &to, force.unwrap_or(false)).await
}

/// 翻译正文（不含 tauri 命令外壳）。
///
/// 抽出来的目的是给「双向词条」（`cmd_lookup_pairs`）复用同一条三级链路
/// 与同一份缓存 —— 否则查「乌鸦」时，界面顶部 Already 已经在调用
/// `cmd_translate` 拿译文，另一侧又各自发一次请求，同一个限频很严的接口
/// 会被白白打两次，更快撞上限流。
///
/// 调用方必须**先确认 `from != to`**，本函数不再重复判断。
pub async fn translate_core(
    state: &Arc<AppState>,
    text: &str,
    from: &str,
    to: &str,
    force: bool,
) -> Result<TranslateResult, String> {
    let text = text.trim().to_string();
    let cfg = state.cfg();
    let now = timeutil::now_ts();
    let key = cache_key(from, to, &text);

    // ---- 1) 本地缓存 ----
    if !force {
        if let Ok(Some(json)) = state.db.get_trans_cache(&key, CACHE_TTL, now) {
            if let Ok(mut cached) = serde_json::from_str::<TranslateResult>(&json) {
                cached.from_cache = true;
                cached.engine = "cache".into();
                cached.note = None;
                // 缓存命中也要回填历史 id：收藏按钮要靠它
                cached.record_id = state
                    .db
                    .upsert_translation(
                        &cached.from,
                        &cached.to,
                        &cached.source,
                        &cached.text,
                        "youdao",
                        now,
                    )
                    .ok();
                return Ok(cached);
            }
        }
    }

    // ---- 2) 有道在线 ----
    //
    // 注意：只发一次请求。失败原因要在这一次里拿到，
    // 绝不能「为了取错误信息再请求一遍」—— 那个接口本来就限频，
    // 重复请求只会更快撞上 411。
    let online = translate::youdao_translate(&state.http(), &cfg.network, &from, &to, &text).await;
    let online_fail = match &online {
        Ok(r) if !r.is_empty() => None,
        Ok(_) => Some("在线翻译未返回内容".to_string()),
        Err(e) => Some(e.to_string()),
    };

    let mut result = match online_fail {
        None => online.expect("已确认成功"),
        // ---- 3) 本地大模型兜底 ----
        Some(why) => {
            let mut r = translate::llm_translate(&state.http(), &cfg.llm, &from, &to, &text)
                .await
                .map_err(|e| format!("{}；本地模型兜底也失败：{}", why, e))?;
            r.note = Some(format!("{}，已改用本地模型翻译", why));
            r
        }
    };

    if result.text.trim().is_empty() {
        return Err("翻译结果为空，请检查输入内容或稍后重试".into());
    }

    // 落缓存（只缓存在线结果：LLM 兜底结果可能是限频期间的临时产物）
    if result.engine == "youdao" {
        if let Ok(json) = serde_json::to_string(&result) {
            let _ = state.db.put_trans_cache(&key, &json, now);
        }
    }

    // 写历史（重复翻译只更新时间），并把 id 带回前端供「收藏」使用
    match state.db.upsert_translation(
        &result.from,
        &result.to,
        &result.source,
        &result.text,
        &result.engine,
        now,
    ) {
        Ok(id) => result.record_id = Some(id),
        Err(e) => eprintln!("写入翻译历史失败：{}", e),
    }

    Ok(result)
}

/* ---------------- AI 增强（需求 5） ---------------- */

/// 组装 AI 增强的任务提示词。
///
/// `explain_lang` 是「说明性文字」要用的语言（用户母语），
/// 与译文的目标语言是两件事。
fn ai_task_prompt(
    action: &str,
    source: &str,
    target: &str,
    src_text: &str,
    dst_text: &str,
    explain_lang: &str,
) -> String {
    let head = format!(
        "原文（{from}）：\n{src}\n\n译文（{to}）：\n{dst}\n\n",
        from = translate::lang_self_name(source),
        to = translate::lang_self_name(target),
        src = src_text,
        dst = dst_text
    );
    let task = match action {
        // AI 释义与语境说明
        "explain" => {
            "请完成两件事：\n\
             1. 【含义与语境】说明原文的真实含义、使用场景、语气（正式/口语/俚语）、\
                是否有歧义或文化特定含义。\n\
             2. 【译文评价】判断上面的译文是否准确、地道；不地道就指出问题并给出更自然的说法。\n\
             用 Markdown 二级标题分段，简洁，不要客套话。"
        }
        // 多版本译文
        "variants" => {
            "请给出 3 个不同风格的译文版本，用 Markdown 表格呈现，列为：\
             风格 | 译文 | 适用场景。\n\
             三个版本分别是：① 直译（贴近原文结构）② 口语自然 ③ 书面/正式。\n\
             只给表格，不要额外说明。"
        }
        // 自动润色
        "polish" => {
            "请把上面的译文润色得更地道、更自然，意思保持不变。\n\
             输出格式：\n\
             ## 润色结果\n（润色后的译文）\n\
             ## 改了什么\n（最多 3 条，每条一行）"
        }
        // 例句生成
        "examples" => {
            "请用**目标语言**造 3 个例句，尽量覆盖上面的用法，句中不要出现翻译腔。\n\
             输出格式：Markdown 无序列表，每条为「例句 —— 该句的译文」。\n\
             例句要贴近日常使用场景。"
        }
        _ => "请针对上面的原文与译文给出简明的语言学习提示。",
    };
    let _ = explain_lang; // 说明语言由 system 里的《输出语言》约束负责
    format!("{}{}", head, task)
}

/// AI 增强：在一个已完成的翻译基础上做进一步讲解 / 润色 / 变体 / 例句。
///
/// 说明性文字的语言跟随 `explain_lang`（默认中文），与译文语言解耦。
#[tauri::command]
pub async fn cmd_translate_ai(
    state: State<'_, Arc<AppState>>,
    action: String,
    text: String,
    translated: Option<String>,
    source_lang: Option<String>,
    target_lang: Option<String>,
) -> Result<String, String> {
    let cfg = state.cfg();
    if !cfg.study.ai_explain {
        return Err("AI 讲解已在设置中关闭".into());
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("请先输入要翻译的内容".into());
    }

    let from = source_lang
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let s = cfg.source_lang.trim();
            if s.is_empty() || s == translate::AUTO {
                translate::lang_name(&translate::guess(&text))
            } else {
                translate::lang_name(&s)
            }
        });
    let to = target_lang
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.target_lang.clone());

    let user = ai_task_prompt(
        &action,
        &from,
        &to,
        &text,
        translated.as_deref().unwrap_or("（暂无译文）"),
        &cfg.explain_lang,
    );

    // 说明性文字用「讲解语言」，并明确禁止输出思考过程
    let system = crate::llm::with_lang_constraint(
        "你是一位专业的翻译讲师与双语编辑。回答要具体、可操作，不要客套话，\
         不要输出思考过程或分析步骤，直接给结论。",
        &cfg.explain_lang,
    );

    let out = crate::llm::chat_ex(
        &state.http(),
        &cfg.llm,
        &system,
        &user,
        0.5,
        cfg.llm.max_tokens.max(1024),
    )
    .await
    .map_err(err)?;

    if out.trim().is_empty() {
        return Err("模型没有返回内容".into());
    }
    Ok(out)
}

/* ---------------- 历史 / 收藏（需求 5） ---------------- */

/// 列出翻译历史。
#[tauri::command(async)]
pub fn cmd_translate_history(
    state: State<'_, Arc<AppState>>,
    limit: Option<i64>,
    only_favorite: Option<bool>,
) -> Result<Vec<crate::models::TransRecord>, String> {
    state
        .db
        .list_translations(limit.unwrap_or(200).clamp(1, 1000), only_favorite.unwrap_or(false))
        .map_err(err)
}

/// 收藏 / 取消收藏。
#[tauri::command(async)]
pub fn cmd_translate_favorite(
    state: State<'_, Arc<AppState>>,
    id: i64,
    on: bool,
) -> Result<(), String> {
    state.db.set_translation_favorite(id, on).map_err(err)
}

#[tauri::command(async)]
pub fn cmd_translate_delete(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.db.delete_translation(id).map_err(err)
}

/// 清空历史。`keep_favorite` 为真时保留收藏项。
#[tauri::command(async)]
pub fn cmd_translate_clear(
    state: State<'_, Arc<AppState>>,
    keep_favorite: Option<bool>,
) -> Result<usize, String> {
    state
        .db
        .clear_translations(keep_favorite.unwrap_or(true))
        .map_err(err)
}

/* ---------------- 方向（需求 1） ---------------- */

/// 一键互换互译方向，返回互换后的 `(源语言, 目标语言)`。
///
/// 源语言为「自动检测」时无法互换（不知道该换成什么），
/// 此时按「用户想反向翻译」的意图，把目标语言当作新的源语言。
#[tauri::command(async)]
pub fn cmd_swap_direction(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    let cur = state.cfg();
    let from = if cur.source_lang.trim().is_empty() {
        translate::AUTO.to_string()
    } else {
        cur.source_lang.clone()
    };
    let to = cur.target_lang.clone();

    let (new_from, new_to) = if from == translate::AUTO {
        // 自动检测 → 换成「把目标语言当作源语言」的反向
        (to.clone(), translate::default_target_for(&to))
    } else {
        (to.clone(), from.clone())
    };

    state
        .update_config(|c| {
            c.source_lang = new_from.clone();
            c.target_lang = new_to.clone();
        })
        .map_err(err)?;

    Ok(vec![new_from, new_to])
}

/// 可用的语言清单，供方向选择器渲染下拉。
#[tauri::command(async)]
pub fn cmd_translate_langs() -> Vec<serde_json::Value> {
    let mut out = vec![serde_json::json!({
        "code": translate::AUTO,
        "name": "自动检测",
        "self_name": "自动检测",
    })];
    for (code, name, self_name) in translate::LANGS {
        out.push(serde_json::json!({
            "code": code, "name": name, "self_name": self_name,
        }));
    }
    out
}

/// 当前限频冷却状态，供界面提示「在线翻译繁忙，已切到本地模型」。
#[tauri::command(async)]
pub fn cmd_translate_status() -> serde_json::Value {
    let cooling = translate::cooldown_remaining();
    serde_json::json!({
        "online_ready": cooling.is_none(),
        "cooldown_secs": cooling.map(|d| d.as_secs() as i64).unwrap_or(0),
    })
}
