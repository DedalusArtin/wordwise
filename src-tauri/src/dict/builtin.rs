//! 内置词典源定义（需求 2 & 6）。
//!
//! 全部走「通用配置 + 字段映射」这一条路径，与用户自定义源完全同构——
//! 这样内置源本身就是扩展的范例，新增语言只需追加配置。
//!
//! 数据源说明：
//! - `free-dictionary`：dictionaryapi.dev，开源免费，英英释义 + 音标 + 变形
//! - `wiktionary`：Wiktionary REST API，多语言支持最好的免费源
//! - `youdao-suggest`：有道公开建议接口，负责中文释义与词形变化
//! - `libre-translate`：LibreTranslate 兼容端点，负责翻译（可指向自建实例）
//! - `lmstudio`：本地大模型兜底，离线可用（在 llm 模块实现）

use crate::models::{DictSourceConfig as S, FieldMapping};
use std::collections::BTreeMap;

fn headers() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(
        "User-Agent".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) WordWise/1.0".to_string(),
    );
    m
}

/// 返回全部内置源。默认启用前三个。
pub fn default_sources() -> Vec<S> {
    vec![
        free_dictionary(),
        wiktionary(),
        youdao_suggest(),
        libre_translate(),
    ]
}

/// dictionaryapi.dev —— 英英释义、音标、变形、同反义词
pub fn free_dictionary() -> S {
    S {
        id: "free-dictionary".into(),
        name: "Free Dictionary (dictionaryapi.dev)".into(),
        builtin: true,
        enabled: true,
        langs: vec!["en".into()],
        url_template: "https://api.dictionaryapi.dev/api/v2/entries/en/{word}".into(),
        method: "GET".into(),
        headers: headers(),
        api_key: String::new(),
        priority: 10,
        mapping: FieldMapping {
            word: "[0].word".into(),
            phonetic_uk: "[0].phonetics[*].text".into(),
            phonetic_us: "[0].phonetic".into(),
            audio: "[0].phonetics[*].audio".into(),
            // 音标数组里空串居多，由解析层过滤
            senses: "[0].meanings".into(),
            sense_pos: "partOfSpeech".into(),
            sense_def: "definitions[0].definition".into(),
            sense_examples: "definitions[*].example".into(),
            example_text: "definitions[0].example".into(),
            example_translation: String::new(),
            inflections: String::new(),
            inflection_label: String::new(),
            inflection_form: String::new(),
            // dictionaryapi.dev 的 synonyms 挂在每个 meaning 内部，
            // 不在顶层，必须带 meanings[*] 前缀才能取到。
            related: "[0].meanings[*].synonyms".into(),
            mnemonic: String::new(),
        },
    }
}

/// Wiktionary  REST —— 多语言主源，天然支持小语种
pub fn wiktionary() -> S {
    S {
        id: "wiktionary".into(),
        name: "Wiktionary (多语言)".into(),
        builtin: true,
        enabled: true,
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
            word: "query".into(),
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
    }
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
}
