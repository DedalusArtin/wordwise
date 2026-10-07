//! 在线更新：版本检测 → 下载安装包 → 启动安装程序。
//!
//! 为什么不用 `tauri-plugin-updater`：
//! 那个插件要求我们自己维护一套签名密钥和 `latest.json` 静态托管，
//! 而本项目的发布渠道就是 GitHub Releases —— 元数据（版本号、说明、
//! 资产地址）本来就现成地放在 Releases API 里。自己读一遍 API 反而更简单，
//! 也少一处「忘了上传 latest.json 就更新不了」的隐性依赖。
//!
//! 三层职责分得很清：
//!   1. [`check_inner`] —— 只读 GitHub，产出一份可展示的 [`UpdateInfo`]；
//!   2. [`cmd_download_update`] —— 把选中的资产下到 `数据目录/updates/`；
//!   3. [`cmd_run_update`] —— 启动安装程序。**这一步有路径白名单**，
//!      因为「执行用户指定的任意路径」等于给前端开了个任意程序执行入口。

use crate::localllm::{self, Progress};
use crate::models::AppConfig;
use crate::net;
use crate::state::AppState;
use crate::timeutil;
use serde::Serialize;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 项目仓库（`owner/name`）。
pub const REPO: &str = "DedalusArtin/wordwise";

/// 更新下载进度事件。
///
/// 刻意与本地模型部署的 `local-llm://progress` 分开：两者会同时出现在
/// 设置页的不同面板里，共用一条事件会让两个进度条互相覆盖。
pub const EVT_UPDATE_PROGRESS: &str = "update://progress";

/// Releases 资产下载的加速镜像（国内直连 `github.com` 经常不通）。
///
/// 用法是把原始 URL 直接拼在镜像域名后面，与 `localllm::ENGINE_MIRRORS`
/// 同一套约定。直连永远排第一：镜像只是兜底，能用直连就别绕第三方。
const DOWNLOAD_MIRRORS: [&str; 3] = [
    "https://gh-proxy.com/",
    "https://ghfast.top/",
    "https://ghproxy.net/",
];

/// 检查更新（GitHub API）的候选地址：直连优先，失败再依次试镜像。
///
/// 为什么要给「检查更新」也配镜像：安装包下载本来就有三条镜像兜底
/// （见 [`download_urls`]），而检查更新原先只打 `api.github.com` —— 于是
/// 出现一种很别扭的不对称：「检查更新」告诉你 GitHub 不可达，可它其实
/// **下载得了**。用户拿到的结论就是「GitHub 不可达」，进而误判整个联网
/// 功能都废了。这里复用与下载**同一批**镜像（同一个常量，不另立一份），
/// 保证两边的可达性判断一致。
fn check_urls(original: &str) -> Vec<String> {
    let mut urls = vec![original.to_string()];
    for m in DOWNLOAD_MIRRORS {
        urls.push(format!("{m}{original}"));
    }
    urls
}

/// 结果里标注数据来源；走镜像时如实写出来，免得用户以为「直连也通了」。
fn source_label(base: &str, via_mirror: bool) -> String {
    if via_mirror {
        format!("{base}·经国内镜像")
    } else {
        base.to_string()
    }
}

/// 就绪的安装包放在数据目录下的这个子目录里。
fn updates_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("updates")
}

// ============================================================
// 版本号比较
// ============================================================

/// 把 `v0.41.0` / `0.41.0-beta.2` / `0.41` 拆成（数字段, 是否预发布）。
///
/// 剥掉 `v` 前缀是因为 git tag 与 `Cargo.toml` 里的写法不一致：
/// tag 习惯带 `v`，而 `env!("CARGO_PKG_VERSION")` 一定不带。
pub fn split_version(s: &str) -> (Vec<u64>, bool) {
    let t = s.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match t.find(['-', '+']) {
        Some(i) => (&t[..i], true),
        None => (t, false),
    };
    let nums = core
        .split('.')
        // 无数字段（比如 tag 就叫 `nightly`）按 0 处理，不 panic。
        // 用整数字段而不是字符串：`0.9.0` < `0.10.0`，按字符串比会反过来。
        .map(|p| p.trim().parse::<u64>().unwrap_or(0))
        .collect();
    (nums, pre)
}

