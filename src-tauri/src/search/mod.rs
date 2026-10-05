//! 在线搜索与知识获取（需求 2）。
//!
//! 提供三档搜索能力，逐层递进：
//! 1. **拼写建议**：拿有道/必应公开建议接口，输入前缀就能给候选词（输入联想）
//! 2. **词条搜索**：多词典源聚合（复用 dict 模块）
//! 3. **知识搜索**：抓取维基百科摘要，补充词条背后的百科知识
//!
//! 全部走 reqwest + rustls，不依赖任何需要 API Key 的服务即可用。

pub mod websearch;

use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

/// 一条搜索候选。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Suggestion {
    /// 候选词
    pub word: String,
    /// 简要释义（若有）
    pub gloss: String,
    /// 来源
    pub source: String,
}

/// 联想候选（输入时实时调用，必须快）。
pub async fn suggest(client: &reqwest::Client, prefix: &str, lang: &str) -> Vec<Suggestion> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return Vec::new();
    }

    // 并行请求多个建议源，谁快谁出结果
    let (a, b) = tokio::join!(
        youdao_suggest(client, prefix),
        bing_suggest(client, prefix, lang)
    );

    let mut out = a;
    for s in b {
        if !out.iter().any(|x| x.word.eq_ignore_ascii_case(&s.word)) {
            out.push(s);
        }
    }
    out.truncate(12);
    out
}

/// 有道建议接口。
async fn youdao_suggest(client: &reqwest::Client, q: &str) -> Vec<Suggestion> {
    let url = format!(
        "https://dict.youdao.com/suggest?num=8&doctype=json&q={}",
        urlencoding::encode(q)
    );
    let Ok(resp) = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .header("User-Agent", "Mozilla/5.0 WordWise/1.0")
        .send()
        .await
    else {
        return Vec::new();
    };
    let Ok(v) = resp.json::<Value>().await else {
        return Vec::new();
    };

    let mut out = Vec::new();
    if let Some(entries) = v
        .get("data")
        .and_then(|d| d.get("entries"))
        .and_then(|e| e.as_array())
    {
        for e in entries {
            let word = e
                .get("entry")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if word.is_empty() {
                continue;
            }
            out.push(Suggestion {
                word,
                gloss: e
                    .get("explain")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                source: "有道".into(),
            });
        }
    }
    out
}

