/* ============================================================
   extra.rs —— 补充命令（需求 4 / 6 / 7）
   1. 在线网络搜索（必应 / 百度，替代维基）
   2. 语言转换方向（源语言 → 目标语言）
   3. 已背过的单词回看
   4. 学习会话的高级模式（拼写 / 例句 / 听音拼写）
   ============================================================ */

use crate::models::{NetProbeItem, NetReport, QuizCard, QuizMode, WordEntry};
use crate::search::websearch::{self, SearchEngine, WebResult};
use crate::state::AppState;
use crate::timeutil;
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    format!("{}", e)
}

/// 可选的搜索引擎清单（供前端下拉框）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineOption {
    pub id: String,
    pub label: String,
    pub cn_friendly: bool,
}

/// 列出可用搜索引擎。
///
/// 顺序即推荐顺序：必应走 RSS 接口最稳，360 作为国内兜底，
/// 百度受反爬限制（会跳验证码页）所以排在后面。
#[tauri::command]
pub fn cmd_search_engines() -> Vec<EngineOption> {
    vec![
        EngineOption { id: "bing".into(), label: "必应".into(), cn_friendly: true },
        EngineOption { id: "so360".into(), label: "360 搜索".into(), cn_friendly: true },
        EngineOption { id: "baidu".into(), label: "百度（可能受限）".into(), cn_friendly: true },
        EngineOption { id: "bingintl".into(), label: "必应国际".into(), cn_friendly: false },
        EngineOption { id: "duckduckgo".into(), label: "DuckDuckGo".into(), cn_friendly: false },
    ]
}

/// 把配置里的字符串解析成引擎枚举。
///
/// 实现在 [`SearchEngine::parse`]，这里只保留一层薄包装：AI 讲解的联网补充
/// 也要用同一套解析规则，两处各写一份 `match` 迟早会漂移。
fn parse_engine(s: &str) -> SearchEngine {
    SearchEngine::parse(s)
}

/// 在线网络搜索（需求 6）。默认并发必应+百度，合并结果。
#[tauri::command]
pub async fn cmd_web_search(
    state: State<'_, Arc<AppState>>,
    query: String,
    engine: Option<String>,
    limit: Option<i64>,
    both: Option<bool>,
) -> Result<Vec<WebResult>, String> {
    let cfg = state.cfg();
    let limit = limit.unwrap_or(8).clamp(1, 20) as usize;
    if both.unwrap_or(false) {
        return Ok(websearch::multi_search(&state.http(), &query, limit).await);
    }
    let eng = parse_engine(&engine.unwrap_or(cfg.search_engine));
    websearch::web_search(&state.http(), &query, eng, limit)
        .await
        .map_err(err)
}

/// 查询界面用：给定单词，返回「在线搜索链接」列表（一键跳转更详细解释）。
/// 需求 6 的「本地缺少权威释义时显示辞书网页链接」。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LookupLink {
    pub name: String,
    pub url: String,
    pub note: String,
    /// 是否国内可直连
    pub cn_friendly: bool,
}

#[tauri::command]
pub fn cmd_lookup_links(word: String, lang: Option<String>) -> Vec<LookupLink> {
    crate::search::dict_links(&word, lang.as_deref().unwrap_or("en"))
        .into_iter()
        .map(|d| LookupLink {
            name: d.name,
            url: d.url,
            note: d.note,
            cn_friendly: d.cn_friendly,
        })
        .collect()
}

/// 设置「查询方向」：源语言与目标语言（需求 7）。
#[tauri::command]
pub fn cmd_set_direction(
    state: State<'_, Arc<AppState>>,
    source_lang: Option<String>,
    target_lang: Option<String>,
    search_engine: Option<String>,
) -> Result<(), String> {
    state
        .update_config(|c| {
            if let Some(s) = source_lang {
                c.source_lang = s;
            }
            if let Some(t) = target_lang {
                c.target_lang = t;
            }
            if let Some(e) = search_engine {
                c.search_engine = e;
            }
        })
        .map_err(err)
}

/// 取当前方向设置。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DirectionInfo {
    pub source_lang: String,
    pub target_lang: String,
    pub search_engine: String,
}

#[tauri::command]
pub fn cmd_get_direction(state: State<'_, Arc<AppState>>) -> DirectionInfo {
    let c = state.cfg();
    DirectionInfo {
        source_lang: c.source_lang,
        target_lang: c.target_lang,
        search_engine: c.search_engine,
    }
}