/// 语义化比较：`Greater` 表示 `a` 比 `b` 新。
///
/// 规则：逐段比数字 → 段数不足按 0 补（`0.41` == `0.41.0`）→
/// 主版本相同时**正式版大于预发布版**（`0.42.0` > `0.42.0-rc.1`）。
pub fn cmp_version(a: &str, b: &str) -> Ordering {
    let (na, pa) = split_version(a);
    let (nb, pb) = split_version(b);
    for i in 0..na.len().max(nb.len()) {
        let x = na.get(i).copied().unwrap_or(0);
        let y = nb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (pa, pb) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

// ============================================================
// 数据结构
// ============================================================

/// 一个可下载的发布资产。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub size_text: String,
    /// `installer` / `portable` / `other`
    pub kind: String,
    /// 能否一键安装（只有 Windows 安装程序可以）
    pub installable: bool,
    /// GitHub 在响应里声明的摘要（`sha256:...`），没有就是空串
    pub digest: String,
}

/// 一次检查的完整结果，前端不必再自己拼。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub repo: String,
    pub latest: String,
    pub tag: String,
    pub has_update: bool,
    pub prerelease: bool,
    pub name: String,
    pub notes: String,
    pub published_at: String,
    pub page_url: String,
    /// 自动挑选出来、准备下载的那个资产
    pub asset: Option<UpdateAsset>,
    pub assets: Vec<UpdateAsset>,
    pub checked_at: i64,
    /// 用户把这一版加进了「跳过」名单
    pub skipped: bool,
    /// 需要额外说明的话（预发布 / 跳过 / 没有可安装资产）
    pub note: Option<String>,
    /// 数据来源说明，直接显示给用户看
    pub source: String,
}

/// 从资产文件名判断类型，并给出「能不能一键装」。
fn asset_kind(name: &str) -> (&'static str, bool) {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".exe") {
        ("installer", true)
    } else if lower.ends_with(".zip") {
        ("portable", false)
    } else {
        ("other", false)
    }
}

/// 挑一个默认要下载的资产。
///
/// 优先级：带 `setup` 的 exe（安装程序，能自动装）→ 任意 exe → zip 便携包。
/// 之所以要排序而不是「取第一个」：Releases 里的资产顺序不稳定，
/// 而便携包和安装包混在一起时，取错了用户就要手动解压。
fn pick_asset(assets: &[UpdateAsset]) -> Option<UpdateAsset> {
    let by = |pred: &dyn Fn(&UpdateAsset) -> bool| assets.iter().find(|a| pred(a)).cloned();
    by(&|a| a.installable && a.name.to_ascii_lowercase().contains("setup"))
        .or_else(|| by(&|a| a.installable))
        .or_else(|| by(&|a| a.kind == "portable"))
}

