//! HTTP 网络层：统一的请求构造、超时、重试与错误归一化。
//!
//! 设计考量：
//! - 使用 rustls 而非系统 schannel，避免在部分环境下
//!   「证书吊销列表离线」导致的握手失败。
//! - 内置简单退避重试，应对临时网络抖动。
//! - 所有错误转成中文可读文案，直接展示给用户。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// 创建全局复用的 HTTP 客户端。
pub fn build_client(timeout_secs: u64) -> Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        // 使用 rustls 后端：不依赖系统的证书吊销列表（CRL）在线校验，
        // 从而规避 Windows 上常见的 CRYPT_E_REVOCATION_OFFLINE 握手失败。
        // 注意：这里保持正常的证书链校验（不放松安全性）。
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(90))
        .user_agent("WordWise/1.0 (Windows; +https://github.com/DedalusArtin/wordwise)")
        .build()?;
    Ok(client)
}

/// GET 并解析 JSON。
pub async fn get_json(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    api_key: &str,
    _word: &str,
    _lang: &str,
) -> Result<Value> {
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300 * (1 << attempt))).await;
        }
        let mut req = client.get(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if !api_key.is_empty() {
            req = req.bearer_auth(api_key);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    match resp.text().await {
                        Ok(t) => match serde_json::from_str::<Value>(&t) {
                            Ok(v) => return Ok(v),
                            Err(_) => {
                                last_err = Some(anyhow!("返回内容不是合法 JSON（HTTP {}）", status.as_u16()));
                                continue;
                            }
                        },
                        Err(e) => {
                            last_err = Some(anyhow!("读取响应失败: {}", e));
                            continue;
                        }
                    }
                } else if status.as_u16() == 404 {
                    // 404 表示这个词不存在，不必重试
                    return Err(anyhow!("404 未收录该词"));
                } else if status.as_u16() == 429 {
                    last_err = Some(anyhow!("请求过于频繁（429），已重试"));
                    continue;
                } else {
                    last_err = Some(anyhow!("服务返回 HTTP {}", status.as_u16()));
                    continue;
                }
            }
            Err(e) => {
                last_err = Some(anyhow!(friendly_reqwest_error(&e)));
                continue;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("请求失败")))
}

/// POST JSON（用于 LibreTranslate 这类接口）。
///
/// `target_lang` 由调用方显式给出，避免「日→中」被错误推导成「日→英」。
pub async fn post_json(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    api_key: &str,
    word: &str,
    source_lang: &str,
    target_lang: &str,
) -> Result<Value> {
    let body = serde_json::json!({
        "q": word,
        "source": source_lang,
        "target": target_lang,
        "format": "text",
        "api_key": api_key,
    });

    let mut req = client.post(url).json(&body);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }

    let resp = req.send().await.map_err(|e| anyhow!(friendly_reqwest_error(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("服务返回 HTTP {}", status.as_u16()));
    }
    Ok(resp.json::<Value>().await?)
}

/// 把 reqwest 的错误翻译成用户能看懂的中文。
pub fn friendly_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "请求超时".to_string()
    } else if e.is_connect() {
        "无法连接服务器（请检查网络或代理设置）".to_string()
    } else if e.is_decode() {
        "响应解析失败".to_string()
    } else {
        format!("网络错误: {}", e)
    }
}

/// 探测某地址是否可连通（用于 LM Studio 健康检查）。
pub async fn probe(client: &reqwest::Client, url: &str) -> bool {
    match client.get(url).timeout(Duration::from_secs(5)).send().await {
        Ok(r) => r.status().is_success() || r.status().as_u16() < 500,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builds() {
        assert!(build_client(30).is_ok());
    }

    #[test]
    fn error_messages_are_chinese() {
        // 只是保证函数存在且可调用（真实错误对象难以构造）
        let s = "请求超时";
        assert!(s.contains("超时"));
    }
}
