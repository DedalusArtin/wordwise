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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 启动后先让路多久（秒）。开屏渲染、词典索引、语言修复迁移都在抢 IO。
const START_DELAY_SECS: u64 = 90;
/// 一轮结束后的歇息（秒）——模型刚跑完一个词，给它也给自己喘口气。
const REST_PER_WORD_SECS: u64 = 10;
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
fn merge_entry(mut base: crate::models::WordEntry, fresh: crate::models::WordEntry) -> crate::models::WordEntry {
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

/// 增强单个词条：取现有 → 生成 → 合并 → 回写。
/// 返回 `Ok(true)` 表示词条确实被充实了。
async fn enrich_one(
    state: &AppState,
    llm_cfg: &LlmConfig,
    word: &str,
    lang: &str,
) -> anyhow::Result<bool> {
    let Some(existing) = state.db.get_word(word, lang).ok().flatten() else {
        return Ok(false); // 用户已删，别再写回去
    };
    if !needs_enrich(&existing) {
        return Ok(false); // 扫描批次里混着的、已被补好的词
    }
    let fresh = llm::generate_entry(&state.http(), llm_cfg, word, lang).await?;
    let merged = merge_entry(existing, fresh);
    let json = serde_json::to_string(&merged)?;
    state.db.update_word_entry_json(word, lang, &json)
}

/// 查词现场补全：本地命中的词条如果单薄（没音标/没释义），不等后台
/// 空闲轮，立即补一次。AI 未启用时直接返回 false（前端不等待）。
/// 与后台循环并发安全：合并纪律「现有为准，生成垫底」保证两边同时
/// 跑也不会互相覆盖。
pub async fn enrich_now(state: &AppState, word: &str, lang: &str) -> bool {
    let cfg = state.config.read().clone();
    let llm_ready = cfg.study.ai_explain && !cfg.llm.base_url.trim().is_empty();
    if !llm_ready {
        return false;
    }
    matches!(enrich_one(state, &cfg.llm, word, lang).await, Ok(true))
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
            if !llm_ready {
                tokio::time::sleep(Duration::from_secs(IDLE_NO_LLM_SECS)).await;
                continue;
            }

            let candidates = match state.db.pick_words_to_enrich(120) {
                Ok(v) => v,
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(IDLE_NO_LLM_SECS)).await;
                    continue;
                }
            };
            if candidates.is_empty() {
                // 词库已够丰满：长歇，新词入库后最多 5 分钟会被发现
                tokio::time::sleep(Duration::from_secs(IDLE_DONE_SECS)).await;
                continue;
            }

            let mut progressed = false;
            for (word, lang) in candidates {
                if STOP.load(Ordering::Relaxed) {
                    break;
                }
                match enrich_one(&state, &cfg.llm, &word, &lang).await {
                    Ok(true) => {
                        ENRICHED.fetch_add(1, Ordering::Relaxed);
                        progressed = true;
                        tokio::time::sleep(Duration::from_secs(REST_PER_WORD_SECS)).await;
                    }
                    _ => {
                        // 单词失败（模型没响应 / JSON 不合法）→ 换下一个，
                        // 绝不在同一个词上反复撞墙
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
            if !progressed {
                // 这一批全军覆没，多半是模型没开：长睡后再看配置
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