/// 为「例句类」题目挑一个带例句的词条（需求 4）。
/// 若当前会话题面的词没有例句，会从同语言词库里再找一个有例句的词。
pub fn pick_example_entry(state: &AppState, lang: &str, seed: &str) -> Option<WordEntry> {
    // 先看种子词本身
    if let Ok(Some(e)) = state.db.get_word(seed, lang) {
        if e.has_example() {
            return Some(e);
        }
    }
    // 再遍历词库找一个有例句的
    let pool = state.db.random_words(lang, 40).ok()?;
    pool.into_iter().find(|e| e.has_example())
}

/// 统计某语言下「有例句」的词数量，供界面提示。
#[tauri::command]
pub fn cmd_example_coverage(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
) -> Result<i64, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let total = state.db.word_count(&lang).map_err(err)?;
    // 抽样 200 个词估算覆盖率，避免全表扫描
    let sample = state.db.random_words(&lang, 200).map_err(err)?;
    let with_ex = sample.iter().filter(|e| e.has_example()).count() as f64;
    let ratio = if sample.is_empty() {
        0.0
    } else {
        with_ex / sample.len() as f64
    };
    Ok((total as f64 * ratio).round() as i64)
}

/// 会话高级模式支持（需求 4）：练习模式清单，附带说明，供前端渲染。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModeInfo {
    pub id: String,
    pub label: String,
    pub desc: String,
    /// 是否手动输入（拼写类）
    pub typing: bool,
    /// 是否用例句
    pub example: bool,
    /// 是否需要音频
    pub audio: bool,
}

/// 列出全部练习模式（拼写 / 例句 / 听音拼写等）。
#[tauri::command]
pub fn cmd_quiz_modes() -> Vec<ModeInfo> {
    let mk = |id: &str, label: &str, desc: &str, m: QuizMode| ModeInfo {
        id: id.into(),
        label: label.into(),
        desc: desc.into(),
        typing: m.is_typing(),
        example: m.uses_example(),
        audio: matches!(m, QuizMode::ListenSpell),
    };
    vec![
        mk("EnToZh", "英→中", "看英文选中文释义", QuizMode::EnToZh),
        mk("ZhToEn", "中→英", "看中文选英文单词", QuizMode::ZhToEn),
        mk("Spelling", "拼写练习", "看释义，手动拼出单词（百词斩式）", QuizMode::Spelling),
        mk("ExToZh", "例句选义", "看例句，选出正确释义", QuizMode::ExToZh),
        mk("ExPickWord", "例句识词", "在例句中识别目标单词", QuizMode::ExPickWord),
        mk("ListenSpell", "听音拼写", "听发音后拼出单词", QuizMode::ListenSpell),
    ]
}

/// 按词库开始一轮背诵（需求 4 / 5）：只从指定词库抽词。
///
/// `book_id` 为空或 "root" 时退化为全库出题。
#[tauri::command]
pub fn cmd_start_book_session(
    state: State<'_, Arc<AppState>>,
    book_id: Option<String>,
    mode: Option<QuizMode>,
    size: Option<i64>,
    lang: Option<String>,
    def_lang: Option<String>,
) -> Result<crate::commands::SessionInfo, String> {
    let cfg = state.cfg();
    let now = timeutil::now_ts();
    let limit = size.unwrap_or(cfg.study.batch_size).clamp(1, 200);
    let mode = mode.unwrap_or_default();
    let def_lang = def_lang.unwrap_or_default();

    let book = book_id.unwrap_or_default();
    // 未指定语言时按**词库自身**的语言出题：
    // 在学英语时点「背这本」打开日语词库，若按 target_lang 出题会一道都出不出来。
    let lang = match lang {
        Some(l) if !l.trim().is_empty() => l.trim().to_string(),
        _ => if book.is_empty() || book == "root" {
            cfg.target_lang.clone()
        } else {
            state
                .db
                .get_wordbook(&book)
                .ok()
                .flatten()
                .map(|b| b.lang)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| cfg.target_lang.clone())
        },
    };
    let mut picked: Vec<WordEntry> = Vec::new();

    if book.is_empty() || book == "root" {
        // 退化为全库：复用主流程的「到期优先」策略
        for st in state.db.due_states(&lang, now, limit).map_err(err)? {
            if let Ok(Some(e)) = state.db.get_word(&st.word, &lang) {
                picked.push(e);
            }
        }
    } else {
        // 指定词库：先从「未学过的」里取，再从全库里补
        for r in state
            .db
            .unscheduled_words_in_book(&book, &lang, limit)
            .map_err(err)?
        {
            picked.push(r.entry);
        }
    }

    // 不足则用该词库的全量词补足
    if (picked.len() as i64) < limit && !book.is_empty() && book != "root" {
        let seen: std::collections::BTreeSet<String> =
            picked.iter().map(|e| e.word.clone()).collect();
        for r in state
            .db
            .words_in_book(&book, &lang, limit * 3, 0)
            .map_err(err)?
        {
            if (picked.len() as i64) >= limit {
                break;
            }
            if !seen.contains(&r.word) {
                picked.push(r.entry);
            }
        }
    }

    if picked.is_empty() {
        return Err("该词库暂无单词，请先导入或下载词库".into());
    }

    let total = picked.len();
    {
        let mut s = state.session.write();
        s.queue = picked;
        s.index = 0;
        s.mode = Some(mode);
        s.correct = 0;
        s.wrong = 0;
        s.started_at = now;
        s.leech_only = false;
        s.def_lang = def_lang;
        // 新一轮不沿用上一轮的「答错回插」预算
        s.requeue_counts.clear();
    }

    Ok(crate::commands::SessionInfo {
        mode,
        total,
        index: 0,
        correct: 0,
        wrong: 0,
        leech_only: false,
    })
}

