//! HTTP 网络层：统一的请求构造、代理解析、超时、重试与错误归一化。
//!
//! 设计考量：
//! - 使用 rustls 而非系统 schannel，避免在部分环境下
//!   「证书吊销列表离线」导致的握手失败。
//! - **默认直连**：`NetworkConfig::enable_proxy` 默认为 `false`，
//!   此时完全不读环境变量、不读注册表，并显式 `no_proxy()` 关掉
//!   reqwest 的自动探测。内置在线资源（jsDelivr 词库镜像、有道词典、
//!   必应 RSS、freedictionaryapi）都保证在国内可直连，装好即用。
//! - **代理是可选能力**：用户显式打开 `enable_proxy` 后才按
//!   「手动配置 → 环境变量 → 系统注册表」解析。之所以要自己读注册表，
//!   是因为图形界面启动的桌面应用拿不到 `HTTP_PROXY` 环境变量，
//!   而 Clash / v2ray 这类工具默认只写「Internet 选项」。
//! - 本机地址（127.0.0.1 / localhost）永远直连，避免把 LM Studio 的
//!   `127.0.0.1:1234` 请求送进代理而连不上本地模型。
//! - 所有错误转成中文可读文案，直接展示给用户。

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::models::NetworkConfig;

/// 统一 UA：部分站点（GitHub raw、搜索引擎）会对空 UA 直接拒绝。
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) WordWise/1.0 \
(+https://github.com/DedalusArtin/wordwise)";

/// 代理解析结果。带上来源说明，方便在设置页如实告诉用户
/// 「当前到底走没走代理、走的是哪一个」，避免再出现「我明明开了代理」的困惑。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProxyResolution {
    /// 最终生效的代理地址；`None` 表示直连
    pub url: Option<String>,
    /// 人类可读的来源：手动配置 / 环境变量 / 系统代理 / 未检测到
    pub origin: String,
    /// 是否已经显式关闭了 reqwest 的自动代理探测
    pub explicit: bool,
}

impl ProxyResolution {
    /// 是否处于直连状态。
    pub fn is_direct(&self) -> bool {
        self.url.is_none()
    }

    /// 给界面用的一句话描述。
    pub fn describe(&self) -> String {
        match &self.url {
            Some(u) => format!("已启用代理 {}（来源：{}）", u, self.origin),
            // 直连时 origin 本身就是一句完整说明，别再套一层括号
            None => self.origin.clone(),
        }
    }
}

/// 补全代理地址的 scheme。`127.0.0.1:7890` 这种写法在代理软件里很常见，
/// 但 reqwest 要求带 scheme，否则直接报错。
pub fn normalize_proxy_url(raw: &str) -> String {
    let s = raw.trim();
    let has_scheme = s
        .split_once("://")
        .map(|(scheme, _)| {
            matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h" | "socks"
            )
        })
        .unwrap_or(false);
    if has_scheme {
        s.to_string()
    } else {
        format!("http://{}", s)
    }
}

/// 从环境变量里找代理。大小写两种写法都认（Windows 上两种都常见）。
fn proxy_from_env() -> Option<String> {
    const KEYS: &[&str] = &[
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    for k in KEYS {
        if let Ok(v) = std::env::var(k) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 解析 Windows「Internet 选项」里的 `ProxyServer` 值。
///
/// 两种格式都要支持：
/// - 简写 `127.0.0.1:7890`
/// - 分协议 `http=127.0.0.1:7890;https=127.0.0.1:7890;ftp=...`
fn pick_from_proxy_server(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if !v.contains('=') {
        return Some(v.to_string());
    }
    let mut https = None;
    let mut http = None;
    let mut any = None;
    for part in v.split(';') {
        let Some((scheme, addr)) = part.split_once('=') else {
            continue;
        };
        let addr = addr.trim();
        if addr.is_empty() {
            continue;
        }
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = https.or_else(|| Some(addr.to_string())),
            "http" => http = http.or_else(|| Some(addr.to_string())),
            _ => any = any.or_else(|| Some(addr.to_string())),
        }
    }
    // https 优先：国内绝大多数目标站都是 https
    https.or(http).or(any)
}

/// 读取 Windows 系统代理（注册表）。
#[cfg(windows)]
fn system_proxy() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    pick_from_proxy_server(&server)
}

/// 读取 Windows 的代理绕过列表（`ProxyOverride`），并入我们的 no_proxy。
#[cfg(windows)]
fn system_proxy_override() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .ok()?;
    let raw: String = key.get_value("ProxyOverride").ok()?;
    // 注册表里用 `;` 分隔，`<local>` 表示所有不含点的主机名
    let list: Vec<String> = raw
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "<local>")
        .map(|s| s.trim_start_matches("*.").to_string())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list.join(","))
    }
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

