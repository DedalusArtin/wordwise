//! 内置词典源定义（需求 2 & 6）。
//!
//! 全部走「通用配置 + 字段映射」这一条路径，与用户自定义源完全同构——
//! 这样内置源本身就是扩展的范例，新增语言只需追加配置。
//!
//! **默认启用的源必须在国内直连可达。** 这是硬约束，不是偏好：
//! 应用默认不启用代理（见 `NetworkConfig::enable_proxy`），所以任何
//! 「默认开启但需要翻墙」的源都会变成每次查词都要白等一次的坑。
//! `default_sources_need_no_proxy` 测试会在编译期把这个约束钉死。
//!
//! 数据源说明：
//! - `free-dictionary`：freedictionaryapi.com，英英释义 + 音标 + 词形变化
//! - `wiktionary`：Wiktionary REST API，多语言支持最好，但**国内不可直连**，
//!   故默认关闭，需要代理时才建议打开
//! - `youdao-suggest`：有道公开建议接口，负责**英语词**的中文释义与词形变化，国内直连
//! - `youdao-jsonapi`：有道 jsonapi 的 `ce` 段，负责中/日/韩/法/德/西/俄词的**中文**解释
//! - `youdao-newhh`：有道 jsonapi 的 `newhh` 段（《现代汉语规范词典》），中文词最佳
//! - `libre-translate`：LibreTranslate 兼容端点，负责翻译（需自备实例/Key）
//! - `lmstudio`：本地大模型兜底，离线可用（在 llm 模块实现）

use crate::models::{DictSourceConfig as S, FieldMapping};
use std::collections::BTreeMap;

/// 所有内置源共用的请求头。部分站点对空 UA 会直接拒绝。
fn headers() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "User-Agent".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) WordWise/1.0".to_string(),
    );
    m
}

/// 返回全部内置源。默认启用的只有「国内直连可达」的那几个。
pub fn default_sources() -> Vec<S> {
    vec![
        free_dictionary(),
        wiktionary(),
        youdao_suggest(),
        youdao_jsonapi_ce(),
        youdao_jsonapi_newhh(),
        libre_translate(),
    ]
}

/// 英英释义主源：freedictionaryapi.com
///
/// 为什么换掉原来的 `api.dictionaryapi.dev`：该接口 2026-10 实测
/// **对所有单词都返回 404**（apple、hello、run 都一样，返回的是一个
/// 静态托管的 404 页面）。它原本还是优先级最高的源，等于每次查词
/// 都先白白等一次必然失败。freedictionaryapi.com 国内直连返回 200，
/// 底层数据同样来自 Wiktionary（CC BY-SA 4.0），字段比原来更规整：
/// 音标、词形变化、同义词都在顶层给全了。
pub fn free_dictionary() -> S {
    S {
        id: "free-dictionary".into(),
        name: "英英释义 (freedictionaryapi)".into(),
        builtin: true,
        enabled: true,
        langs: vec!["en".into()],
        url_template: "https://freedictionaryapi.com/api/v1/entries/en/{word}".into(),
        method: "GET".into(),
        headers: headers(),
        api_key: String::new(),
        priority: 10,
        mapping: FieldMapping {
            // {"word": "...", "entries": [{...}], "source": {...}}
            word: "word".into(),
            // 每个 entry 有多个 IPA 读音，取第一个带斜杠的（解析层已做过滤）
            phonetic_uk: "entries[0].pronunciations[*].text".into(),
            phonetic_us: String::new(),
            // 该接口不提供音频
            audio: String::new(),
            // senses 挂在每个 entry（= 一个词性）下，结构上是对象列表
            senses: "entries".into(),
            sense_pos: "partOfSpeech".into(),
            sense_def: "senses[0].definition".into(),
            // 例句在该接口里就是字符串数组，直接取文本
            sense_examples: "senses[*].examples[*]".into(),
            example_text: String::new(),
            example_translation: String::new(),
            // 词形变化：{"word": "ran", "tags": ["past"]}
            inflections: "entries[*].forms[*]".into(),
            inflection_label: "tags[0]".into(),
            inflection_form: "word".into(),
            related: "entries[*].synonyms".into(),
            mnemonic: String::new(),
        },
        // 国内可直连，无需代理
        needs_proxy: false,
    }
}