/// 拼写题判分（需求 4）：忽略大小写与首尾空格；给出提示字符。
///
/// 返回 (是否正确, 规范化后的用户输入, 规范化后的正确答案)
#[tauri::command]
pub fn cmd_check_spelling(input: String, answer: String, strict: Option<bool>) -> SpellingCheck {
    let norm = |s: &str| {
        let t = s.trim().to_string();
        if strict.unwrap_or(false) {
            t
        } else {
            t.to_lowercase()
        }
    };
    let a = norm(&answer);
    let u = norm(&input);
    SpellingCheck {
        correct: !u.is_empty() && u == a,
        user_input: u,
        answer: a,
        // 逐字符比对，供前端高亮「哪几个字母对了」
        matched_prefix: {
            let mut n = 0usize;
            for (x, y) in input.trim().chars().zip(answer.trim().chars()) {
                if x.to_ascii_lowercase() == y.to_ascii_lowercase() {
                    n += 1;
                } else {
                    break;
                }
            }
            n
        },
    }
}

/// 拼写判分结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpellingCheck {
    pub correct: bool,
    pub user_input: String,
    pub answer: String,
    /// 前缀匹配的字符数（用于逐步提示）
    pub matched_prefix: usize,
}

/// 生成拼写提示（需求 4）：按难度给出掩码，如 a _ _ l e。
#[tauri::command]
pub fn cmd_spell_hint(word: String, reveal: Option<i64>) -> String {
    let chars: Vec<char> = word.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    // reveal 表示「会保留的首字母个数 + 尾字母个数」
    let r = reveal.unwrap_or(0).clamp(0, 3) as usize;
    if r == 0 {
        return chars
            .iter()
            .map(|c| if c.is_whitespace() { " ".to_string() } else { "_".to_string() })
            .collect::<Vec<String>>()
            .join(" ");
    }
    let mut out = Vec::with_capacity(n);
    for (i, c) in chars.iter().enumerate() {
        let keep = i < r || i + r >= n;
        if *c == ' ' {
            out.push(' ');
        } else if keep {
            out.push(*c);
        } else {
            out.push('_');
        }
    }
    out.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ")
}

/// 例句挖空（需求 4）：把例句里的目标词替换为 ____，用于「例句识词」。
#[tauri::command]
pub fn cmd_mask_example(sentence: String, word: String) -> String {
    let s = &sentence;
    if word.trim().is_empty() {
        return s.clone();
    }
    // 大小写不敏感替换，保留前后标点
    let lower = s.to_lowercase();
    let w = word.to_lowercase();
    if let Some(pos) = lower.find(&w) {
        let mut out = String::with_capacity(s.len());
        out.push_str(&s[..pos]);
        out.push_str("____");
        out.push_str(&s[pos + w.len()..]);
        out
    } else {
        // 词形变化（如复数/时态）时退化为按词干匹配
        let stem = if w.len() > 4 { &w[..w.len() - 1] } else { &w[..] };
        if let Some(pos) = lower.find(stem) {
            let end = (pos + stem.len() + 2).min(s.len());
            let mut out = String::with_capacity(s.len());
            out.push_str(&s[..pos]);
            out.push_str("____");
            out.push_str(&s[end..]);
            out
        } else {
            s.clone()
        }
    }
}

