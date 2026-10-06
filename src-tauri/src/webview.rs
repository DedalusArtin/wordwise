//! 应用内网页浏览（需求 11：在线搜索不再跳到系统浏览器）。
//!
//! 点「在线搜索」里的一条结果，过去走 `commands::cmd_open_url` 弹系统浏览器；
//! 现在走 `commands::cmd_open_in_app`：**桌面端**建一个应用内的浏览窗口，
//! **移动端**没有多窗口语义，回退到系统浏览器（复用同一套 opener 逻辑）。
//!
//! 之所以把窗口逻辑单独放一个文件：它与 `windows.rs` 里的主窗口 / 侧边栏
//! 是两个互不相干的关注点 —— 那两个窗口加载的是应用自身页面（`WebviewUrl::App`），
//! 这里加载的是**外部网页**（`WebviewUrl::External`），并且需要额外的 URL 校验。
//!
//! 平台切分同样遵守 `windows.rs` 顶部那条约定：桌面与移动分开，
//! 移动端给的是**可用的回退**，而不是「编译能过、一点就崩」。

use tauri::AppHandle;

/// 应用内浏览窗口的标签。
///
/// 与 `windows::MAIN_LABEL` / `windows::SIDEBAR_LABEL` 并列，互不重名。
/// 同一个 label 在 Tauri 里只能对应一个窗口，这正是「复用同一个浏览窗口」
/// 的实现基础（`get_webview_window(WEBVIEW_LABEL)` 判存在性）。
#[cfg(desktop)]
pub const WEBVIEW_LABEL: &str = "webview";

/// 打开网页。
///
/// 调用方（`commands::cmd_open_in_app`）已经做过 `http(s)` 白名单校验，
/// 这里不再重复判断 scheme。
#[cfg(desktop)]
pub fn open_in_app(app: &AppHandle, url: &str) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let parsed = tauri::Url::parse(url).map_err(|e| format!("链接格式不对：{e}"))?;
    // 缺 host 的极端情况（如 `http://`）退回一个中性标题，不 panic
    let host = parsed.host_str().unwrap_or("网页浏览").to_string();

    // ---- 已存在则复用：把同一个窗口导航到新地址 ----
    //
    // 用户从搜索结果里连点几条时，体验是「同一个浏览窗口一直在用」，
    // 而不是每点一次多冒一个窗口。用 label 判存在性是最省事的做法：
    // 不需要自己在 AppState 里维护窗口句柄，也不会因为窗口被用户关掉
    // 而留下悬垂引用（关掉后 `get_webview_window` 自然返回 None）。
    if let Some(win) = app.get_webview_window(WEBVIEW_LABEL) {
        // 用 `navigate`（webview_window.rs 的 `pub fn navigate(&self, url: Url)`）
        // 而不是 `eval("location.href=...")`：
        //   - `navigate` 是 wry 的原生导航，不受页面 CSP / 协程限制，
        //     也不会因为目标页把 `location` 冻结而失效；
        //   - `eval` 需要先把 URL 拼进 JS 字符串，多一层转义与注入风险。
        win.navigate(parsed).map_err(|e| format!("导航到该网页失败：{e}"))?;
        win.set_title(&host).ok();
        win.show().ok();
        win.unminimize().ok();
        win.set_focus().ok();
        return Ok(());
    }

    // ---- 首次创建 ----
    //
    // `WebviewUrl::External` 加载外部网页；窗口**保留系统标题栏**
    // （不调 `decorations(false)`），这样自带最小化 / 最大化 / 关闭按钮，
    // 不必再自绘一套 —— 主窗口那套自绘边框是为了贴合应用 UI，
    // 而浏览窗口装的是别人家的页面，系统标题栏更合适。
    WebviewWindowBuilder::new(app, WEBVIEW_LABEL, WebviewUrl::External(parsed))
        .title(&host)
        .inner_size(1100.0, 820.0)
        .min_inner_size(640.0, 480.0)
        .resizable(true)
        .visible(true)
        .build()
        .map_err(|e| format!("创建浏览窗口失败：{e}"))?;

    Ok(())
}

/// 移动端回退：Android 没有「第二个窗口」这一说（见 `windows.rs` 顶部说明），
/// 直接复用现有的外部浏览器打开逻辑（`commands::mod` 的 `open_in_browser`），
/// 它与「权威词典」那排链接走的是同一条经过验证的路径。
#[cfg(mobile)]
pub fn open_in_app(app: &AppHandle, url: &str) -> Result<(), String> {
    crate::commands::open_in_browser(app, url).map_err(|e| e.to_string())
}