/// Wiktionary REST —— 多语言主源，天然支持小语种
///
/// **默认关闭**：`wiktionary.org` 属于 Wikimedia 域名，中国大陆直连必然
/// 超时（实测 000）。应用默认不启用代理，所以把它默认开着只会让每次查词
/// 多一条「已跳过」的噪声；需要小语种支持并且有代理的用户，在设置页
/// 手动开启即可。
pub fn wiktionary() -> S {
    S {
        id: "wiktionary".into(),
        name: "Wiktionary (多语言，需代理)".into(),
        builtin: true,
        enabled: false,
        // 覆盖常见小语种，新增语言只需在此加代码
        langs: vec![
            "en".into(), "ja".into(), "fr".into(), "de".into(),
            "es".into(), "ru".into(), "ko".into(), "it".into(),
        ],
        url_template:
            "https://{lang}.wiktionary.org/api/rest_v1/page/definition/{word}".into(),
        method: "GET".into(),
        headers: headers(),
        api_key: String::new(),
        priority: 20,
        mapping: FieldMapping {
            word: "title".into(),
            phonetic_uk: String::new(),
            phonetic_us: String::new(),
            audio: String::new(),
            senses: "$".into(),
            sense_pos: "partOfSpeech".into(),
            sense_def: "definitions[0].definition".into(),
            sense_examples: "definitions[*].examples[*]".into(),
            example_text: String::new(),
            example_translation: String::new(),
            inflections: String::new(),
            inflection_label: String::new(),
            inflection_form: String::new(),
            related: String::new(),
            mnemonic: String::new(),
        },
        // wiktionary.org 属于 Wikimedia 域名，中国大陆直连必然超时。
        // 标 true 后：有代理时正常使用，没代理时直接跳过（而不是干等超时）。
        needs_proxy: true,
    }
}

/// 有道建议接口 —— 提供中文释义与词形变化，质量稳定
pub fn youdao_suggest() -> S {
    S {
        id: "youdao-suggest".into(),
        name: "有道词典 (中文释义)".into(),
        builtin: true,
        enabled: true,
        langs: vec!["en".into(), "ja".into(), "ko".into(), "fr".into(), "de".into(), "es".into()],
        url_template: "https://dict.youdao.com/suggest?num=3&doctype=json&q={word}".into(),
        method: "GET".into(),
        headers: headers(),
        api_key: String::new(),
        priority: 30,
        mapping: FieldMapping {
            // ★ 词头必须取**联想命中的真实词条**，不能用 `query`（那是把
            //   用户输入原样回显）：查 grievaunce 时有道联想回的是 grievance
            //   的整套中文释义，用 query 做词头等于把别人家的释义挂在这个
            //   不存在的词头上，用户一上有道查「根本没有这个词」。
            //   取 entries[0].entry 后，pick_entry 的词头错配过滤会把这种
            //   「联想替身」整源丢掉。
            word: "data.entries[0].entry".into(),
            phonetic_uk: String::new(),
            phonetic_us: String::new(),
            audio: String::new(),
            senses: "data.entries".into(),
            sense_pos: "type".into(),
            sense_def: "explain".into(),
            sense_examples: String::new(),
            example_text: String::new(),
            example_translation: String::new(),
            inflections: String::new(),
            inflection_label: String::new(),
            inflection_form: String::new(),
            related: String::new(),
            mnemonic: String::new(),
        },
        // 有道是国内可直连的源，也是无代理环境下最稳定的一个
        needs_proxy: false,
    }
}