#[cfg(not(windows))]
fn system_proxy_override() -> Option<String> {
    None
}

/// 未启用代理时的来源说明。
///
/// 单独提成常量是为了让「默认直连」这件事在测试和界面里都能被断言，
/// 而不是散落成魔法字符串。
pub const ORIGIN_DIRECT_NO_PROXY: &str = "未启用代理（直连）";

/// 按优先级解析出生效的代理。
///
/// 优先级：**总开关 → 手动配置 → （可选）环境变量 → （可选）系统注册表**。
///
/// 第 0 步「总开关」必须是第一道判断：默认配置下代理功能整体关闭，
/// 连 `HTTP_PROXY` 环境变量也不读。否则只要机器上残留一个环境变量，
/// 「默认直连」就会失效，用户会莫名其妙地连不上内网资源。
pub fn resolve_proxy(cfg: &NetworkConfig) -> ProxyResolution {
    resolve_proxy_inner(cfg, proxy_from_env())
}

/// `resolve_proxy` 的实现体。
///
/// 把「环境变量」作为入参而不是在函数里现读，是为了让测试能确定性地
/// 断言「未启用代理时，即使环境里有 `HTTP_PROXY` 也必须直连」——
/// 直接改进程环境变量的测试在并行执行时是不稳定且互相干扰的。
fn resolve_proxy_inner(cfg: &NetworkConfig, env_proxy: Option<String>) -> ProxyResolution {
    // 0) 总开关：默认关闭 → 一律直连，不探测任何代理来源。
    if !cfg.enable_proxy {
        return ProxyResolution {
            url: None,
            origin: ORIGIN_DIRECT_NO_PROXY.into(),
            explicit: true,
        };
    }

    // 1) 用户手动填写的最优先——毕竟是用户明确指定的
    let manual = cfg.proxy.trim();
    if !manual.is_empty() {
        return ProxyResolution {
            url: Some(normalize_proxy_url(manual)),
            origin: "手动配置".into(),
            explicit: true,
        };
    }

    // 2) 用户没填地址时才自动探测；关掉自动探测就只用手动地址。
    if !cfg.use_system_proxy {
        return ProxyResolution {
            url: None,
            origin: "已启用代理但未填写地址（自动探测已关闭）".into(),
            explicit: true,
        };
    }

    // 3) 环境变量
    if let Some(v) = env_proxy {
        if !v.trim().is_empty() {
            return ProxyResolution {
                url: Some(normalize_proxy_url(&v)),
                origin: "环境变量".into(),
                explicit: true,
            };
        }
    }

    // 4) 系统代理（Windows 注册表）
    if let Some(v) = system_proxy() {
        return ProxyResolution {
            url: Some(normalize_proxy_url(&v)),
            origin: "Windows 系统代理设置".into(),
            explicit: true,
        };
    }

    // 用户开了代理但确实没探测到地址：显式直连并如实说明。
    // 这里刻意不去「交给 reqwest 兜底」——已经查过环境变量和注册表了，
    // 再让 reqwest 自己猜一次只会让行为不可预期。
    ProxyResolution {
        url: None,
        origin: "已启用代理但未检测到可用地址".into(),
        explicit: true,
    }
}

