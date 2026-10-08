//! 词库内容增强：软件空闲时用本地大模型把「单薄词条」补成有模有样的详解。
//!
//! 用户需求原文：「AI 只是给一个简单的意思，词汇讲解太少。要求软件开着时
//! 后台给词库的词用 AI 补充内容；查询新的单词也用 AI + 在线资源补充；
//! 尽量做到有道词典的精细程度。」
//!
//! 分工：
//!   * **查词时**（在线 + 有道音标补全）—— 见 `dict::lookup_with_cache`：
//!     新词本来就走「在线多源 → LLM 兜底」；
//!   * **后台空闲时**（本模块）—— 词库里已经躺着的单薄词条，用
//!     [`llm::generate_entry`] 逐个补齐，回写词库。
//!
//! 三条底线，比「补得快」重要得多：
//!   1. **绝不覆盖用户已有内容** —— 模型只是「补齐材料」的来源之一，
//!      词库里的现有内容（词典导入 / 用户录入）权威性永远更高，
//!      所以合并方向永远是「现有为准，生成垫底」（[`merge_entry`]）；
//!   2. **绝不拖慢前台** —— 一局只处理一个词，LLM 推理本身就是 5~20 秒
//!      的天然节流；启动后先歇 [`START_DELAY_SECS`] 秒，让正事先跑完；
//!   3. **AI 没配就完全不跑** —— 纯增强能力，没配模型时安静待命，
//!      连一行日志都不刷。

use crate::llm;
use crate::models::LlmConfig;
use crate::state::AppState;
use tauri::Emitter; // AppHandle::emit（tauri 2 的 trait 方法）
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 启动后先让路多久（秒）。开屏渲染、词典索引、语言修复迁移都在抢 IO。
const START_DELAY_SECS: u64 = 90;
/// 一轮结束后的歇息（秒）——模型刚跑完一个词，给它也给自己喘口气。
const REST_PER_WORD_SECS: u64 = 10;
/// 网络层（音标/变形）两词之间的间隔 —— 有道 jsonapi 毫秒级返回，
/// 节奏只需要避开「打爆接口」，350ms 一个词，4964 个缺音标的词约 45 分钟刷完。
const NETWORK_GAP_MS: u64 = 350;

/// AI 未启用时空转的间隔（秒）。配置是热更新的，睡醒再看。
const IDLE_NO_LLM_SECS: u64 = 60;
/// 词库已全部丰满后的长歇（秒）。有新词入库后最多 5 分钟就会被发现。
const IDLE_DONE_SECS: u64 = 300;

/// 本次会话已充实的词条数（前端展示用，重启清零）。
static ENRICHED: AtomicU64 = AtomicU64::new(0);
/// 停止开关。目前只有进程退出这一个消费者，保留接口是为了以后
/// 「设置里关掉增强」不用改结构。
static STOP: AtomicBool = AtomicBool::new(false);

/// 增强进度（`cmd_enrich_status` 的返回值）。
pub fn status() -> serde_json::Value {
    serde_json::json!({
        "running": !STOP.load(Ordering::Relaxed),
        "enriched": ENRICHED.load(Ordering::Relaxed),
    })
}

/// 请求停止后台循环。
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// 词条是否仍缺关键内容（音标或释义）。
fn needs_enrich(e: &crate::models::WordEntry) -> bool {
    (e.phonetic.uk.trim().is_empty() && e.phonetic.us.trim().is_empty()) || e.senses.is_empty()
}