/// 有道 `jsonapi` —— 非英语词（含中文）的**中文**释义。
///
/// 为什么必须补这一条：原先 `youdao-suggest` 是唯一的中文释义源，但它
/// 本质是「中→英」查词接口——实测 `q=开心` 返回的是 `feel happy; be
/// delighted…`，`q=日本語` 返回 `Ocenebra japonica`，全部是**英文**解释
/// （响应里 `"language":"en"`）。于是「选日语查中文词」时，用户看到的是
/// 一条挂着「日语」标签的英文释义——这就是「多语言适配有问题」的现场。
/// 更糟的是 `youdao-suggest` 的 `langs` 不含 `zh`，中文词在直连环境下
/// **一个可用源都没有**，只能落到本地大模型兜底。
///
/// `jsonapi` 的 `ce` 段给出的是中文解释（`#tran` 字段），中/日/韩/法/德/西/俄
/// 都能命中，实测：
/// - `开心`   → 「感到快乐：情绪愉悦，心情愉快的状态。」
/// - `日本語` → 「日本（人）的；日语的；日本文化的；日本人；日语；」
pub fn youdao_jsonapi_ce() -> S {
    S {
        id: "youdao-jsonapi".into(),
        name: "有道释义 API (中文解释)".into(),
        builtin: true,
        enabled: true,
        langs: vec![
            "zh".into(), "ja".into(), "ko".into(), "fr".into(),
            "de".into(), "es".into(), "ru".into(),
        ],
        url_template: "https://dict.youdao.com/jsonapi?q={word}".into(),
        method: "GET".into(),
        headers: headers(),
        api_key: String::new(),
        // 比 youdao-suggest(30) 更靠前：有中文解释时先用它
        priority: 22,
        mapping: FieldMapping {
            word: "meta.input".into(),
            // 非英语词给不出国际音标，这里放的是读音/拼音（如「kāi xīn」）
            phonetic_uk: "simple.word[0].phone".into(),
            phonetic_us: String::new(),
            audio: String::new(),
            senses: "ce.word[0].trs[*].tr[*]".into(),
            sense_pos: "l.pos".into(),
            sense_def: "l.#tran".into(),
            sense_examples: String::new(),
            example_text: String::new(),
            example_translation: String::new(),
            inflections: String::new(),
            inflection_label: String::new(),
            inflection_form: String::new(),
            related: String::new(),
            mnemonic: String::new(),
        },
        needs_proxy: false,
    }
}

/// 有道 `jsonapi` 的 `newhh` 段 —— 《现代汉语规范词典》，中文词的最佳释义。
///
/// 只对中文词有效（其他语言没有这一段），因此 `langs` 只写 `zh`。
/// 相比 `ce` 段，它额外提供词性（形容词/动词）和规范例句。
pub fn youdao_jsonapi_newhh() -> S {
    S {
        id: "youdao-newhh".into(),
        name: "现代汉语规范词典 (有道)".into(),
        builtin: true,
        enabled: true,
        langs: vec!["zh".into()],
        url_template: "https://dict.youdao.com/jsonapi?q={word}".into(),
        method: "GET".into(),
        headers: headers(),
        api_key: String::new(),
        // 中文词里最权威，排最前
        priority: 18,
        mapping: FieldMapping {
            word: "newhh.dataList[0].word".into(),
            phonetic_uk: "simple.word[0].phone".into(),
            phonetic_us: String::new(),
            audio: String::new(),
            senses: "newhh.dataList[0].sense".into(),
            sense_pos: "cat".into(),
            // `def` 是数组（如 ["心情愉快；高兴"]），靠 query_text 拼接
            sense_def: "def".into(),
            sense_examples: "examples".into(),
            example_text: String::new(),
            example_translation: String::new(),
            inflections: String::new(),
            inflection_label: String::new(),
            inflection_form: String::new(),
            related: String::new(),
            mnemonic: String::new(),
        },
        needs_proxy: false,
    }
}

/// LibreTranslate 兼容端点 —— 翻译能力，可指向自建实例或公有实例
pub fn libre_translate() -> S {
    S {
        id: "libre-translate".into(),
        name: "LibreTranslate (翻译)".into(),
        builtin: true,
        // 默认关闭：需要自行指定可用实例或 API Key
        enabled: false,
        langs: vec![
            "en".into(), "zh".into(), "ja".into(), "fr".into(), "de".into(),
            "es".into(), "ru".into(), "ko".into(), "it".into(), "pt".into(),
            "ar".into(), "hi".into(),
        ],
        // LibreTranslate 是 POST + JSON body 接口，词本身放在请求体里。
        // 这里带上 `{word}` 查询串：一是便于用户直接粘贴到浏览器自测，
        // 二是让「所有源模板都必须含 {word}」这一约束保持一致（POST 时不参与 URL 渲染）。
        url_template: "https://libretranslate.com/translate?q={word}".into(),
        method: "POST".into(),
        headers: headers(),
        api_key: String::new(),
        priority: 40,
        mapping: FieldMapping {
            word: String::new(),
            phonetic_uk: String::new(),
            phonetic_us: String::new(),
            audio: String::new(),
            senses: String::new(),
            sense_pos: String::new(),
            sense_def: "translatedText".into(),
            sense_examples: String::new(),
            example_text: String::new(),
            example_translation: String::new(),
            inflections: String::new(),
            inflection_label: String::new(),
            inflection_form: String::new(),
            related: String::new(),
            mnemonic: String::new(),
        },
        needs_proxy: false,
    }
}