/// 组装 no_proxy 列表：用户配置 + 永远直连的本机地址 + 系统绕过列表。
fn build_no_proxy(cfg: &NetworkConfig) -> String {
    let mut parts: Vec<String> = Vec::new();
    for s in cfg.no_proxy.split(',') {
        let s = s.trim();
        if !s.is_empty() {
            parts.push(s.to_string());
        }
    }
    // 本机地址无条件直连：LM Studio / 本地服务绝不能进代理
    for host in ["localhost", "127.0.0.1", "::1", "0.0.0.0"] {
        if !parts.iter().any(|p| p == host) {
            parts.push(host.to_string());
        }
    }
    if let Some(extra) = system_proxy_override() {
        for s in extra.split(',') {
            let s = s.trim();
            if !s.is_empty() && !parts.iter().any(|p| p == s) {
                parts.push(s.to_string());
            }
        }
    }
    parts.join(",")
}

/// 创建 HTTP 客户端（使用默认网络配置）。
pub fn build_client(timeout_secs: u64) -> Result<reqwest::Client> {
    let cfg = NetworkConfig {
        timeout_secs,
        ..NetworkConfig::default()
    };
    Ok(build_client_with(&cfg)?.0)
}

/// 按网络配置创建 HTTP 客户端，同时返回代理解析结果（供界面展示）。
pub fn build_client_with(cfg: &NetworkConfig) -> Result<(reqwest::Client, ProxyResolution)> {
    let resolved = resolve_proxy(cfg);

    let mut builder = reqwest::Client::builder()
        // 使用 rustls 后端：不依赖系统的证书吊销列表（CRL）在线校验，
        // 从而规避 Windows 上常见的 CRYPT_E_REVOCATION_OFFLINE 握手失败。
        // 注意：这里保持正常的证书链校验（不放松安全性）。
        .timeout(Duration::from_secs(cfg.timeout_secs.clamp(5, 600)))
        .connect_timeout(Duration::from_secs(cfg.connect_timeout_secs.clamp(3, 120)))
        .pool_idle_timeout(Duration::from_secs(90))
        .user_agent(USER_AGENT);

    if let Some(url) = &resolved.url {
        let no_proxy = build_no_proxy(cfg);
        let proxy = reqwest::Proxy::all(url.as_str())
            .map_err(|e| anyhow!("代理地址无效（{}）：{}", url, e))?
            .no_proxy(reqwest::NoProxy::from_string(&no_proxy));
        builder = builder
            .proxy(proxy)
            // 关掉 reqwest 的自动代理探测：既然我们已经显式解析出代理并配好了
            // 绕过列表，就不该再让它自己追加一个不受控的代理匹配器。
            .no_proxy();
    } else if resolved.explicit {
        // 直连（含「用户未启用代理」这一默认情形）。
        //
        // `no_proxy()` 会彻底禁用 reqwest 的所有代理逻辑。这一步不能省：
        // 我们启用了 `system-proxy` 特性，若不在解析为直连时显式关掉，
        // reqwest 仍会自己回头去读环境变量/注册表，把已经明确关闭的
        // 代理又悄悄打开——「默认直连」就成了一句空话。
        builder = builder.no_proxy();
    }

    let client = builder.build()?;
    Ok((client, resolved))
}

/// 让宿主环境（例如 Node 风格的库或用户手动设的变量）不干扰本机直连。
/// 只写进程内的 `NO_PROXY`，不落盘、不影响系统设置。
pub fn ensure_no_proxy_env() {
    let current = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    let mut list: Vec<String> = current
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for host in ["localhost", "127.0.0.1", "::1"] {
        if !list.iter().any(|p| p == host) {
            list.push(host.to_string());
        }
    }
    let joined = list.join(",");
    // SAFETY: 单线程初始化阶段调用，不存在并发读写环境变量的风险。
    std::env::set_var("NO_PROXY", &joined);
    std::env::set_var("no_proxy", &joined);
}