/// 把模型生成的词条**合并**进已有词条：只填缺失，绝不覆盖。
///
/// 这是整个增强引擎最重要的一条纪律。模型会出错（音标可能标错、释义
/// 可能偏窄），而词库里的现有内容要么来自词典、要么用户自己录入过，
/// 权威性都高于生成结果 —— 合并方向必须是「现有为准，生成垫底」。
///
/// ★ 例句要**逐义项补齐**（用户报「点了AI但是没有显示」后加的）：
///   旧逻辑只在 `base.senses` 整体为空时才拿 fresh 的 senses —— 而
///   「有释义没例句」正是最常见的状态（词表导入的词条），于是讲解里
///   辛辛苦苦生成的例句整组被丢掉，左栏例句永远停在占位。
pub(crate) fn merge_entry(
    mut base: crate::models::WordEntry,
    fresh: crate::models::WordEntry,
) -> crate::models::WordEntry {
    if base.phonetic.uk.trim().is_empty() {
        base.phonetic.uk = fresh.phonetic.uk;
    }
    if base.phonetic.us.trim().is_empty() {
        base.phonetic.us = fresh.phonetic.us;
    }
    if base.phonetic.audio.trim().is_empty() {
        base.phonetic.audio = fresh.phonetic.audio;
    }
    if base.senses.is_empty() {
        base.senses = fresh.senses;
    } else {
        // 逐义项补例句（只填空）：对位补，对不上位时把 fresh 里任意
        // 一组例句挂到第一条义项 —— 总比空着强，义项顺序本就不保证一致。
        for (i, bs) in base.senses.iter_mut().enumerate() {
            if !bs.examples.is_empty() {
                continue;
            }
            if let Some(fs) = fresh.senses.get(i).filter(|f| !f.examples.is_empty()) {
                bs.examples = fs.examples.clone();
            }
        }
        if base.senses.iter().all(|s| s.examples.is_empty()) {
            if let Some(fs) = fresh.senses.iter().find(|f| !f.examples.is_empty()) {
                if let Some(first) = base.senses.first_mut() {
                    first.examples = fs.examples.clone();
                }
            }
        }
    }
    if base.inflections.is_empty() {
        base.inflections = fresh.inflections;
    }
    if base.related.is_empty() {
        base.related = fresh.related;
    }
    if base.mnemonic.trim().is_empty() {
        base.mnemonic = fresh.mnemonic;
    }
    base
}

/// 词条是否缺「例句或词形变化」—— 讲解自动回写的触发条件。
///
/// 两样都齐才跳过：AI 讲解的价值对这类词条就是例句与变形，
/// 缺一样就值得跑一次回写。
pub fn explain_gap(e: &crate::models::WordEntry) -> bool {
    let has_example = e
        .senses
        .iter()
        .any(|s| s.examples.iter().any(|x| !x.text.trim().is_empty()));
    e.inflections.is_empty() || !has_example
}

/// 讲解自动回写词条（后台、只填空、完成广播）。
///
/// 兑现例句/变形占位文案「点下方 AI 讲解可生成例句与词形变化」：
/// `cmd_ai_explain` 只产讲解文本存讲解档，不碰词条 —— 不接这一层的话，
/// 左栏例句/变形永远停在占位（用户报「明明点了 AI 但是没有显示」）。
/// 流程：讲解 markdown → `entry_from_explain`（模型整理）→ `merge_entry`
/// （只填空）→ 回写 → 广播 `enrich://done`（查词页刷新、详情卡回源）。
/// 任何一步失败都静默返回 false —— 这是增强能力，不该弹错误。
pub async fn merge_explain_into_entry(
    state: &AppState,
    app: &tauri::AppHandle,
    llm_cfg: &LlmConfig,
    word: &str,
    lang: &str,
    markdown: &str,
) -> bool {
    let Ok(Some(base)) = state.db.get_word(word, lang) else {
        return false; // 词不在库（用户没加过）→ 不凭空建词条
    };
    if !explain_gap(&base) {
        return false; // 例句变形都齐了，省一次 LLM
    }
    let Ok(fresh) = llm::entry_from_explain(&state.http(), llm_cfg, word, lang, markdown).await
    else {
        return false;
    };
    // 一条像样的释义都没有 → 宁可不写也不污染词库（与手动并入同一条守则）
    if fresh.senses.iter().all(|s| s.definition.trim().is_empty()) {
        return false;
    }
    let merged = merge_entry(base, fresh);
    let Ok(json) = serde_json::to_string(&merged) else {
        return false;
    };
    if !matches!(state.db.update_word_entry_json(word, lang, &json), Ok(true)) {
        return false;
    }
    let _ = app.emit(
        "enrich://done",
        serde_json::json!({ "word": word, "lang": lang }),
    );
    true
}