/// 必应建议接口。
async fn bing_suggest(client: &reqwest::Client, q: &str, lang: &str) -> Vec<Suggestion> {
    let market = match lang {
        "en" => "en-US",
        "ja" => "ja-JP",
        _ => "zh-CN",
    };
    let url = format!(
        "https://api.bing.com/osjson.aspx?query={}&market={}",
        urlencoding::encode(q),
        market
    );
    let Ok(resp) = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .header("User-Agent", "Mozilla/5.0 WordWise/1.0")
        .send()
        .await
    else {
        return Vec::new();
    };
    let Ok(v) = resp.json::<Value>().await else {
        return Vec::new();
    };

    // 必应返回 ["query", ["a","b",...]]
    v.get(1)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str())
                .map(|s| Suggestion {
                    word: s.to_string(),
                    gloss: String::new(),
                    source: "必应".into(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 维基百科摘要——为词条补充百科级知识。
#[derive(Debug, Clone, serde::Serialize)]
pub struct WikiSummary {
    pub title: String,
    pub extract: String,
    pub url: String,
    pub thumbnail: String,
}

pub async fn wiki_summary(
    client: &reqwest::Client,
    word: &str,
    lang: &str,
) -> Option<WikiSummary> {
    // 中文查询用 zh 站点，其他语言用对应站点，找不到再退回 en
    let hosts: Vec<&str> = if lang == "en" {
        vec!["en"]
    } else {
        vec![lang, "en"]
    };

    for host in hosts {
        let url = format!(
            "https://{}.wikipedia.org/api/rest_v1/page/summary/{}",
            host,
            urlencoding::encode(word)
        );
        let Ok(resp) = client
            .get(&url)
            .timeout(Duration::from_secs(8))
            .header("User-Agent", "WordWise/1.0 (educational use)")
            .send()
            .await
        else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(v) = resp.json::<Value>().await else {
            continue;
        };

        let extract = v
            .get("extract")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if extract.trim().is_empty() {
            continue;
        }

        return Some(WikiSummary {
            title: v
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or(word)
                .to_string(),
            extract,
            url: v
                .get("content_urls")
                .and_then(|c| c.get("desktop"))
                .and_then(|d| d.get("page"))
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string(),
            thumbnail: v
                .get("thumbnail")
                .and_then(|t| t.get("source"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

/// 一条辞书跳转链接——本地词库没有的内容，让用户一键去权威辞书查看。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DictLink {
    /// 辞书名称，如「有道词典」
    pub name: String,
    /// 网页地址（已带查询词）
    pub url: String,
    /// 说明/特点，如「中文释义·例句」
    pub note: String,
    /// 是否国内可直连（用户反馈 wiki 国内能力差，据此排序）
    pub cn_friendly: bool,
}

/// 生成辞书跳转链接（需求 14）。
/// 依语言给出对应辞书，中文友好源排在前面。
pub fn dict_links(word: &str, lang: &str) -> Vec<DictLink> {
    let w = word.trim();
    if w.is_empty() {
        return Vec::new();
    }
    let e = urlencoding::encode(w);
    let mut out: Vec<DictLink> = Vec::new();

    let cn = |n: &str, u: String, note: &str| DictLink {
        name: n.into(), url: u, note: note.into(), cn_friendly: true,
    };
    let intl = |n: &str, u: String, note: &str| DictLink {
        name: n.into(), url: u, note: note.into(), cn_friendly: false,
    };

    match lang {
        "ja" => {
            out.push(cn("Moji 辞書", format!("https://www.mojidict.com/search?q={}", e), "日语·中文释义"));
            out.push(cn("沪江小D 日语", format!("https://dict.hjenglish.com/jp/jc/{}", e), "日语·中日对照"));
            out.push(cn("有道日语", format!("https://dict.youdao.com/result?word={}&lang=jp", e), "日语释义"));
            out.push(intl("Jisho", format!("https://jisho.org/search/{}", e), "英日日日英"));
        }
        "ko" => {
            out.push(cn("沪江小D 韩语", format!("https://dict.hjenglish.com/kr/{}", e), "韩语·中韩对照"));
            out.push(cn("有道韩语", format!("https://dict.youdao.com/result?word={}&lang=kr", e), "韩语释义"));
            out.push(intl("Naver 词典", format!("https://dict.naver.com/search.nhn?query={}", e), "韩语权威"));
        }
        "zh" => {
            out.push(cn("汉典", format!("https://www.zdic.net/hans/{}", e), "汉字·字源·古义"));
            out.push(cn("有道汉语", format!("https://dict.youdao.com/result?word={}&lang=zh", e), "汉语释义"));
            out.push(intl("MDBG", format!("https://www.mdbg.net/chinese/dictionary?page=worddict&wdrst=0&wdqb={}", e), "英汉汉英"));
        }
        _ => {
            // 英语为主
            out.push(cn("有道词典", format!("https://dict.youdao.com/result?word={}&lang=en", e), "中文释义·例句·发音"));
            out.push(cn("剑桥词典", format!("https://dictionary.cambridge.org/dictionary/english-chinese-simplified/{}", e), "英汉双解·权威"));
            out.push(cn("柯林斯", format!("https://www.collinsdictionary.com/dictionary/english-chinese/{}", e), "英语释义·用法"));
            out.push(intl("牛津学习词典", format!("https://www.oxfordlearnersdictionaries.com/definition/english/{}", e), "牛津·释义与搭配"));
            out.push(intl("Merriam-Webster", format!("https://www.merriam-webster.com/dictionary/{}", e), "美式英语权威"));
            out.push(intl("Wiktionary", format!("https://en.wiktionary.org/wiki/{}", e), "词源·多语言"));
        }
    }
    out
}

/// 综合搜索：给出候选 + 维基知识。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub query: String,
    pub suggestions: Vec<Suggestion>,
    pub wiki: Option<WikiSummary>,
}

pub async fn search(
    client: &reqwest::Client,
    q: &str,
    lang: &str,
    with_wiki: bool,
) -> Result<SearchResult> {
    let (suggestions, wiki) = if with_wiki {
        let (s, w) = tokio::join!(
            suggest(client, q, lang),
            wiki_summary(client, q, lang)
        );
        (s, w)
    } else {
        (suggest(client, q, lang).await, None)
    };

    Ok(SearchResult {
        query: q.to_string(),
        suggestions,
        wiki,
    })
}

/// 校验一个词是否形态合理（用于过滤搜索噪声）。
pub fn is_plausible_word(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > 40 {
        return false;
    }
    // 不允许整句（超过 4 个空格视为句子）
    if s.matches(' ').count() > 3 {
        return false;
    }
    s.chars().any(|c| c.is_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_word_filter() {
        assert!(is_plausible_word("apple"));
        assert!(is_plausible_word("苹果"));
        assert!(!is_plausible_word(""));
        assert!(!is_plausible_word("this is a very long sentence here"));
        assert!(!is_plausible_word(&"x".repeat(50)));
    }
}