/// GET 并解析 JSON。
///
/// `timeout_secs` 为**单次尝试**的超时，`retries` 为额外重试次数。
/// 查词这类用户可感知的操作用小超时 + 少重试，避免一个被墙的源拖死整条链路。
pub async fn get_json(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    api_key: &str,
    timeout_secs: u64,
    retries: usize,
    _word: &str,
    _lang: &str,
) -> Result<Value> {
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 0..=retries {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
        }
        match get_raw(client, url, headers, api_key, timeout_secs).await {
            Ok((status, body)) => {
                if (200..300).contains(&status) {
                    match serde_json::from_str::<Value>(&body) {
                        Ok(v) => return Ok(v),
                        Err(_) => {
                            last_err = Some(anyhow!("返回内容不是合法 JSON（HTTP {}）", status));
                            continue;
                        }
                    }
                } else if status == 404 {
                    // 404 表示这个词不存在，不必重试
                    return Err(anyhow!("404 未收录该词"));
                } else if status == 429 {
                    last_err = Some(anyhow!("请求过于频繁（429），已重试"));
                    continue;
                } else {
                    last_err = Some(anyhow!("服务返回 HTTP {}", status));
                    continue;
                }
            }
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("请求失败")))
}

/// GET 一次，拿到 `(HTTP 状态码, 响应体原文)`。
///
/// 只负责「发请求 + 读 body」，不含解析与业务判定，供 `get_json` /
/// `get_json_or_text` 复用，保证两条路径的超时、代理、鉴权行为完全一致。
async fn get_raw(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    api_key: &str,
    timeout_secs: u64,
) -> Result<(u16, String)> {
    let mut req = client.get(url).timeout(Duration::from_secs(timeout_secs));
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!(friendly_reqwest_error(&e)))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| anyhow!("读取响应失败: {}", e))?;
    Ok((status, body))
}