/// 用最新的内置源定义刷新用户配置里的内置源。
///
/// 为什么需要它：词典源是持久化在配置里的，光改 `default_sources()`
/// 只能影响新装用户。老用户的配置里存着旧定义（比如已经失效的
/// `dictionaryapi.dev` 地址），如果不刷新，修好的源永远到不了他们手上。
///
/// 刷新策略：
/// - URL 模板、字段映射、`needs_proxy`、语言列表、显示名 —— 一律以代码为准；
/// - `enabled` / `api_key` / `priority` —— 尊重用户自己的选择，保留；
/// - 用户新增的自定义源（`builtin == false`）原样保留，追加在末尾。
///
/// 注意：这个函数只看「代码里的最新定义」，不看版本号，所以**不要每次启动都调**。
/// 设置页允许用户修改内置源的 URL，每次都刷会把用户的手工调整冲掉。
/// 正常入口是 `migrate_config()`，它只在配置版本落后时执行一次。
pub fn refresh_builtin_sources(cfg: &mut crate::models::AppConfig) {
    use std::collections::HashMap;

    // 记录用户对内置源做过的选择
    let mut overrides: HashMap<String, (bool, String, i32)> = cfg
        .dict_sources
        .iter()
        .filter(|s| s.builtin)
        .map(|s| (s.id.clone(), (s.enabled, s.api_key.clone(), s.priority)))
        .collect();

    let mut merged: Vec<S> = default_sources()
        .into_iter()
        .map(|mut d| {
            if let Some((enabled, api_key, priority)) = overrides.remove(&d.id) {
                d.enabled = enabled;
                d.api_key = api_key;
                d.priority = priority;
            }
            d
        })
        .collect();

    // 用户自定义源原样保留
    merged.extend(cfg.dict_sources.iter().filter(|s| !s.builtin).cloned());

    cfg.dict_sources = merged;
}

