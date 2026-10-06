//! 在线网络搜索（需求 6）：为词条补充背景知识与权威链接。
//!
//! 这里是「在线搜索不出结果」的核心修复点。
//!
//! 老实现直接用 reqwest 抓 `cn.bing.com/search?q=...` 和 `www.baidu.com/s?wd=...`
//! 的 HTML，再用 `<h2>` 启发式抽标题链接。实测：
//! - 百度会 302 到 `wappass.baidu.com/static/captcha/...` 图形验证页（反爬），
//!   返回体只有 400 多字节，一条结果都抽不到；
//! - 必应 HTML 页面结构随版本变动，`<h2>` 规则经常失效。
//!
//! 所以改成：
//! - **必应走官方 RSS**（`&format=rss`）——返回结构化 XML，字段稳定、体积只有 HTML 的
//!   几十分之一、无需解析脚本，国内也能直连。这是默认引擎。
//! - 补充 **360 搜索**（`so.com`，国内可直连、可解析）与 **DuckDuckGo**（需代理）作为备选；
//! - 百度保留但会自动降级：抓不到结果就透明地改用必应 RSS，而不是给用户一个空列表。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::net;

/// 可选的搜索引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    /// 必应（默认，走 RSS 接口，中文结果质量好且国内可直连）
    Bing,
    /// 必应国际（英文结果）
    BingIntl,
    /// 360 搜索（国内可直连，作为必应之外的兜底）
    So360,
    /// 百度（受反爬限制，抓不到时自动切换到必应）
    Baidu,
    /// DuckDuckGo（需代理，英文结果较全）
    DuckDuckGo,
}

impl Default for SearchEngine {
    fn default() -> Self {
        SearchEngine::Bing
    }
}

impl SearchEngine {
    /// 引擎的展示名。
    pub fn label(&self) -> &'static str {
        match self {
            SearchEngine::Bing => "必应",
            SearchEngine::BingIntl => "必应国际",
            SearchEngine::So360 => "360 搜索",
            SearchEngine::Baidu => "百度",
            SearchEngine::DuckDuckGo => "DuckDuckGo",
        }
    }

    /// 构造搜索地址（网页版，用于「在浏览器里打开」等场景）。
    pub fn url(&self, q: &str) -> String {
        let e = urlencoding::encode(q);
        match self {
            SearchEngine::Bing => format!("https://cn.bing.com/search?q={}", e),
            SearchEngine::BingIntl => format!("https://www.bing.com/search?q={}&ensearch=1", e),
            SearchEngine::So360 => format!("https://www.so.com/s?q={}", e),
            SearchEngine::Baidu => format!("https://www.baidu.com/s?wd={}", e),
            SearchEngine::DuckDuckGo => format!("https://duckduckgo.com/?q={}", e),
        }
    }

    /// 把配置里的字符串解析成引擎枚举。
    ///
    /// 这是**唯一**的解析入口：`cmd_web_search`、AI 讲解的联网补充等场景
    /// 都从这里走，避免每个调用点各写一份 `match`（一字之差就会让
    /// 用户在选择器里挑了 360、实际却仍然打必应）。
    pub fn parse(s: &str) -> SearchEngine {
        match s.trim().to_ascii_lowercase().as_str() {
            "baidu" => SearchEngine::Baidu,
            "so360" | "360" | "so" => SearchEngine::So360,
            "bingintl" | "bing_intl" | "bing-intl" => SearchEngine::BingIntl,
            "duckduckgo" | "ddg" => SearchEngine::DuckDuckGo,
            _ => SearchEngine::Bing,
        }
    }

    /// 构造 RSS 地址（只有必应提供 RSS 输出）。
    fn rss_url(&self, q: &str) -> Option<String> {
        let e = urlencoding::encode(q);
        match self {
            SearchEngine::Bing => Some(format!("https://cn.bing.com/search?q={}&format=rss", e)),
            SearchEngine::BingIntl => {
                Some(format!("https://www.bing.com/search?q={}&format=rss&setlang=en", e))
            }
            _ => None,
        }
    }
}

/// 一条搜索结果。
#[derive(Debug, Clone, Serialize)]
pub struct WebResult {
    /// 结果标题
    pub title: String,
    /// 落地链接
    pub url: String,
    /// 摘要（若能抽到）
    pub snippet: String,
    /// 来源引擎
    pub engine: String,
}

/// 浏览器级请求头。缺了这些，国内搜索引擎基本都会当成爬虫拦掉。
fn browser_headers() -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    m.insert(
        "User-Agent".into(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
            .into(),
    );
    m.insert("Accept-Language".into(), "zh-CN,zh;q=0.9,en;q=0.8".into());
    m.insert(
        "Accept".into(),
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into(),
    );
    m
}