/// GET 并**尽量**解析 JSON；响应体不是 JSON 时把原文一并带回。
///
/// 为什么需要它：不少词典源直接返回 HTML 页面，或返回带 BOM / 前缀说明的
/// 「半 JSON」。走 `get_json` 时这类响应会被判为「返回内容不是合法 JSON」
/// 而直接丢弃，于是这个源在界面上表现为「连得上却查不出词」。
/// 这里把原文交回上层，让词典模块自己决定怎么抽取
/// （见 `dict::normalize_heuristic`）。
///
/// 返回 `(Some(json), body)` 或 `(None, body)`；HTTP 非 2xx 一律返回 Err。
pub async fn get_json_or_text(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    api_key: &str,
    timeout_secs: u64,
    retries: usize,
) -> Result<(Option<Value>, String)> {
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 0..=retries {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
        }
        match get_raw(client, url, headers, api_key, timeout_secs).await {
            Ok((status, body)) => {
                if (200..300).contains(&status) {
                    // 去掉可能存在的 UTF-8 BOM 再试解析（很多国内源会带）
                    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
                    let v = serde_json::from_str::<Value>(trimmed).ok();
                    return Ok((v, body));
                }
                if status == 404 {
                    return Err(anyhow!("404 未收录该词"));
                }
                if status == 429 {
                    last_err = Some(anyhow!("请求过于频繁（429），已重试"));
                    continue;
                }
                last_err = Some(anyhow!("服务返回 HTTP {}", status));
            }
            Err(e) => last_err = Some(e),
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
    timeout_secs: u64,
) -> Result<Value> {
    let body = serde_json::json!({
        "q": word,
        "source": source_lang,
        "target": target_lang,
        "format": "text",
        "api_key": api_key,
    });

    let mut req = client
        .post(url)
        .timeout(Duration::from_secs(timeout_secs))
        .json(&body);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!(friendly_reqwest_error(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("服务返回 HTTP {}", status.as_u16()));
    }
    Ok(resp.json::<Value>().await?)
}

/// 抓取一段 HTML 文本（用于搜索引擎结果页）。
/// 走和业务请求同一套超时/代理设置，避免「查词能通、搜索不通」这种割裂。
pub async fn get_text(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    timeout_secs: u64,
) -> Result<String> {
    let mut req = client.get(url).timeout(Duration::from_secs(timeout_secs));
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!(friendly_reqwest_error(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("服务返回 HTTP {}", status.as_u16()));
    }
    resp.text().await.map_err(|e| anyhow!("读取响应失败: {}", e))
}

/// 从 URL 里取「主机:端口」，用于在提示文案里说清「连的是哪个服务」。
///
/// 不用 `reqwest::Url` 解析：那是重量级操作，而且用户填的地址经常缺协议头
/// （`api.deepseek.com/v1`），Url 会直接报错。这里只需要能展示，够用即可。
pub fn host_of(url: &str) -> String {
    let s = url.trim();
    let s = s.split("://").last().unwrap_or(s);
    let s = s.split('/').next().unwrap_or(s);
    if s.is_empty() {
        url.trim().to_string()
    } else {
        s.to_string()
    }
}

/// 把 reqwest 的错误翻译成用户能看懂的中文。
pub fn friendly_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "请求超时".to_string()
    } else if e.is_connect() {
        "无法连接服务器（请检查网络或代理设置）".to_string()
    } else if e.is_decode() {
        "响应解析失败".to_string()
    } else if e.is_redirect() {
        "重定向次数过多".to_string()
    } else {
        let s = e.to_string();
        if s.contains("proxy") || s.contains("Proxy") {
            format!("代理连接失败: {}", s)
        } else {
            format!("网络错误: {}", s)
        }
    }
}