/// 构造一道进阶题（需求 4）：支持拼写 / 例句 / 听音。
/// 与 `cmd_current_question` 不同，这里会确保例句类题目真的带例句。
#[tauri::command]
pub fn cmd_build_advanced_card(
    state: State<'_, Arc<AppState>>,
    mode: QuizMode,
    lang: Option<String>,
    // 会话类型：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽（需求 14）。
    kind: Option<String>,
) -> Result<Option<QuizCard>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let (entry, index, total, correct, wrong, is_leech, def_lang) = {
        let s = state.session_slot(kind.as_deref()).read();
        if !s.is_active() {
            return Ok(None);
        }
        let Some(e) = s.current().cloned() else {
            return Ok(None);
        };
        let leech = state
            .db
            .get_state(&e.word, &lang)
            .ok()
            .flatten()
            .map(|st| st.is_leech)
            .unwrap_or(false);
        (e, s.index, s.total(), s.correct, s.wrong, leech, s.def_lang.clone())
    };

    let card = crate::commands::build_card_public(
        &state,
        &entry,
        mode,
        &lang,
        is_leech,
        index + 1,
        total,
        correct,
        wrong,
        &def_lang,
    )?;
    Ok(Some(card))
}

/* ============================================================
   网络诊断（修复「开了代理仍提示网络问题」）
   ============================================================ */

/// 当前生效的代理信息（设置页展示用）。
#[tauri::command]
pub fn cmd_network_info(state: State<'_, Arc<AppState>>) -> crate::net::ProxyResolution {
    state.proxy_info()
}