/// 对一段 HTML/XML 做实体解码（&amp; / &lt; / &#x27; 等常见实体）。
fn decode_entities(s: &str) -> String {
    let mut out = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    // 处理 &#数字; 形式的实体
    while let Some(start) = out.find("&#") {
        let Some(rel_end) = out[start..].find(';') else {
            break;
        };
        let end = start + rel_end;
        let num_str = &out[start + 2..end];
        let code = if let Some(hex) = num_str.strip_prefix('x').or_else(|| num_str.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            num_str.parse::<u32>().ok()
        };
        if let Some(c) = code.and_then(char::from_u32) {
            out.replace_range(start..=end, &c.to_string());
        } else {
            // 无法解析则跳过这个实体，防止死循环
            out.replace_range(start..=end, "");
        }
    }
    out
}

/// 去掉 HTML/XML 标签。
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).trim().to_string()
}

/// 去掉 CDATA 包裹。
fn unwrap_cdata(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("<![CDATA[") {
        if let Some(inner) = rest.strip_suffix("]]>") {
            return inner.trim().to_string();
        }
    }
    t.to_string()
}

/// 从 XML 片段里取某个标签的文本内容（取第一个匹配）。
fn xml_tag(block: &str, tag: &str) -> String {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let Some(i) = block.find(&open) else {
        return String::new();
    };
    let rest = &block[i + open.len()..];
    let Some(j) = rest.find(&close) else {
        return String::new();
    };
    decode_entities(&unwrap_cdata(&rest[..j]))
}

/// 解析必应 RSS 输出。
fn extract_results_rss(xml: &str, engine: &str, limit: usize) -> Vec<WebResult> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while out.len() < limit {
        let Some(rel) = xml[cursor..].find("<item>") else {
            break;
        };
        let start = cursor + rel + "<item>".len();
        let end = match xml[start..].find("</item>") {
            Some(e) => start + e,
            None => break,
        };
        let block = &xml[start..end];

        let title = xml_tag(block, "title");
        let url = xml_tag(block, "link");
        let snippet = strip_tags(&xml_tag(block, "description"));

        if !title.is_empty() && url.starts_with("http") {
            out.push(WebResult {
                title,
                url,
                snippet,
                engine: engine.to_string(),
            });
        }
        cursor = end + "</item>".len();
    }
    out
}

/// 从 HTML 里粗略抓取搜索结果（必应 / 360 通用启发式）。
///
/// 以 `<h2>` 或 `<h3>` 里的 `<a href="...">` 为锚点，向后取一段作为摘要。
fn extract_results_html(html: &str, engine: &str, limit: usize) -> Vec<WebResult> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while out.len() < limit {
        // 找下一个标题块：h2 或 h3 都认
        let next_h2 = html[cursor..].find("<h2");
        let next_h3 = html[cursor..].find("<h3");
        let (h_start, tag) = match (next_h2, next_h3) {
            (Some(a), Some(b)) => {
                if a <= b {
                    (cursor + a, "h2")
                } else {
                    (cursor + b, "h3")
                }
            }
            (Some(a), None) => (cursor + a, "h2"),
            (None, Some(b)) => (cursor + b, "h3"),
            (None, None) => break,
        };
        let close = format!("</{}>", tag);
        let Some(block_end_rel) = html[h_start..].find(&close) else {
            break;
        };
        let block_end = h_start + block_end_rel;
        let block = &html[h_start..block_end];

        let href = extract_attr(block, "data-mdurl=\"")
            .or_else(|| extract_attr(block, "href=\""))
            .or_else(|| extract_attr(block, "href='"));
        let title_html = {
            let after = block.find('>').map(|i| &block[i + 1..]).unwrap_or(block);
            strip_tags(after)
        };

        if let (Some(url), false) = (href, title_html.is_empty()) {
            if url.starts_with("http") && !is_engine_internal(&url) {
                // 摘要：取标题块之后 900 字符里的第一段
                let tail_end = (block_end + 900).min(html.len());
                let tail = &html[block_end..tail_end];
                let snippet = extract_first_para(tail);
                out.push(WebResult {
                    title: title_html,
                    url: decode_entities(&url),
                    snippet,
                    engine: engine.to_string(),
                });
            }
        }
        cursor = if block_end + close.len() <= html.len() {
            block_end + close.len()
        } else {
            html.len()
        };
        if cursor >= html.len() {
            break;
        }
    }
    out
}