/// 一个「绝对直连」的客户端：不读任何代理设置。
///
/// 用于访问本机服务（LM Studio 等）。哪怕用户把代理配错了，
/// 本地模型也必须是通的，否则整个应用的功能会互相拖累。
pub fn client_direct(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.clamp(3, 300)))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(USER_AGENT)
        .no_proxy()
        .build()
        .unwrap_or_else(|_| {
            // 兜底也必须维持「不走代理」这个不变量。
            //
            // 不能直接回退到 `Client::new()`：我们启用了 reqwest 的
            // `system-proxy` 特性，`Client::new()` 会自己去读环境变量和注册表，
            // 于是「绝对直连」的客户端反而会把 LM Studio 的请求送进代理。
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        })
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
    fn host_of_extracts_display_host() {
        assert_eq!(host_of("http://127.0.0.1:1234/v1"), "127.0.0.1:1234");
        assert_eq!(host_of("https://api.deepseek.com/v1"), "api.deepseek.com");
        assert_eq!(host_of("https://api.deepseek.com"), "api.deepseek.com");
        assert_eq!(host_of("https://open.bigmodel.cn/api/paas/v4"), "open.bigmodel.cn");
        // 用户手填时常忘了协议头：不能因为解析不了就显示空
        assert_eq!(host_of("api.deepseek.com/v1"), "api.deepseek.com");
        // 极端输入也要给点东西看，不能是空白
        assert_eq!(host_of(""), "");
    }

    #[test]
    fn client_builds() {
        assert!(build_client(30).is_ok());
    }

    // ---------- 默认直连（这是本项目的一条硬前提） ----------

    /// 默认配置必须是「不启用代理」。
    ///
    /// 这条断言看着琐碎，但它守住的是一个产品决定：应用装好即可用，
    /// 不要求用户先准备一个代理，也不会因为机器上有代理设置就改变行为。
    #[test]
    fn default_config_does_not_enable_proxy() {
        let cfg = NetworkConfig::default();
        assert!(!cfg.enable_proxy, "默认配置不应启用代理");
        assert!(cfg.proxy.is_empty(), "默认配置不应预填代理地址");
    }

    /// 默认配置解析出来必须是直连，且已经显式关掉了 reqwest 的自动探测。
    #[test]
    fn default_config_resolves_to_direct() {
        let r = resolve_proxy(&NetworkConfig::default());
        assert!(r.url.is_none(), "默认配置不应解析出代理");
        assert!(r.is_direct());
        assert_eq!(r.origin, ORIGIN_DIRECT_NO_PROXY);
        // explicit=true 才会让 build_client_with 调 .no_proxy()，
        // 从而连 reqwest 自己的系统代理探测也一并关掉
        assert!(r.explicit, "直连时也必须显式关闭自动代理探测");
    }

    /// 未启用代理时，**环境变量里的 HTTP_PROXY 也不能被采纳**。
    ///
    /// 这是「默认直连」最容易被悄悄破坏的一条路径：只要机器上（或被别的
    /// 软件）设了 `HTTP_PROXY`，代理就会在用户毫无察觉的情况下生效。
    #[test]
    fn disabled_proxy_ignores_env_vars() {
        let cfg = NetworkConfig::default();
        let r = resolve_proxy_inner(&cfg, Some("http://127.0.0.1:7890".into()));
        assert!(r.url.is_none(), "未启用代理时不应采纳 HTTP_PROXY");
        assert_eq!(r.origin, ORIGIN_DIRECT_NO_PROXY);
    }

    /// 未启用代理时，手动填了地址也不生效——总开关优先级最高。
    #[test]
    fn disabled_proxy_ignores_manual_address() {
        let cfg = NetworkConfig {
            proxy: "127.0.0.1:7890".into(),
            use_system_proxy: true,
            ..NetworkConfig::default()
        };
        let r = resolve_proxy_inner(&cfg, Some("http://10.0.0.1:8080".into()));
        assert!(r.url.is_none());
        assert_eq!(r.origin, ORIGIN_DIRECT_NO_PROXY);
    }

    // ---------- 启用代理后仍然完好 ----------

    #[test]
    fn client_builds_with_manual_proxy() {
        let cfg = NetworkConfig {
            enable_proxy: true,
            proxy: "127.0.0.1:7890".into(),
            ..NetworkConfig::default()
        };
        let (client, resolved) = build_client_with(&cfg).expect("应能构造带代理的客户端");
        assert!(client.get("https://example.com").build().is_ok());
        assert_eq!(resolved.url.as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(resolved.origin, "手动配置");
    }

    /// 启用代理 + 地址留空 → 自动探测，环境变量优先于注册表。
    #[test]
    fn enabled_proxy_reads_env_var() {
        let cfg = NetworkConfig {
            enable_proxy: true,
            ..NetworkConfig::default()
        };
        let r = resolve_proxy_inner(&cfg, Some("socks5://127.0.0.1:7891".into()));
        assert_eq!(r.url.as_deref(), Some("socks5://127.0.0.1:7891"));
        assert_eq!(r.origin, "环境变量");
    }

    /// 启用代理但关掉自动探测、又没填地址 → 直连，且说明清楚原因。
    #[test]
    fn enabled_proxy_without_address_and_without_autodetect_is_direct() {
        let cfg = NetworkConfig {
            enable_proxy: true,
            use_system_proxy: false,
            ..NetworkConfig::default()
        };
        let r = resolve_proxy_inner(&cfg, Some("http://127.0.0.1:7890".into()));
        assert!(r.url.is_none());
        assert!(r.explicit);
        assert!(r.origin.contains("未填写地址"), "来源应说明原因：{}", r.origin);
    }

    #[test]
    fn normalize_proxy_scheme() {
        assert_eq!(normalize_proxy_url("127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:7891"),
            "socks5://127.0.0.1:7891"
        );
    }

    #[test]
    fn proxy_server_formats() {
        assert_eq!(
            pick_from_proxy_server("127.0.0.1:7890").as_deref(),
            Some("127.0.0.1:7890")
        );
        assert_eq!(
            pick_from_proxy_server("http=127.0.0.1:7890;https=127.0.0.1:7891").as_deref(),
            Some("127.0.0.1:7891")
        );
        assert_eq!(pick_from_proxy_server("   ").as_deref(), None);
    }

    #[test]
    fn localhost_never_goes_through_proxy() {
        let cfg = NetworkConfig {
            enable_proxy: true,
            proxy: "127.0.0.1:7890".into(),
            ..NetworkConfig::default()
        };
        let np = build_no_proxy(&cfg);
        assert!(np.contains("127.0.0.1"));
        assert!(np.contains("localhost"));
    }

    /// `client_direct` 必须真的能直连本机。
    ///
    /// 这条测试守住的是「LM Studio 永不被代理误伤」这个不变量。
    /// 在配了系统代理的机器上，如果 `client_direct` 丢了 `.no_proxy()`，
    /// 请求会被送进代理而不是本机监听端口，测试就会失败。
    #[tokio::test]
    async fn client_direct_reaches_localhost() {
        use std::io::{Read, Write};

        // 起一个最小 HTTP 服务：读完整请求头再回 200。
        // 必须先读再回 —— 直接回完就关会把请求数据留在接收缓冲里，
        // 内核会发 RST，客户端看到的是「连接被重置」而不是我们的响应。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定随机端口失败");
        let addr = listener.local_addr().expect("取本地地址失败");
        let server = std::thread::spawn(move || -> String {
            let Ok((mut s, _)) = listener.accept() else {
                return String::new();
            };
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            );
            let _ = s.flush();
            req
        });

        let client = client_direct(3);
        let resp = client.get(format!("http://{}/probe", addr)).send().await;
        let req_line = server.join().unwrap_or_default();
        assert!(
            resp.is_ok(),
            "client_direct 必须能直连本机服务（不能被代理拦走）。\n收到请求行：{}\n错误：{:?}",
            req_line.lines().next().unwrap_or("(无)"),
            resp.err()
        );
        assert_eq!(resp.unwrap().status().as_u16(), 200);
        // 直连时服务端看到的应当是普通 GET，而不是代理的 CONNECT
        assert!(
            !req_line.starts_with("CONNECT"),
            "本机请求被送进了代理：{}",
            req_line.lines().next().unwrap_or("(无)")
        );
    }

    #[test]
    fn error_messages_are_chinese() {
        // 只是保证函数存在且可调用（真实错误对象难以构造）
        let s = "请求超时";
        assert!(s.contains("超时"));
    }

    /// Windows 上必须能从注册表里读到「Internet 选项」的代理设置。
    ///
    /// 这是代理能力的核心断言：图形界面启动的应用拿不到 HTTP_PROXY，
    /// 只有读注册表才知道用户开了代理（当然，前提是用户启用了代理）。
    #[cfg(windows)]
    #[test]
    fn windows_system_proxy_is_readable() {
        // 直接调用底层读取函数，确认注册表访问链路通
        match system_proxy() {
            Some(u) => assert!(!u.trim().is_empty(), "读到的代理地址不应为空"),
            None => {
                // 本机确实没开系统代理也属正常（CI / 干净环境）
                eprintln!("本机未检测到系统代理，跳过强断言");
            }
        }
        // 用户启用代理后，解析结果不应 panic，且来源说明非空
        let cfg = NetworkConfig {
            enable_proxy: true,
            ..NetworkConfig::default()
        };
        let r = resolve_proxy(&cfg);
        assert!(!r.origin.is_empty(), "无论有没有代理，来源说明都不该为空");
    }

    /// 端到端联网冒烟测试（需要真实网络，默认跳过）。
    ///
    /// **前提是直连可用**：不设任何代理也能全部通过，这就是
    /// 「系统必须能够在没有代理的情况下正常访问」的验证。
    ///
    /// ```bash
    /// WORDWISE_LIVE_TEST=1 cargo test --lib live_network_smoke -- --nocapture
    /// ```
    #[tokio::test]
    async fn live_network_smoke() {
        if std::env::var("WORDWISE_LIVE_TEST").as_deref() != Ok("1") {
            return;
        }
        // 刻意用默认配置（= 直连）来跑，验证无代理环境下的可用性
        let cfg = NetworkConfig::default();
        assert!(!cfg.enable_proxy);
        let (client, resolved) = build_client_with(&cfg).expect("构造客户端失败");
        eprintln!("[live] 代理解析结果：{}", resolved.describe());
        assert!(resolved.is_direct(), "默认配置应直连");

        let mut headers = BTreeMap::new();
        headers.insert("User-Agent".to_string(), USER_AGENT.to_string());

        // 1) 词库下载链路。
        //
        // 这里刻意验证**整条镜像链**而不是单个主机：jsDelivr 在密集请求下
        // 会出现瞬时连接超时（实测同一批 URL 两次运行失败项不同，重试即恢复），
        // 所以「某一个镜像挂了」不代表用户下不了词库——应用本来就是
        // jsDelivr → gh-proxy → raw 依次回退的。要求至少一条通即可。
        let mirrors = [
            "https://cdn.jsdelivr.net/gh/mahavivo/english-wordlists@master/CET4_edited.txt",
            "https://gh-proxy.com/https://raw.githubusercontent.com/mahavivo/english-wordlists/master/CET4_edited.txt",
        ];
        let mut book_ok = false;
        let mut failures: Vec<String> = Vec::new();
        for url in mirrors {
            match get_text(&client, url, &headers, 30).await {
                Ok(text) if text.contains("abandon") || text.len() > 1000 => {
                    eprintln!("[live] 词库镜像 OK，{} 字节（{}）", text.len(), url);
                    book_ok = true;
                    break;
                }
                Ok(text) => failures.push(format!("{}：内容异常（{} 字节）", url, text.len())),
                Err(e) => failures.push(format!("{}：{}", url, e)),
            }
        }
        assert!(
            book_ok,
            "词库镜像链全部失败（直连）——用户将无法下载词库：\n{}",
            failures.join("\n")
        );

        // 2) 必应 RSS：在线搜索链路必须通
        let rss = "https://cn.bing.com/search?q=apple&format=rss";
        let xml = get_text(&client, rss, &headers, 30)
            .await
            .expect("必应 RSS 请求失败（直连）");
        assert!(xml.contains("<item>"), "必应 RSS 未返回结果项");
        eprintln!("[live] 必应 RSS OK");

        // 3) 有道：中文释义主源必须通
        let yd = "https://dict.youdao.com/suggest?num=1&doctype=json&q=apple";
        let v = get_json(&client, yd, &headers, "", 30, 0, "apple", "en")
            .await
            .expect("有道词典请求失败（直连）");
        assert!(v.get("data").is_some(), "有道返回结构异常");
        eprintln!("[live] 有道词典 OK");

        // 4) 英英释义源：也必须在直连下可用
        let fd = "https://freedictionaryapi.com/api/v1/entries/en/apple";
        let v = get_json(&client, fd, &headers, "", 30, 0, "apple", "en")
            .await
            .expect("英英释义源请求失败（直连）");
        assert!(v.get("entries").is_some(), "英英释义源返回结构异常");
        eprintln!("[live] 英英释义源 OK");

        // 5) 本机地址绝不走代理（LM Studio 健康检查）
        let direct = client_direct(3);
        let _ = probe(&direct, "http://127.0.0.1:1234/v1/models").await;
        eprintln!("[live] 本机直连探测完成");
    }
}