/// 一次性的配置迁移。返回是否发生了改动（调用方据此决定要不要落盘）。
///
/// 只在 `config_version` 落后于 [`crate::models::CONFIG_VERSION`] 时执行，
/// 这样既能保证「失效的内置源地址」这类修复能送到老用户手上，
/// 又不会在之后每次启动都把用户对内置源的手工调整覆盖掉。
///
/// 除了刷新内置源定义，这里还会做一件归一化的事：**未启用代理时，
/// 把标记了 `needs_proxy` 的内置源关掉**。旧版本的默认配置里
/// wiktionary 是开着的，而应用默认直连，留着它只会让每次查词都多一条
/// 「已跳过」的噪声。
pub fn migrate_config(cfg: &mut crate::models::AppConfig) -> bool {
    let current = cfg.config_version.unwrap_or(0);
    if current >= crate::models::CONFIG_VERSION {
        return false;
    }

    refresh_builtin_sources(cfg);

    // v4：把「写死了只用简体中文」的老默认人设换成语言中性的新默认。
    //
    // 只替换**确认没被用户改过**的（等于内置默认或为空）：
    // 用户自己调过的人设一律不动 —— 那是他的内容，我们只在运行时追加
    // 《输出语言》约束，并靠后处理兜底，不做破坏性覆盖。
    if cfg.llm.system_prompt.trim().is_empty()
        || crate::models::is_builtin_prompt(&cfg.llm.system_prompt)
    {
        cfg.llm.system_prompt = crate::models::DEFAULT_TUTOR_PROMPT.to_string();
    }

    // 讲解语言为空（老配置缺字段）时补默认中文
    if cfg.explain_lang.trim().is_empty() {
        cfg.explain_lang = "zh".to_string();
    }

    // v5：互译方向。老配置里 source_lang 是被清空的空串（当时语言下拉已被合并
    // 成一个），现在它是「源语言」这个明确的角色，空串要变成「自动检测」。
    if cfg.source_lang.trim().is_empty() {
        cfg.source_lang = crate::translate::AUTO.to_string();
    }

    if !cfg.network.enable_proxy {
        for s in cfg.dict_sources.iter_mut() {
            if s.builtin && s.needs_proxy {
                s.enabled = false;
            }
        }
    }

    cfg.config_version = Some(crate::models::CONFIG_VERSION);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sources_non_empty() {
        let s = default_sources();
        assert!(s.len() >= 3);
        assert!(s.iter().any(|x| x.enabled));
    }

    #[test]
    fn all_sources_have_unique_ids() {
        let s = default_sources();
        let mut ids: Vec<&str> = s.iter().map(|x| x.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), s.len(), "词典源 id 必须唯一");
    }

    #[test]
    fn url_templates_contain_placeholder() {
        for s in default_sources() {
            assert!(
                s.url_template.contains("{word}"),
                "源 {} 的 URL 模板缺少 {{word}} 占位符",
                s.id
            );
        }
    }

    /// 默认启用的源必须全部无需代理。
    ///
    /// 应用默认不启用代理，所以这条是硬约束：一旦有人往默认开启的列表里
    /// 塞了一个需要翻墙的源，用户拿到的就是「每次查词都白等一次超时」。
    #[test]
    fn default_sources_need_no_proxy() {
        for s in default_sources() {
            if s.enabled {
                assert!(
                    !s.needs_proxy,
                    "源 {} 默认启用却标记需要代理：默认配置是直连，它永远不可能生效",
                    s.id
                );
            }
        }
    }

    /// 默认启用集合里必须至少有「英英释义」和「中文释义」两类，
    /// 保证直连环境下也能查出完整词条（不是只有一条中文或只有一条英文）。
    #[test]
    fn default_enabled_covers_english_and_chinese() {
        let enabled: Vec<S> = default_sources().into_iter().filter(|s| s.enabled).collect();
        assert!(
            enabled.iter().any(|s| s.id == "free-dictionary"),
            "默认应启用英英释义源"
        );
        assert!(
            enabled.iter().any(|s| s.id == "youdao-suggest"),
            "默认应启用中文释义源"
        );
    }

    /// 直连环境下必须存在能给出「中文释义」的源，且要覆盖中/日两种语言。
    ///
    /// 这条是「多语言适配」的守门测试：`youdao-suggest` 的 `langs` 不含 `zh`，
    /// 且它对中/日词返回的是英文解释；一旦有人把 `youdao-jsonapi` 从默认集合
    /// 里拿掉，中文词在直连环境就会退化成「一个源都没有」。
    #[test]
    fn default_enabled_gives_chinese_gloss_for_zh_and_ja() {
        let enabled: Vec<S> = default_sources().into_iter().filter(|s| s.enabled).collect();
        for lang in ["zh", "ja"] {
            let hit = enabled
                .iter()
                .any(|s| s.langs.iter().any(|l| l == lang));
            assert!(hit, "默认启用的源里没有任何一个支持 {} 词", lang);
        }
        assert!(
            enabled
                .iter()
                .any(|s| s.id == "youdao-jsonapi" && s.langs.iter().any(|l| l == "ja")),
            "日语词的中文解释依赖 youdao-jsonapi"
        );
    }

    /// 迁移内置源：URL 以代码为准，用户选择被保留，自定义源不丢。
    #[test]
    fn migrate_refreshes_builtins_and_keeps_user_choices() {
        use crate::models::AppConfig;

        let mut cfg = AppConfig::default();
        cfg.config_version = None; // 模拟旧配置
        // 模拟「老配置」：旧地址 + 用户手动关掉了有道 + 一个自定义源
        cfg.dict_sources = vec![
            {
                let mut s = free_dictionary();
                s.url_template = "https://api.dictionaryapi.dev/api/v2/entries/en/{word}".into();
                s
            },
            {
                let mut s = youdao_suggest();
                s.enabled = false;
                s.priority = 99;
                s
            },
            {
                let mut s = wiktionary();
                // 老配置里 wiktionary 是开着的
                s.enabled = true;
                s
            },
            {
                let mut s = free_dictionary();
                s.id = "my-custom".into();
                s.builtin = false;
                s.name = "我的自定义源".into();
                s
            },
        ];

        assert!(migrate_config(&mut cfg), "旧配置应触发迁移");

        let get = |id: &str| cfg.dict_sources.iter().find(|s| s.id == id).cloned();

        // 失效地址被换掉
        let fd = get("free-dictionary").expect("内置源应存在");
        assert!(
            fd.url_template.contains("freedictionaryapi.com"),
            "内置源的失效 URL 应被刷新"
        );
        // 用户关掉的仍然关着，优先级保留
        let yd = get("youdao-suggest").expect("内置源应存在");
        assert!(!yd.enabled, "用户关闭有道源的意愿应被保留");
        assert_eq!(yd.priority, 99, "用户调整的优先级应被保留");
        // 未启用代理时，需要代理的源不应处于启用态
        let wk = get("wiktionary").expect("内置源应存在");
        assert!(!wk.enabled, "未启用代理时不应启用需要代理的源");
        // 自定义源原样保留
        assert!(cfg.dict_sources.iter().any(|s| s.id == "my-custom"));
        // 版本号被推进
        assert_eq!(cfg.config_version, Some(crate::models::CONFIG_VERSION));
    }

    /// 迁移是「一次性」的：跑过一次之后，用户对内置源的改动不会被再覆盖。
    #[test]
    fn migrate_runs_only_once() {
        use crate::models::AppConfig;

        let mut cfg = AppConfig::default();
        cfg.config_version = None;
        assert!(migrate_config(&mut cfg));
        assert!(!migrate_config(&mut cfg), "已迁移过的配置不应再次迁移");

        // 用户手工改了内置源 URL，重启后（再跑一次迁移）必须原样保留
        let custom_url = "https://my-own-mirror.example/dict?w={word}";
        if let Some(s) = cfg
            .dict_sources
            .iter_mut()
            .find(|s| s.id == "free-dictionary")
        {
            s.url_template = custom_url.into();
        }
        assert!(!migrate_config(&mut cfg));
        let fd = cfg
            .dict_sources
            .iter()
            .find(|s| s.id == "free-dictionary")
            .unwrap();
        assert_eq!(
            fd.url_template, custom_url,
            "迁移只应发生一次，用户手工调整的内置源 URL 不能被覆盖"
        );
    }

    /// v4 迁移：把「写死简体中文」的老默认人设换成语言中性的新默认。
    ///
    /// 这正是「选了目标语言，讲解还是全英文/全中文」的根因 —— prompt 是
    /// **持久化**的，光改代码里的默认值追不到老用户，只能在迁移里订正。
    #[test]
    fn migrate_v4_replaces_hardcoded_chinese_prompt() {
        use crate::models::{AppConfig, DEFAULT_TUTOR_PROMPT, LEGACY_TUTOR_PROMPT_V1};

        // ① 老默认人设（写死了简体中文）→ 应被语言中性版本替换
        let mut cfg = AppConfig::default();
        cfg.config_version = Some(3);
        cfg.llm.system_prompt = LEGACY_TUTOR_PROMPT_V1.to_string();
        cfg.explain_lang = String::new();

        assert!(migrate_config(&mut cfg), "v3 配置应触发迁移");
        assert_eq!(cfg.llm.system_prompt, DEFAULT_TUTOR_PROMPT);
        assert!(
            !cfg.llm.system_prompt.contains("使用简体中文讲解"),
            "新默认人设不能再写死输出语言"
        );
        assert_eq!(cfg.explain_lang, "zh", "缺失的讲解语言要补默认中文");
    }

    /// 迁移不能覆盖用户自己调过的人设 —— 那是他的内容。
    #[test]
    fn migrate_v4_keeps_custom_prompt() {
        use crate::models::AppConfig;

        let mine = "你是一位严厉的词汇老师，用最少的字讲清楚。";
        let mut cfg = AppConfig::default();
        cfg.config_version = Some(3);
        cfg.llm.system_prompt = mine.to_string();

        migrate_config(&mut cfg);
        assert_eq!(cfg.llm.system_prompt, mine, "用户自定义的人设不能被覆盖");
    }

    /// 启用代理时，需要代理的内置源不该被迁移强行关掉。
    #[test]
    fn migrate_keeps_proxy_source_when_proxy_enabled() {
        use crate::models::AppConfig;

        let mut cfg = AppConfig::default();
        cfg.config_version = None;
        cfg.network.enable_proxy = true;
        cfg.dict_sources = vec![{
            let mut s = wiktionary();
            s.enabled = true;
            s
        }];

        migrate_config(&mut cfg);
        let wk = cfg
            .dict_sources
            .iter()
            .find(|s| s.id == "wiktionary")
            .unwrap();
        assert!(wk.enabled, "启用代理后，需要代理的源应保持用户设置的开启状态");
    }
}