/// 把 GitHub 的资产 JSON 数组转成我们的结构。
fn parse_assets(v: &serde_json::Value) -> Vec<UpdateAsset> {
    let Some(arr) = v.get("assets").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|a| {
            let name = a.get("name").and_then(|x| x.as_str())?.to_string();
            let url = a.get("browser_download_url").and_then(|x| x.as_str())?.to_string();
            if url.is_empty() {
                return None;
            }
            let size = a.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
            let (kind, installable) = asset_kind(&name);
            Some(UpdateAsset {
                name,
                url,
                size,
                size_text: localllm::human_bytes(size),
                kind: kind.to_string(),
                installable,
                digest: a
                    .get("digest")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

/// 把一次 API 响应整理成 [`UpdateInfo`]。
fn build_info(v: &serde_json::Value, current: &str, cfg: &AppConfig, source: &str) -> UpdateInfo {
    let tag = v
        .get("tag_name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let latest = tag.trim_start_matches(['v', 'V']).to_string();
    let prerelease = v.get("prerelease").and_then(|x| x.as_bool()).unwrap_or(false);

    let skip = cfg
        .skip_update_version
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let skipped = !skip.is_empty() && cmp_version(&latest, &skip) == Ordering::Equal;

    let has_update = !latest.is_empty()
        && cmp_version(&latest, current) == Ordering::Greater
        // 「跳过此版本」只压掉提示，不隐藏信息：latest 字段照样带回去，
        // 用户想装随时能在发布页找到。
        && !skipped;

    let assets = parse_assets(v);
    let asset = pick_asset(&assets);

    // GitHub 的 body 是 Markdown，偶尔会有几万字的 changelog。
    // 超长的直接截断：前端渲染几万字会明显卡顿，而用户也不会真去读。
    let raw_notes = v.get("body").and_then(|x| x.as_str()).unwrap_or("");
    let notes = if raw_notes.chars().count() > 8000 {
        let cut: String = raw_notes.chars().take(8000).collect();
        format!("{cut}\n\n…（说明过长，已截断，完整内容见发布页）")
    } else {
        raw_notes.to_string()
    };

    let note = if skipped {
        Some(format!("你已把 {tag} 加入跳过名单，不再提示这一个版本"))
    } else if prerelease {
        Some("这一版被标记为预发布（pre-release），可能不稳定，建议先看发布说明".to_string())
    } else if has_update && asset.is_none() {
        Some("新版本里没有找到可自动安装的安装包，请到发布页手动下载".to_string())
    } else {
        None
    };

    UpdateInfo {
        current: current.to_string(),
        repo: REPO.to_string(),
        latest,
        tag,
        has_update,
        prerelease,
        name: v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        notes,
        published_at: v
            .get("published_at")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            // 只要日期部分：`2026-10-06T01:20:33Z` → `2026-10-06`
            .split('T')
            .next()
            .unwrap_or("")
            .to_string(),
        page_url: v
            .get("html_url")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        asset,
        assets,
        checked_at: timeutil::now_ts(),
        skipped,
        note,
        source: source.to_string(),
    }
}

// ============================================================
// 网络
// ============================================================

/// 单独 GET 一个 GitHub API 地址。
///
/// 不走 `net::get_json`：那个助手是给词典源用的（带重试、带源配置），
/// 语义不匹配。这里只要「一次请求 + 把错误说人话」。
///
/// 返回 `Err("NOT_FOUND")` 是一个**约定信号**：调用方据此决定要不要
/// 退回 `releases` 列表接口。真实的网络错误原样往上抛。
async fn fetch_release(
    client: &reqwest::Client,
    url: &str,
    timeout_secs: u64,
) -> Result<serde_json::Value, String> {
    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        // 覆盖客户端默认超时：更新检查不该让用户对着转圈等 30 秒
        .timeout(std::time::Duration::from_secs(timeout_secs.clamp(5, 60)))
        .send()
        .await
        .map_err(|e| format!("连接 GitHub 失败：{}", net::friendly_reqwest_error(&e)))?;

    let status = resp.status();
    if status.as_u16() == 404 {
        return Err("NOT_FOUND".to_string());
    }
    if !status.is_success() {
        // 403 基本只有一种原因：这台机器/这个网络被 GitHub 限流或拦截。
        // 说清楚比丢一个裸状态码有用得多。
        let hint = if status.as_u16() == 403 {
            "（可能是请求过于频繁或当前网络被拦截）"
        } else {
            ""
        };
        return Err(format!("GitHub 返回 HTTP {}{hint}", status.as_u16()));
    }

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("解析 GitHub 响应失败：{e}"))
}

/// 依次尝试「直连 → 各镜像」拉一个 GitHub API。
///
/// 返回值里的 `bool` 表示**是否走了镜像**，用于在结果里如实标注来源。
///
/// `NOT_FOUND` 是一个语义信号（仓库里没有这个资源），换镜像也改变不了它，
/// 所以立刻上抛，不做无谓的重复请求。
async fn fetch_release_any(
    client: &reqwest::Client,
    urls: &[String],
    timeout_secs: u64,
) -> Result<(serde_json::Value, bool), String> {
    let mut last_err = String::from("未知原因");
    for (i, url) in urls.iter().enumerate() {
        match fetch_release(client, url, timeout_secs).await {
            Ok(v) => return Ok((v, i > 0)),
            Err(e) if e == "NOT_FOUND" => return Err(e),
            Err(e) => last_err = e,
        }
    }
    // ★ 这条尾巴很重要：检查更新失败**不等于**下载不可用。安装包下载走的是
    //   另一条独立链路（资产地址 + 自己的镜像回退），用户不该因为这里红一次
    //   就以为整个更新功能都废了。
    Err(format!(
        "{last_err}；已依次尝试直连与 {} 个镜像。注意：检查更新失败不代表下载不可用，安装包下载走的是另一条独立链路，通常仍可成功。",
        urls.len().saturating_sub(1)
    ))
}

/// 检查更新的实际实现（命令层只是它的壳，`cmd_download_update` 也要复用）。
async fn check_inner(state: &AppState) -> Result<UpdateInfo, String> {
    let cfg = state.cfg();
    let client = state.http();
    let timeout = cfg.network.timeout_secs;
    let current = env!("CARGO_PKG_VERSION").to_string();

    let latest_url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let list_url = format!("https://api.github.com/repos/{REPO}/releases");

    match fetch_release_any(&client, &check_urls(&latest_url), timeout).await {
        Ok((v, via_mirror)) => Ok(build_info(
            &v,
            &current,
            &cfg,
            &source_label("GitHub Releases（最新正式版）", via_mirror),
        )),
        // 仓库只有预发布版本时 `/releases/latest` 会 404。
        // 退一步用列表接口取第一个非草稿，这样开发期也能测到更新链路。
        Err(e) if e == "NOT_FOUND" => {
            let (list, via_mirror) =
                fetch_release_any(&client, &check_urls(&list_url), timeout).await?;
            let first = list
                .as_array()
                .and_then(|a| {
                    a.iter().find(|r| {
                        !r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false)
                    })
                })
                .cloned()
                .ok_or_else(|| format!("{REPO} 里还没有任何 Release"))?;
            Ok(build_info(
                &first,
                &current,
                &cfg,
                &source_label("GitHub Releases（最近一次发布）", via_mirror),
            ))
        }
        Err(e) => Err(e),
    }
}

/// 检查是否有新版本。
#[tauri::command]
pub async fn cmd_check_update(state: State<'_, Arc<AppState>>) -> Result<UpdateInfo, String> {
    check_inner(&state).await
}

// ============================================================
// 下载
// ============================================================

/// 生成「直连优先 + 镜像兜底」的候选地址列表。
fn download_urls(original: &str) -> Vec<String> {
    let mut urls = vec![original.to_string()];
    for m in DOWNLOAD_MIRRORS {
        urls.push(format!("{m}{original}"));
    }
    urls
}

/// 取消下载用的标志。
///
/// 与 `commands::localllm::cancel_flag` 同构但**刻意不复用**：两边的下载
/// 可以同时进行（用户一边下模型一边点更新），共用一个标志会让其中一个
/// 莫名其妙地被取消掉。
fn cancel_flag() -> Arc<AtomicBool> {
    static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
}

/// 取消正在进行的更新下载。
#[tauri::command(async)]
pub fn cmd_update_cancel() {
    cancel_flag().store(true, AtomicOrdering::Relaxed);
}

/// 计算文件的 SHA256（返回小写十六进制）。
fn sha256_file(p: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut hasher = Sha256::new();
    // 1 MB 一块：安装包不到 20 MB，但用固定缓冲避免把整个文件读进内存
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 下载更新安装包到 `数据目录/updates/`。
#[tauri::command]
pub async fn cmd_download_update(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let st = state.inner().clone();

    // ★ 重新自己解析一次，**不信任前端传来的 URL**。
    //   否则前端一旦被注入，这个命令就变成「下载任意地址并执行」。
    let info = check_inner(&st).await?;
    let asset = info
        .asset
        .clone()
        .ok_or_else(|| "新版本里没有可下载的安装包，请到发布页手动下载".to_string())?;
    if !asset.installable {
        return Err(format!(
            "{} 是便携包，不能自动安装。请到发布页下载后手动解压",
            asset.name
        ));
    }

    let dir = updates_dir(&st.data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let dest = dir.join(&asset.name);

    // 之前已经下好同一个文件就直接复用，别让用户白等一遍
    if dest.is_file() {
        let have = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if asset.size > 0 && have == asset.size {
            let sha = sha256_file(&dest).unwrap_or_default();
            return Ok(serde_json::json!({
                "ok": true,
                "already": true,
                "path": dest.display().to_string(),
                "size": have,
                "size_text": localllm::human_bytes(have),
                "sha256": sha,
                "version": info.latest,
                "message": format!("{} 已经下载过，可直接安装", asset.name),
            }));
        }
    }

    cancel_flag().store(false, AtomicOrdering::Relaxed);
    let _ = app.emit(
        EVT_UPDATE_PROGRESS,
        Progress::stage("update", "download", &format!("准备下载 {}…", asset.name)),
    );

    let urls = download_urls(&asset.url);
    let client = st.http();
    let app2 = app.clone();
    localllm::download_with_progress(&client, &urls, &dest, "update", cancel_flag(), move |p| {
        let _ = app2.emit(EVT_UPDATE_PROGRESS, p);
    })
    .await
    .map_err(err)?;

    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let sha = sha256_file(&dest).unwrap_or_default();

    // 尺寸对不上说明下到了一份残缺的文件。宁可在这里失败，
    // 也不要让用户去执行一个装到一半就报错的安装程序。
    if asset.size > 0 && size != asset.size {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "下载不完整（{} / {}），已删除，请重试",
            localllm::human_bytes(size),
            asset.size_text
        ));
    }
    if !asset.digest.is_empty() {
        let want = asset.digest.trim_start_matches("sha256:").to_ascii_lowercase();
        if !sha.is_empty() && want != sha {
            let _ = std::fs::remove_file(&dest);
            return Err("安装包校验失败（摘要与 GitHub 声明不一致），已删除，请重试".to_string());
        }
    }

    let _ = app.emit(
        EVT_UPDATE_PROGRESS,
        Progress::stage("update", "done", &format!("{} 下载完成", asset.name)),
    );

    Ok(serde_json::json!({
        "ok": true,
        "already": false,
        "path": dest.display().to_string(),
        "size": size,
        "size_text": localllm::human_bytes(size),
        "sha256": sha,
        "version": info.latest,
        "message": format!("{} 已下载（{}）", asset.name, localllm::human_bytes(size)),
    }))
}

// ============================================================
// 安装
// ============================================================

/// 在系统文件管理器里打开一个目录。
fn open_dir(p: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(p.as_os_str())
            .spawn()
            .map_err(|e| format!("打开文件夹失败：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(p.as_os_str())
            .spawn()
            .map_err(|e| format!("打开文件夹失败：{e}"))?;
    }
    // ★ 显式排除 android：见 `commands::maint::cmd_open_dir` 里同样的说明。
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        std::process::Command::new("xdg-open")
            .arg(p.as_os_str())
            .spawn()
            .map_err(|e| format!("打开文件夹失败：{e}"))?;
    }
    #[cfg(target_os = "android")]
    {
        return Err("移动端没有系统文件管理器入口".to_string());
    }
    Ok(())
}

/// 打开更新包的存放目录。
#[tauri::command(async)]
pub fn cmd_open_update_dir(
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    let dir = updates_dir(&state.data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    open_dir(&dir)?;
    Ok(serde_json::json!({ "ok": true, "path": dir.display().to_string() }))
}

/// 启动下载好的安装程序，并让 WordWise 自己退出。
///
/// ★ 路径白名单是这个命令存在的**前提**：它接收一个来自前端的路径并执行。
///   少了下面这层校验，任何能调用它的代码都能在本机跑任意程序。
#[tauri::command(async)]
pub fn cmd_run_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<serde_json::Value, String> {
    let dir = updates_dir(&state.data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let canon_dir = dir
        .canonicalize()
        .map_err(|e| format!("更新目录不可用：{e}"))?;

    let canon = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| format!("安装包不存在或已被移动：{e}"))?;

    // canonicalize 会解析 `..` 与符号链接，所以这里的前缀判断是可靠的
    if !canon.starts_with(&canon_dir) {
        return Err("只允许运行更新目录里的安装包".to_string());
    }
    let is_exe = canon
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| x.eq_ignore_ascii_case("exe"))
        .unwrap_or(false);
    if !is_exe {
        return Err("这个文件不是可执行的安装程序".to_string());
    }

    std::process::Command::new(&canon)
        .spawn()
        .map_err(|e| format!("启动安装程序失败（可能是权限不足）：{e}"))?;

    // 安装程序要覆盖正在运行的 wordwise.exe，Windows 下文件被占用会失败。
    // 给安装包 1.2 秒完成自解压后再退出，用户看到的是「点了一下就自动装」。
    let h = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        h.exit(0);
    });

    Ok(serde_json::json!({
        "ok": true,
        "path": canon.display().to_string(),
        "message": "安装程序已启动，WordWise 即将退出",
    }))
}

/// 保存「启动时检查更新 / 跳过此版本」两个偏好。
///
/// 单独一个命令而不是走整份 `cmd_save_config`：跳过版本是点一下就生效的
/// 轻量动作，走整份保存会把设置页里其它未提交的编辑一并写下去。
#[tauri::command(async)]
pub fn cmd_set_update_prefs(
    state: State<'_, Arc<AppState>>,
    check_on_start: Option<bool>,
    skip_version: Option<String>,
) -> Result<serde_json::Value, String> {
    let skip = skip_version.map(|v| v.trim().to_string());
    state
        .update_config(|c| {
            if let Some(b) = check_on_start {
                c.check_update_on_start = b;
            }
            if let Some(s) = &skip {
                // 空串的语义是「取消跳过」，而不是「跳过空版本号」
                c.skip_update_version = s.clone();
            }
        })
        .map_err(err)?;

    let cfg = state.cfg();
    Ok(serde_json::json!({
        "ok": true,
        "check_on_start": cfg.check_update_on_start,
        "skip_version": cfg.skip_update_version,
        "message": if skip.is_some() {
            if cfg.skip_update_version.is_empty() { "已不再跳过任何版本" } else { "已跳过该版本" }
        } else {
            "更新偏好已保存"
        },
    }))
}

/// 当前更新相关偏好（前端初始化时读一次，免得为了两个布尔量拉整份配置）。
#[tauri::command(async)]
pub fn cmd_update_prefs(state: State<'_, Arc<AppState>>) -> serde_json::Value {
    let cfg = state.cfg();
    serde_json::json!({
        "check_on_start": cfg.check_update_on_start,
        "skip_version": cfg.skip_update_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_split_strips_v_prefix() {
        assert_eq!(split_version("v0.41.0").0, vec![0, 41, 0]);
        assert_eq!(split_version("0.41.0").0, vec![0, 41, 0]);
        assert_eq!(split_version("V1.2.3").0, vec![1, 2, 3]);
        assert!(!split_version("0.41.0").1);
        assert!(split_version("0.41.0-beta.1").1);
        assert_eq!(split_version("0.41.0-beta.1").0, vec![0, 41, 0]);
    }

    #[test]
    fn version_compare_is_numeric_not_lexicographic() {
        // 字符串比较会把 "0.9.0" 排在 "0.10.0" 后面 —— 这正是必须转数字的原因
        assert_eq!(cmp_version("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(cmp_version("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(cmp_version("1.0.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn version_compare_handles_shorter_and_v_prefix() {
        assert_eq!(cmp_version("v0.41", "0.41.0"), Ordering::Equal);
        assert_eq!(cmp_version("v0.42.0", "0.41.0"), Ordering::Greater);
        assert_eq!(cmp_version("v0.41.0", "0.41.0"), Ordering::Equal);
        // 多一段数字也要能比
        assert_eq!(cmp_version("0.41.0.1", "0.41.0"), Ordering::Greater);
    }

    #[test]
    fn prerelease_is_older_than_release() {
        assert_eq!(cmp_version("0.42.0", "0.42.0-rc.1"), Ordering::Greater);
        assert_eq!(cmp_version("0.42.0-rc.1", "0.42.0"), Ordering::Less);
        // 两个都是预发布：主版本相同就算相等，不细比后缀
        assert_eq!(cmp_version("0.42.0-rc.1", "0.42.0-rc.2"), Ordering::Equal);
        // 但主版本更高的预发布仍然更新
        assert_eq!(cmp_version("0.43.0-rc.1", "0.42.0"), Ordering::Greater);
    }

    #[test]
    fn garbage_version_does_not_panic() {
        assert_eq!(cmp_version("nightly", "0.0.0"), Ordering::Equal);
        assert_eq!(cmp_version("", "0.0.0"), Ordering::Equal);
        assert!(!split_version("x.y.z").1);
    }

    #[test]
    fn asset_kind_classifies_by_extension() {
        assert_eq!(asset_kind("WordWise-Setup-0.41.0.exe").1, true);
        assert_eq!(asset_kind("WordWise-0.41.0-portable.zip").1, false);
        assert_eq!(asset_kind("notes.txt").0, "other");
        // 大小写不敏感：GitHub 上的名字不一定全小写
        assert_eq!(asset_kind("WordWise-SETUP.EXE").1, true);
    }

    #[test]
    fn pick_asset_prefers_installer_over_portable() {
        let mk = |name: &str| {
            let (kind, installable) = asset_kind(name);
            UpdateAsset {
                name: name.to_string(),
                url: format!("https://example.com/{name}"),
                size: 10,
                size_text: "10 B".to_string(),
                kind: kind.to_string(),
                installable,
                digest: String::new(),
            }
        };
        // 顺序刻意让 zip 排前面：挑出来的仍必须是对应的 exe
        let list = vec![
            mk("WordWise-0.42.0-portable.zip"),
            mk("WordWise-Setup-0.42.0.exe"),
        ];
        assert_eq!(pick_asset(&list).unwrap().name, "WordWise-Setup-0.42.0.exe");

        // 只有便携包时退而求其次
        let only_zip = vec![mk("WordWise-0.42.0-portable.zip")];
        assert_eq!(pick_asset(&only_zip).unwrap().kind, "portable");
        assert!(!pick_asset(&only_zip).unwrap().installable);

        // 什么都没有时是 None，而不是 panic
        assert!(pick_asset(&[]).is_none());
    }

    #[test]
    fn download_urls_put_direct_first_and_mirrors_after() {
        let u = download_urls("https://github.com/a/b/releases/download/v1/x.exe");
        assert_eq!(u[0], "https://github.com/a/b/releases/download/v1/x.exe");
        assert_eq!(u.len(), DOWNLOAD_MIRRORS.len() + 1);
        // 镜像就是把原地址拼在域名后面
        assert!(u[1].starts_with(DOWNLOAD_MIRRORS[0]));
        assert!(u[1].ends_with("/releases/download/v1/x.exe"));
    }

    /// 检查更新必须和下载用**同一批**镜像，否则又会出现「检查说不可达、
    /// 下载却能成」的不对称。
    #[test]
    fn check_urls_reuse_the_same_mirrors_as_download() {
        let api = "https://api.github.com/repos/DedalusArtin/wordwise/releases/latest";
        let u = check_urls(api);
        assert_eq!(u[0], api, "直连永远排第一");
        assert_eq!(u.len(), DOWNLOAD_MIRRORS.len() + 1);
        for (i, m) in DOWNLOAD_MIRRORS.iter().enumerate() {
            assert!(u[i + 1].starts_with(m));
            // 镜像就是把整个原始 URL 拼在域名后面
            assert_eq!(u[i + 1], format!("{m}{api}"));
        }
    }

    #[test]
    fn source_label_marks_mirror_usage() {
        assert_eq!(source_label("GitHub Releases", false), "GitHub Releases");
        assert!(source_label("GitHub Releases", true).contains("镜像"));
    }

    #[test]
    fn build_info_flags_skipped_version() {
        let v = serde_json::json!({
            "tag_name": "v0.42.0",
            "name": "WordWise v0.42.0",
            "body": "## 更新\n- 修了个 bug",
            "published_at": "2026-10-07T01:20:33Z",
            "html_url": "https://github.com/DedalusArtin/wordwise/releases/tag/v0.42.0",
            "prerelease": false,
            "assets": [
                { "name": "WordWise-Setup-0.42.0.exe",
                  "browser_download_url": "https://example.com/s.exe",
                  "size": 1234 }
            ]
        });

        let mut cfg = AppConfig::default();
        let info = build_info(&v, "0.41.0", &cfg, "test");
        assert!(info.has_update);
        assert_eq!(info.latest, "0.42.0");
        assert_eq!(info.published_at, "2026-10-07");
        assert_eq!(info.asset.as_ref().unwrap().name, "WordWise-Setup-0.42.0.exe");

        // 加进跳过名单后不再提示，但版本信息仍然完整带回来
        cfg.skip_update_version = "v0.42.0".to_string();
        let skipped = build_info(&v, "0.41.0", &cfg, "test");
        assert!(!skipped.has_update);
        assert!(skipped.skipped);
        assert_eq!(skipped.latest, "0.42.0");
    }

    #[test]
    fn build_info_says_no_update_when_current_is_newest() {
        let v = serde_json::json!({ "tag_name": "v0.41.0", "assets": [] });
        let info = build_info(&v, "0.41.0", &AppConfig::default(), "test");
        assert!(!info.has_update);
        assert!(info.asset.is_none());
    }

    #[test]
    fn build_info_truncates_huge_release_notes() {
        let long = "字".repeat(9000);
        let v = serde_json::json!({ "tag_name": "v0.42.0", "body": long });
        let info = build_info(&v, "0.41.0", &AppConfig::default(), "test");
        assert!(info.notes.contains("已截断"));
        assert!(info.notes.chars().count() < 9000);
    }

    #[test]
    fn prerelease_gets_a_note() {
        let v = serde_json::json!({ "tag_name": "v0.43.0-rc.1", "prerelease": true, "assets": [] });
        let info = build_info(&v, "0.41.0", &AppConfig::default(), "test");
        assert!(info.has_update);
        assert!(info.prerelease);
        assert!(info.note.unwrap().contains("预发布"));
    }
}