/// 增强单个词条的结果 —— 决定主循环的节奏。
enum Outcome {
    /// LLM 补的：推理 5~20 秒，按词歇 [`REST_PER_WORD_SECS`]。
    Llm,
    /// 网络源补的（有道 jsonapi：音标 + 词形变化，免 AI）：毫秒级，
    ///   短歇继续刷 —— 词库 50% 的词只缺音标变形，全靠这层才补得动
    ///   （LLM 逐词跑要几十小时）。
    Network,
    /// 两层都没补上（断网 + AI 没开 / 词条被删）。
    Nothing,
}

/// 第一层：**免 AI 的网络补全**（有道 jsonapi）。
///
/// 只做两件事，且都遵守「只填空不覆盖」：缺音标 → 填英/美音标；
/// 缺词形变化 → 填 `wfs`（第三人称单数/过去式…，label 本来就是中文）。
/// 英语专用（jsonapi 是英语词典）；失败静默返回 false。
async fn enrich_network(
    state: &AppState,
    word: &str,
    lang: &str,
    base: &crate::models::WordEntry,
) -> bool {
    if !lang.eq_ignore_ascii_case("en") {
        return false;
    }
    let need_ph = base.phonetic.uk.trim().is_empty() && base.phonetic.us.trim().is_empty();
    let need_infl = base.inflections.is_empty();
    if !need_ph && !need_infl {
        return false;
    }
    let Some((uk, us, wfs)) = crate::dict::fetch_youdao_jsonapi(&state.http(), word).await
    else {
        return false;
    };
    let mut e = base.clone();
    let mut changed = false;
    if need_ph {
        let uk = crate::models::clean_phonetic_value(&uk, lang);
        let us = crate::models::clean_phonetic_value(&us, lang);
        if e.phonetic.uk.trim().is_empty() && !uk.is_empty() {
            e.phonetic.uk = uk;
            changed = true;
        }
        if e.phonetic.us.trim().is_empty() && !us.is_empty() {
            e.phonetic.us = us;
            changed = true;
        }
    }
    if need_infl && !wfs.is_empty() {
        e.inflections = wfs
            .into_iter()
            .map(|(label, form)| crate::models::Inflection { label, form })
            .collect();
        changed = true;
    }
    if !changed {
        return false;
    }
    let Ok(json) = serde_json::to_string(&e) else {
        return false;
    };
    matches!(state.db.update_word_entry_json(word, lang, &json), Ok(true))
}

/// 增强单个词条：**网络层（音标/变形，免 AI）→ AI 层（释义等）**。
///
/// 分层的理由（用户反馈「有些单词还没有音标」）：词库 9908 词里 4984 个
/// 缺音标 —— 全是词表导入没有音标列。这类词**只缺音标变形**，有道
/// jsonapi 毫秒级就能补齐，根本轮不到 LLM；把网络层放前面，AI 只处理
/// 真正缺释义的词条，速度差两个数量级。
async fn enrich_one(
    state: &AppState,
    llm_cfg: &LlmConfig,
    llm_ready: bool,
    word: &str,
    lang: &str,
) -> anyhow::Result<Outcome> {
    let Some(existing) = state.db.get_word(word, lang).ok().flatten() else {
        return Ok(Outcome::Nothing); // 用户已删，别再写回去
    };
    if !needs_enrich(&existing) {
        return Ok(Outcome::Nothing); // 扫描批次里混着的、已被补好的词
    }

    // 第一层：网络（音标 + 词形变化）
    let networked = enrich_network(state, word, lang, &existing).await;

    // 重取：网络层可能已经回写
    let Some(cur) = state.db.get_word(word, lang).ok().flatten() else {
        return Ok(if networked { Outcome::Network } else { Outcome::Nothing });
    };
    if !needs_enrich(&cur) {
        return Ok(if networked { Outcome::Network } else { Outcome::Nothing });
    }

    // 第二层：AI（补释义等网络源给不了的内容）；AI 未配就到此为止
    if !llm_ready {
        return Ok(if networked { Outcome::Network } else { Outcome::Nothing });
    }
    let fresh = llm::generate_entry(&state.http(), llm_cfg, word, lang).await?;
    let merged = merge_entry(cur, fresh);
    let json = serde_json::to_string(&merged)?;
    state.db.update_word_entry_json(word, lang, &json)?;
    Ok(Outcome::Llm)
}