/// 按最新配置重建 HTTP 客户端。
///
/// 用户在设置页改完代理后调用，无需重启应用。
#[tauri::command]
pub fn cmd_reload_network(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::net::ProxyResolution, String> {
    state.reload_http().map_err(err)
}

/// 网络诊断：逐个探测关键站点，告诉用户「到底哪一条链路不通、走没走代理」。
///
/// 之所以要做这个：之前失败信息只有一句「网络错误」，用户即使开了代理
/// 也完全无法判断是应用没读到代理、还是目标站真的访问不了。
///
/// 探测清单以**默认配置（直连）**下必须可用的端点为主；需要代理的端点
/// 单独标注，避免用户误以为「必须开代理才能用」。
///
/// 三个可选入参用于**按界面上尚未保存的待测配置**临时探测一次：
/// `enable_proxy` / `proxy` / `use_system_proxy`。**一个都不传时回退到当前
/// 已生效的配置**，所以旧的「不传参」调用行为完全不变（Tauri 对 `Option`
/// 参数在缺省时给 `None`）。
///
/// 之所以需要它们：本命令原先读的是 `state.http()`，也就是**上一次保存后**
/// 才生效的客户端。用户改完代理输入框、还没点保存就来点「诊断网络」，
/// 看到的自然是旧结论——于是误以为「诊断证明我配的代理没用」，其实参数
/// 根本没提交。
#[tauri::command]
pub async fn cmd_network_report(
    state: State<'_, Arc<AppState>>,
    enable_proxy: Option<bool>,
    proxy: Option<String>,
    use_system_proxy: Option<bool>,
) -> Result<NetReport, String> {
    let mut cfg = state.cfg();

    // 只有调用方确实传了「待测配置」才临时重建客户端；否则沿用当前生效的那个。
    let pending = enable_proxy.is_some() || proxy.is_some() || use_system_proxy.is_some();
    let (client, info) = if pending {
        if let Some(b) = enable_proxy {
            cfg.network.enable_proxy = b;
        }
        if let Some(p) = proxy {
            // 与设置页保存时的处理保持一致：去掉首尾空白，避免把「 127.0.0.1:7890 」
            // 当成一个非法地址而误报「代理地址无效」。
            cfg.network.proxy = p.trim().to_string();
        }
        if let Some(b) = use_system_proxy {
            cfg.network.use_system_proxy = b;
        }
        let (c, r) = crate::net::build_client_with(&cfg.network).map_err(err)?;
        (c, r)
    } else {
        (state.http(), state.proxy_info())
    };

    // 远端探测的超时与真实请求对齐：真实客户端的超时就是配置里的 timeout_secs。
    // 用一个更短的超时去探测，只会在代理握手慢时假报失败（见 net.rs 的说明）。
    let remote_timeout = cfg.network.timeout_secs.clamp(5, 600);

    // 探测项：(名称, 地址, 是否为「本地地址」)
    //
    // jsDelivr 的三个后端单独探：它们是三家不同 CDN，互为独立故障域，
    // 只要有一条通，词库就能下载。合成一项会让用户看不出「到底挂了几条」。
    let mut targets: Vec<(&str, String, bool)> = vec![
        (
            "在线词库镜像 1（jsDelivr·cdn）",
            "https://cdn.jsdelivr.net/gh/mahavivo/english-wordlists@master/CET4_edited.txt".into(),
            false,
        ),
        (
            "在线词库镜像 2（jsDelivr·fastly）",
            "https://fastly.jsdelivr.net/gh/mahavivo/english-wordlists@master/CET4_edited.txt"
                .into(),
            false,
        ),
        (
            "在线词库镜像 3（jsDelivr·gcore）",
            "https://gcore.jsdelivr.net/gh/mahavivo/english-wordlists@master/CET4_edited.txt"
                .into(),
            false,
        ),
        (
            "中文释义（有道词典）",
            "https://dict.youdao.com/suggest?num=1&doctype=json&q=apple".into(),
            false,
        ),
        (
            "英英释义（freedictionaryapi）",
            "https://freedictionaryapi.com/api/v1/entries/en/apple".into(),
            false,
        ),
        (
            "在线搜索（必应 RSS）",
            "https://cn.bing.com/search?q=test&format=rss".into(),
            false,
        ),
        (
            "GitHub 原文（国内需代理，仅兜底）",
            "https://raw.githubusercontent.com/mahavivo/english-wordlists/master/CET4_edited.txt"
                .into(),
            false,
        ),
        (
            "维基词典（需代理，默认已关闭）",
            "https://en.wiktionary.org/api/rest_v1/page/definition/apple".into(),
            false,
        ),
    ];

    // 本地大模型单独放最后：它走本机回环，必须直连
    let llm_url = format!("{}/models", cfg.llm.base_url.trim_end_matches('/'));
    targets.push(("本地大模型（LM Studio）", llm_url, true));

    let mut items: Vec<NetProbeItem> = Vec::new();

    for (name, url, is_local) in targets {
        let started = std::time::Instant::now();
        // 本机地址用独立的直连客户端，避免被代理配置误伤；
        // 远端则用**本次实际生效**的客户端（可能来自待测配置），超时也用它自己的。
        let (probe_client, probe_timeout) = if is_local {
            (crate::net::client_direct(6), 6u64)
        } else {
            (client.clone(), remote_timeout)
        };
        let ok = crate::net::probe_with_timeout(&probe_client, &url, probe_timeout).await;
        let elapsed_ms = started.elapsed().as_millis() as i64;
        items.push(NetProbeItem {
            name: name.into(),
            url,
            ok,
            elapsed_ms,
            detail: if ok {
                format!("正常（{} ms）", elapsed_ms)
            } else if is_local {
                // 本机探测永远直连，失败基本只有一个原因，直接说出来更有用
                format!("本机直连不可达：本地服务可能未启动（{} ms）", elapsed_ms)
            } else {
                // ★ 把「本次探测实际走的是哪条路径」写进每一项：用户才能自己
                //   对照下面的 no_proxy 判断「是不是我的绕过列表把这个域名
                //   放行成直连了」。
                format!(
                    "不可达：超时或被拒绝（{} ms；本次探测走{}）",
                    elapsed_ms,
                    info.path.label()
                )
            },
        });
    }

    let using_proxy = !info.is_direct();
    // 结论里把「实际走的路径」和「生效的 no_proxy 排除表」一并给出：
    // 如果某个目标域名出现在排除表里，它就**不经过代理**——用户一眼就能看出
    // 问题出在自己的绕过配置上，而不是笼统的一句「网络错误」。
    let proxy = format!(
        "{}；{}",
        info.describe(),
        crate::net::describe_probe_path(&cfg.network, &info)
    );

    Ok(NetReport {
        proxy,
        proxy_url: info.url.clone(),
        using_proxy,
        proxy_origin: info.origin.clone(),
        items,
    })
}