/// 在片段里找第一个 `<p>`（或 `<div class="...desc...">`）当摘要。
fn extract_first_para(tail: &str) -> String {
    if let Some(i) = tail.find("<p") {
        let seg = &tail[i..];
        if let Some(j) = seg.find('>') {
            let s = &seg[j + 1..];
            if let Some(e) = s.find("</p>") {
                let t = strip_tags(&s[..e]);
                if !t.is_empty() {
                    return t;
                }
            }
        }
    }
    String::new()
}

/// 判断链接是否指向搜索引擎自身（这类链接对用户没有价值）。
fn is_engine_internal(url: &str) -> bool {
    // 注意：360 的 `so.com/link?m=...` 是真实结果的跳转链接，必须放行；
    // 只过滤搜索结果页、导航页这类纯内部地址。
    let u = url.to_ascii_lowercase();
    if u.contains("bing.com/search") || u.contains("bing.com/?") {
        return true;
    }
    if u.contains("baidu.com/s?") || u.contains("baidu.com/?") {
        return true;
    }
    if u.contains("so.com/s?") || u.contains("hao.360.com") {
        return true;
    }
    false
}

/// 从 HTML 片段里取属性值（简化版，不处理转义引号）。
fn extract_attr(s: &str, key: &str) -> Option<String> {
    let i = s.find(key)?;
    let rest = &s[i + key.len()..];
    let end = rest.find(['"', '\''])?;
    Some(decode_entities(&rest[..end]))
}

/// 执行一次网络搜索。必应优先走 RSS，失败再退回 HTML。
pub async fn web_search(
    client: &reqwest::Client,
    query: &str,
    engine: SearchEngine,
    limit: usize,
) -> Result<Vec<WebResult>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }

    // 1) 必应：RSS 优先
    if let Some(rss) = engine.rss_url(q) {
        match net::get_text(client, &rss, &browser_headers(), 10).await {
            Ok(xml) => {
                let hits = extract_results_rss(&xml, engine.label(), limit);
                if !hits.is_empty() {
                    return Ok(hits);
                }
                // 空结果也继续走 HTML 兜底（可能是接口改版）
            }
            Err(e) => {
                // 国际版走不通时退回国内站，避免整体失败
                if engine == SearchEngine::BingIntl {
                    if let Ok(xml) =
                        net::get_text(client, &SearchEngine::Bing.rss_url(q).unwrap(), &browser_headers(), 10)
                            .await
                    {
                        let hits = extract_results_rss(&xml, SearchEngine::Bing.label(), limit);
                        if !hits.is_empty() {
                            return Ok(hits);
                        }
                    }
                }
                let html = net::get_text(client, &engine.url(q), &browser_headers(), 10)
                    .await
                    .map_err(|e2| anyhow::anyhow!("{}；HTML 兜底同样失败：{}", e, e2))?;
                return Ok(extract_results_html(&html, engine.label(), limit));
            }
        }
        let html = net::get_text(client, &engine.url(q), &browser_headers(), 10).await?;
        return Ok(extract_results_html(&html, engine.label(), limit));
    }

    // 2) 百度：国内反爬严重，抓不到就透明降级到必应，而不是给用户空列表
    if engine == SearchEngine::Baidu {
        if let Ok(html) = net::get_text(client, &engine.url(q), &browser_headers(), 10).await {
            let hits = extract_results_html(&html, engine.label(), limit);
            if !hits.is_empty() {
                return Ok(hits);
            }
        }
        let rss = SearchEngine::Bing.rss_url(q).expect("必应 RSS 地址应可用");
        let xml = net::get_text(client, &rss, &browser_headers(), 10).await?;
        return Ok(extract_results_rss(&xml, "必应（百度受限已自动切换）", limit));
    }

    // 3) 360 / DuckDuckGo：HTML 抓取
    let html = net::get_text(client, &engine.url(q), &browser_headers(), 10).await?;
    Ok(extract_results_html(&html, engine.label(), limit))
}