/// 查词现场补全：本地命中的词条如果单薄（没音标/没释义），不等后台
/// 空闲轮，立即补一次。**AI 未启用时网络层照样工作**（音标补全不需要
/// 模型）。与后台循环并发安全：合并纪律「只填空」保证两边同时
/// 跑也不会互相覆盖。
pub async fn enrich_now(state: &AppState, word: &str, lang: &str) -> bool {
    let cfg = state.config.read().clone();
    let llm_ready = cfg.study.ai_explain && !cfg.llm.base_url.trim().is_empty();
    matches!(
        enrich_one(state, &cfg.llm, llm_ready, word, lang).await,
        Ok(Outcome::Llm) | Ok(Outcome::Network)
    )
}

/// 启动后台增强循环（整个应用生命周期一个任务）。
pub fn spawn(state: Arc<AppState>) {
    // 单测等非 runtime 环境构造 AppState 时直接跳过：增强是纯增强能力，
    // 测试不需要它，也绝不能让「后台任务起不来」影响主流程的构造。
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(START_DELAY_SECS)).await;
        loop {
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            let cfg = state.config.read().clone();
            let llm_ready = cfg.study.ai_explain && !cfg.llm.base_url.trim().is_empty();
            // ★ 不再因「AI 未配」整轮空转 —— 第一层网络补全（音标/变形）
            //   不需要模型；只有 LLM 层才受 llm_ready 门控。

            let candidates = match state.db.pick_words_to_enrich(120) {
                Ok(v) => v,
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(IDLE_NO_LLM_SECS)).await;
                    continue;
                }
            };
            if candidates.is_empty() {
                // 词库已够丰满：长歇，新词入库后最多 5 分钟就会被发现
                tokio::time::sleep(Duration::from_secs(IDLE_DONE_SECS)).await;
                continue;
            }

            let mut progressed = false;
            for (word, lang) in candidates {
                if STOP.load(Ordering::Relaxed) {
                    break;
                }
                match enrich_one(&state, &cfg.llm, llm_ready, &word, &lang).await {
                    Ok(Outcome::Llm) => {
                        ENRICHED.fetch_add(1, Ordering::Relaxed);
                        progressed = true;
                        tokio::time::sleep(Duration::from_secs(REST_PER_WORD_SECS)).await;
                    }
                    Ok(Outcome::Network) => {
                        // 网络层是毫秒级的：只给个网络往返的喘息，继续刷
                        ENRICHED.fetch_add(1, Ordering::Relaxed);
                        progressed = true;
                        tokio::time::sleep(Duration::from_millis(NETWORK_GAP_MS)).await;
                    }
                    _ => {
                        // 两层都没补上（断网 + AI 没开 / 词条被删）→ 换下一个，
                        // 绝不在同一个词上反复撞墙
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
            if !progressed {
                // 这一批全军覆没（多半是断网且模型没开）：长睡后再看
                tokio::time::sleep(Duration::from_secs(IDLE_NO_LLM_SECS * 2)).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合并纪律是整个引擎的命门：现有内容一个字都不能被生成结果覆盖。
    #[test]
    fn merge_never_overwrites_existing_content() {
        let mk = |uk: &str, mn: &str| crate::models::WordEntry {
            word: "test".into(),
            lang: "en".into(),
            phonetic: crate::models::Phonetic {
                uk: uk.into(),
                us: String::new(),
                audio: String::new(),
            },
            senses: vec![crate::models::Sense {
                pos: "n.".into(),
                definition: "已有释义".into(),
                examples: vec![],
            }],
            inflections: vec![],
            related: vec![],
            mnemonic: mn.into(),
            source: String::new(),
            extra: Default::default(),
        };

        let base = mk("/test/", "已有记忆法");
        let fresh = mk("/wrong/", "模型编的");
        let merged = merge_entry(base, fresh);
        assert_eq!(merged.phonetic.uk, "/test/", "已有音标绝不能被覆盖");
        assert_eq!(merged.mnemonic, "已有记忆法", "已有记忆法绝不能被覆盖");
    }

    /// ★ 「点了 AI 但没有显示」的病灶回归：有释义没例句的词条
    ///   （词表导入的常态）必须能接到 fresh 的例句与变形，
    ///   且自己的释义一个字都不能被覆盖。
    #[test]
    fn merge_fills_examples_into_existing_senses() {
        let mut base = crate::models::WordEntry::new("proceeding");
        base.lang = "en".into();
        base.senses.push(crate::models::Sense {
            pos: "n.".into(),
            definition: "进行；会议记录".into(),
            examples: vec![],
        });

        let mut fresh = crate::models::WordEntry::new("proceeding");
        fresh.lang = "en".into();
        fresh.senses.push(crate::models::Sense {
            pos: "n.".into(),
            definition: "（讲解里给的别的措辞）".into(),
            examples: vec![crate::models::Example {
                text: "The proceedings were published.".into(),
                translation: "会议记录已发表。".into(),
            }],
        });
        fresh.inflections.push(crate::models::Inflection {
            label: "现在分词".into(),
            form: "proceeding".into(),
        });

        let m = merge_entry(base, fresh);
        assert_eq!(m.senses[0].definition, "进行；会议记录", "释义绝不能被覆盖");
        assert_eq!(m.senses[0].examples.len(), 1, "例句必须补进已有义项");
        assert_eq!(m.inflections.len(), 1, "变形必须补上");
    }

    /// 讲解自动回写的触发闸：缺例句**或**缺变形就写；两样齐才跳过。
    #[test]
    fn explain_gap_gates_auto_writeback() {
        let mut e = crate::models::WordEntry::new("x");
        e.lang = "en".into();
        e.senses.push(crate::models::Sense {
            pos: "n.".into(),
            definition: "释义".into(),
            examples: vec![],
        });
        assert!(explain_gap(&e), "无例句无变形 → 要回写");

        e.inflections.push(crate::models::Inflection {
            label: "复数".into(),
            form: "xs".into(),
        });
        assert!(explain_gap(&e), "有变形但没例句 → 仍要回写");

        e.senses[0].examples.push(crate::models::Example {
            text: "An example.".into(),
            translation: "例句。".into(),
        });
        assert!(!explain_gap(&e), "例句变形都齐 → 跳过，省一次 LLM");
    }

    /// 缺失的字段必须被填上，否则引擎就是空转。
    #[test]
    fn merge_fills_missing_fields() {
        let mut base = crate::models::WordEntry::new("significance");
        base.lang = "en".into();
        let mut fresh = crate::models::WordEntry::new("significance");
        fresh.lang = "en".into();
        fresh.phonetic.us = "/sɪɡˈnɪfɪkəns/".into();
        fresh.senses.push(crate::models::Sense {
            pos: "n.".into(),
            definition: "意义；重要性".into(),
            examples: vec![],
        });
        fresh.mnemonic = "sign(标记) + ificance → 有标记的东西 → 重要".into();

        let merged = merge_entry(base, fresh);
        assert_eq!(merged.phonetic.us, "/sɪɡˈnɪfɪkəns/");
        assert_eq!(merged.senses.len(), 1);
        assert!(!merged.mnemonic.is_empty());
    }
}