/// 多引擎并发搜索，合并去重（前端只调一次即可拿到更全结果）。
///
/// 这里选的是「国内可直连」的组合：必应 RSS + 360。
/// 老实现里的百度会 302 到验证码页，等于白白浪费一次请求。
pub async fn multi_search(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Vec<WebResult> {
    let (a, b) = tokio::join!(
        web_search(client, query, SearchEngine::Bing, limit),
        web_search(client, query, SearchEngine::So360, limit)
    );
    let mut out: Vec<WebResult> = Vec::new();
    for r in a.unwrap_or_default().into_iter().chain(b.unwrap_or_default()) {
        // 同名同链接视为重复
        if out.iter().any(|x| x.url == r.url) {
            continue;
        }
        out.push(r);
    }
    out.truncate(limit);
    out
}

/// 供「探测」用的最短请求：只判断引擎是否可用。
pub async fn ping(client: &reqwest::Client, engine: SearchEngine) -> Result<Duration> {
    let started = std::time::Instant::now();
    let q = "test";
    if let Some(rss) = engine.rss_url(q) {
        net::get_text(client, &rss, &browser_headers(), 8).await?;
    } else {
        net::get_text(client, &engine.url(q), &browser_headers(), 8).await?;
    }
    Ok(started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt;"), "a & b <c>");
        assert_eq!(decode_entities("x &#39;y&#39;"), "x 'y'");
        assert_eq!(decode_entities("中 &#x4e2d; 文"), "中 中 文");
    }

    #[test]
    fn tags_strip() {
        assert_eq!(strip_tags("<b>hello</b> <i>world</i>"), "hello world");
    }

    #[test]
    fn engine_urls() {
        assert!(SearchEngine::Bing.url("apple").contains("cn.bing.com"));
        assert!(SearchEngine::So360.url("苹果").contains("so.com"));
    }

    #[test]
    fn bing_has_rss_others_not() {
        assert!(SearchEngine::Bing.rss_url("apple").unwrap().contains("format=rss"));
        assert!(SearchEngine::BingIntl.rss_url("apple").is_some());
        assert!(SearchEngine::So360.rss_url("apple").is_none());
        assert!(SearchEngine::Baidu.rss_url("apple").is_none());
    }

    #[test]
    fn extract_from_bing_rss() {
        let xml = r#"
        <rss><channel>
          <item><title>Apple 苹果</title><link>https://example.com/apple</link>
            <description>苹果是一种水果。</description></item>
          <item><title>Pear</title><link>https://example.org/pear</link>
            <description><![CDATA[梨 &amp; 相关]]></description></item>
          <item><title>bad</title><link>not-a-url</link><description>x</description></item>
        </channel></rss>"#;
        let r = extract_results_rss(xml, "必应", 5);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].title, "Apple 苹果");
        assert_eq!(r[0].url, "https://example.com/apple");
        assert_eq!(r[0].snippet, "苹果是一种水果。");
        // CDATA + 实体都要能处理
        assert_eq!(r[1].snippet, "梨 & 相关");
    }

    #[test]
    fn extract_from_html_h2_and_h3() {
        let html = r#"
            <li><h2><a href="https://example.com/apple">Apple 苹果</a></h2>
            <p>苹果是一种水果。</p></li>
            <li><h3><a href="https://cn.bing.com/search?q=x">should skip</a></h3></li>
            <li><h3 class="t"><a href="https://www.so.com/link?m=abc">Pear 梨</a></h3>
            <p>梨。</p></li>
            <li><h3><a data-mdurl="https://example.org/pear">Pear</a></h3></li>
        "#;
        let r = extract_results_html(html, "必应", 5);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].title, "Apple 苹果");
        assert_eq!(r[0].url, "https://example.com/apple");
        assert_eq!(r[0].snippet, "苹果是一种水果。");
        // 360 的跳转链接要保留（它是真实结果的入口）
        assert_eq!(r[1].url, "https://www.so.com/link?m=abc");
        // data-mdurl 优先
        assert_eq!(r[2].url, "https://example.org/pear");
    }

    #[test]
    fn engine_internal_filter() {
        assert!(is_engine_internal("https://cn.bing.com/search?q=x"));
        assert!(is_engine_internal("https://www.baidu.com/s?wd=x"));
        assert!(!is_engine_internal("https://www.so.com/link?m=abc"));
        assert!(!is_engine_internal("https://example.com/x"));
    }

    #[test]
    fn parse_engine_config_strings() {
        // 配置里写的是小写 id；大小写与空格都要能容忍
        assert_eq!(SearchEngine::parse("bing"), SearchEngine::Bing);
        assert_eq!(SearchEngine::parse(" Bing "), SearchEngine::Bing);
        assert_eq!(SearchEngine::parse("so360"), SearchEngine::So360);
        assert_eq!(SearchEngine::parse("360"), SearchEngine::So360);
        assert_eq!(SearchEngine::parse("baidu"), SearchEngine::Baidu);
        assert_eq!(SearchEngine::parse("bingIntl"), SearchEngine::BingIntl);
        assert_eq!(SearchEngine::parse("ddg"), SearchEngine::DuckDuckGo);
        // 未知值退回默认，绝不能 panic（旧配置里可能是空串）
        assert_eq!(SearchEngine::parse(""), SearchEngine::Bing);
        assert_eq!(SearchEngine::parse("google"), SearchEngine::Bing);
    }
}
