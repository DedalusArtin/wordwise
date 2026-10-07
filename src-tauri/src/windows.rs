//! 窗口管理与应用启动。
//!
//! **桌面端**（Windows）包含三部分：
//! - 主窗口：完整功能界面
//! - **侧边栏窗口**：可长期挂起的窄条窗口，置顶、无边框、随时查词与学习
//! - 系统托盘：快速唤出侧边栏、开始复习、退出
//!
//! **移动端**（Android）只有系统给的那一个窗口：没有第二个窗口、没有托盘，
//! 也没有「关掉主窗口退到托盘」这回事。这批能力用 `#[cfg(desktop)]` 整段
//! 圈起来，移动端留下的是**可用的子集**，而不是「编译能过、一点就崩」。
//!
//! 之所以按 `desktop` / `mobile` 切而不是按 `windows` 切：菜单、托盘、
//! 多窗口这三类 API 在 Tauri 里是按「桌面 vs 移动」分发的，macOS / Linux
//! 与 Windows 共用同一套；只有「弹原生消息框」「读注册表代理」这类才是
//! 真正的 `cfg(windows)`。

use crate::commands;
#[cfg(desktop)]
use crate::state::resolve_data_dir_ex;
// 两个平台都要建状态，所以 `AppState` 无条件引入；
// `DataDirSource::System` 只有移动端用得到。
#[cfg(mobile)]
use crate::state::DataDirSource;
use crate::state::AppState;
use std::sync::Arc;
#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri::{AppHandle, Manager};

/// 侧边栏窗口标签
pub const SIDEBAR_LABEL: &str = "sidebar";
/// 主窗口标签
pub const MAIN_LABEL: &str = "main";

/// 命令占用主线程的告警阈值（毫秒）。
///
/// 只用于**记录**，不改变任何行为。同步执行的命令会占住主线程，占到 5 秒
/// Windows 就会把窗口判成「未响应」（事件日志里的 AppHangB1）。
const SLOW_CMD_MS: u64 = 300;

/// 注册所有 Tauri 命令。
///
/// ★ 外面这层计时不是装饰：异步命令在这里几乎瞬时返回（函数体在线程池上
///   跑），所以量出来的就是**真正堵住界面**的那段时间。下次再出现「未响应」，
///   日志里会直接点名是哪个命令，不用再靠猜。
fn build_invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    // 类型必须写死：`generate_handler!` 展开出的闭包不带任何类型标注，
    // 直接 `let inner = ...` 会让 rustc 推断不出来（E0282）。它不捕获任何
    // 东西，所以能安全退化成 fn 指针。
    let inner: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        // 系统与配置
        commands::cmd_app_info,
        commands::cmd_get_config,
        commands::cmd_save_config,
        commands::cmd_set_study_options,
        commands::cmd_set_llm_config,
        // LM Studio
        commands::cmd_llm_status,
        commands::cmd_llm_autoconnect,
        commands::cmd_ai_explain,
        commands::cmd_ai_explain_sync,
        commands::cmd_ai_generate_entry,
        // AI 讲解语言（需求：选择后即时生效并记住上次语言）
        commands::cmd_translate_text,
        commands::cmd_set_explain_lang,
        commands::cmd_set_ui_lang,
        // 查词与搜索
        commands::cmd_lookup,
        commands::cmd_lookup_pairs,
        commands::cmd_suggest,
        commands::cmd_search,
        commands::cmd_wiki,
        commands::cmd_dict_links,
        commands::cmd_open_url,
        // 需求 11：在线搜索的结果在**应用内**浏览窗口打开（不再跳系统浏览器）。
        // 桌面端建/复用 "webview" 窗口，移动端回退系统浏览器 —— 平台差异在
        // `webview.rs` 内部消化，所以这里无条件注册，与上面四个窗口命令同理。
        commands::cmd_open_in_app,
        commands::cmd_get_word,
        // 需求 5：同族派生词（happy → happiness…）—— 本地词库确认，不联网
        commands::cmd_word_family,
        commands::cmd_search_words,
        commands::cmd_recent_searches,
        // AI 讲解存档（讲解也是一种词库资料，可搜索、可并入词库）
        commands::explain::cmd_save_explain,
        commands::explain::cmd_get_explain,
        commands::explain::cmd_list_explains,
        commands::explain::cmd_search_explains,
        commands::explain::cmd_delete_explain,
        commands::explain::cmd_clear_explains,
        commands::explain::cmd_explain_count,
        commands::explain::cmd_explain_to_entry,
        // 后台词库内容增强（AI 空闲时把单薄词条补成统一详细的详解）
        commands::explain::cmd_enrich_status,
        commands::explain::cmd_enrich_word,
        // N 卡 / GPU 加速状态（Vulkan 后端 + nvidia-smi 探测）
        commands::localllm::cmd_gpu_status,
        // 关闭行为可选：直接退出（默认是缩小到托盘）
        commands::localllm::cmd_app_exit,
        // 运行日志诊断（自动分析错误/警告并给建议）+ 前端写日志
        crate::logging::cmd_log_analysis,
        crate::logging::cmd_log_write,
        // 词库
        commands::cmd_list_words,
        commands::cmd_add_word,
        commands::cmd_import_words,
        commands::cmd_delete_word,
        commands::cmd_word_count,
        commands::cmd_seed_demo,
        // 多级词库管理（需求 3 / 5）
        commands::books::cmd_list_wordbooks,
        commands::books::cmd_get_wordbook,
        commands::books::cmd_create_wordbook,
        commands::books::cmd_delete_wordbook,
        commands::books::cmd_import_words_to_book,
        commands::books::cmd_remote_catalog,
        commands::books::cmd_download_book,
        commands::books::cmd_reviewed_words,
        commands::books::cmd_words_in_book,
        // 翻译（需求 1-5）：三级链路 + AI 增强 + 历史收藏
        commands::translate::cmd_translate,
        commands::translate::cmd_translate_ai,
        commands::translate::cmd_translate_history,
        commands::translate::cmd_translate_favorite,
        commands::translate::cmd_translate_delete,
        commands::translate::cmd_translate_clear,
        commands::translate::cmd_swap_direction,
        commands::translate::cmd_translate_langs,
        commands::translate::cmd_translate_status,
        // 在线搜索 / 方向 / 进阶练习（需求 4 / 6 / 7）
        commands::extra::cmd_search_engines,
        commands::extra::cmd_web_search,
        commands::extra::cmd_lookup_links,
        // 网络与代理诊断
        commands::extra::cmd_network_info,
        commands::extra::cmd_network_report,
        commands::extra::cmd_reload_network,
        commands::extra::cmd_set_direction,
        commands::extra::cmd_get_direction,
        commands::extra::cmd_example_coverage,
        commands::extra::cmd_quiz_modes,
        commands::extra::cmd_start_book_session,
        commands::extra::cmd_check_spelling,
        commands::extra::cmd_spell_hint,
        commands::extra::cmd_mask_example,
        commands::extra::cmd_build_advanced_card,
        // 背诵与调度
        commands::cmd_start_session,
        commands::cmd_current_question,
        commands::cmd_submit_answer,
        commands::cmd_skip,
        commands::cmd_end_session,
        commands::cmd_word_state,
        // 复习会话与学习目标（需求 14 / 15）—— 均为新增命令，追加在此
        commands::cmd_start_review_session,
        commands::cmd_study_goal,
        commands::cmd_set_study_goal,
        commands::cmd_extra_study,
        // 统计与计划
        commands::cmd_stats,
        commands::cmd_review_plan,
        commands::plan::cmd_due_words,
        commands::cmd_leech_list,
        commands::cmd_clear_leech,
        // 知识图谱（独立栏目）
        commands::graph::cmd_graph_build,
        commands::graph::cmd_graph_view,
        commands::graph::cmd_graph_expand,
        commands::graph::cmd_graph_search,
        commands::graph::cmd_graph_rels,
        commands::graph::cmd_graph_clear,
        commands::graph::cmd_graph_stats,
        // 错题本增强：筛选 + 导出
        commands::leech::cmd_leech_query,
        commands::leech::cmd_leech_summary,
        commands::leech::cmd_leech_remove_many,
        commands::leech::cmd_leech_export,
        // 数据库维护
        commands::maint::cmd_db_info,
        commands::maint::cmd_db_maintain,
        commands::maint::cmd_open_dir,
        // 数据与模型的存放位置（需求：装到哪，数据就落哪，默认不写 C 盘）
        commands::storage::cmd_storage_info,
        commands::storage::cmd_set_data_dir,
        commands::storage::cmd_set_models_dir,
        commands::storage::cmd_restart_app,
        // 本地大模型一键部署
        commands::localllm::cmd_local_llm_status,
        commands::localllm::cmd_local_llm_models,
        commands::localllm::cmd_local_llm_install_engine,
        commands::localllm::cmd_local_llm_install_model,
        commands::localllm::cmd_local_llm_start,
        commands::localllm::cmd_local_llm_stop,
        commands::localllm::cmd_local_llm_probe,
        commands::localllm::cmd_local_llm_cancel,
        commands::localllm::cmd_local_llm_remove_model,
        commands::localllm::cmd_set_local_llm_auto,
        // 词典源
        commands::cmd_get_sources,
        commands::cmd_save_sources,
        commands::cmd_test_source,
        commands::cmd_reset_sources,
        commands::cmd_clear_cache,
        // 数据
        commands::cmd_export,
        commands::cmd_import,
        // 在线更新（版本检测 / 下载安装包 / 启动安装程序）
        commands::update::cmd_check_update,
        commands::update::cmd_download_update,
        commands::update::cmd_update_cancel,
        commands::update::cmd_run_update,
        commands::update::cmd_open_update_dir,
        commands::update::cmd_update_prefs,
        commands::update::cmd_set_update_prefs,
        // 朗读（本地 Piper 神经语音）：状态 / 装引擎 / 装语音包 / 合成
        commands::tts::cmd_tts_status,
        commands::tts::cmd_tts_install_engine,
        commands::tts::cmd_tts_install_voice,
        commands::tts::cmd_tts_remove_voice,
        commands::tts::cmd_tts_cancel,
        commands::tts::cmd_tts_prefs,
        commands::tts::cmd_set_tts_prefs,
        commands::tts::cmd_tts_speak,
        commands::tts::cmd_audio_devices,
        commands::tts::cmd_tts_clear_cache,
        // 窗口
        //
        // 这四个名字在**两个平台上都存在**，只是来源不同：
        //   桌面端 → 上面那批 `#[cfg(desktop)] fn sidebar_show ...`（真窗口操作）
        //   移动端 → 下面那批 `#[cfg(mobile)] fn sidebar_show ...`（返回人话 Err 的替身）
        // 所以这里**不能**加 `#[cfg(desktop)]` —— 加了的话移动端就不再注册这四个
        // 命令，前端 `invoke("sidebar_toggle")` 会拿到 "command not found"，
        // 而不是替身精心准备的那句中文字。见文件下方「移动端替身」段落。
        sidebar_show,
        sidebar_hide,
        sidebar_toggle,
        main_show,
    ];

    move |invoke: tauri::ipc::Invoke| -> bool {
        let cmd = invoke.message.command().to_string();
        let t0 = std::time::Instant::now();
        let handled = inner(invoke);
        let ms = t0.elapsed().as_millis() as u64;
        if ms >= SLOW_CMD_MS {
            log::warn!("慢命令 {} 占用主线程 {}ms（阈值 {}ms）", cmd, ms, SLOW_CMD_MS);
        }
        handled
    }
}

/// 创建主窗口（仅桌面端）。
///
/// 移动端不建窗口：Android 的 WebView 由 Activity 创建并托管，这里再建
/// 一个只会得到一个不显示的空壳，还会把 `MAIN_LABEL` 占住。
#[cfg(desktop)]
fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(MAIN_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App("index.html".into()))
        .title("WordWise · 背单词与 AI 讲解")
        .inner_size(1180.0, 780.0)
        .min_inner_size(900.0, 620.0)
        .center()
        // 无边框 + 自绘标题栏（#titlebar）：窗口顶部只保留一套按钮，
        // 不会出现「系统标题栏一排 + 自绘一排」的重复控件。
        // 注意：decorations(false) 不会移除 WS_THICKFRAME，拖拽边缘缩放仍然可用。
        .decorations(false)
        .resizable(true)
        .visible(true)
        .build()?;
    Ok(())
}

/// 创建侧边栏窗口：始终置顶的窄条，长期挂起（仅桌面端）。
///
/// 移动端没有「第二个窗口」这一说，`always_on_top` / `skip_taskbar`
/// 这类窗口属性在 Android 上也不存在。
#[cfg(desktop)]
fn create_sidebar_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(SIDEBAR_LABEL).is_some() {
        return Ok(());
    }
    let width = 380.0;
    let height = 620.0;

    WebviewWindowBuilder::new(app, SIDEBAR_LABEL, WebviewUrl::App("index.html?view=sidebar".into()))
        .title("WordWise 侧边栏")
        .inner_size(width, height)
        // 置顶：用户随时查词不被打断
        .always_on_top(true)
        .resizable(true)
        .min_inner_size(300.0, 400.0)
        .decorations(false)
        .visible(false)
        .skip_taskbar(true)
        .build()?;

    // 关闭时改为隐藏而非销毁，保持常驻
    if let Some(win) = app.get_webview_window(SIDEBAR_LABEL) {
        let w = win.clone();
        win.on_window_event(move |e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        });
    }
    Ok(())
}

/// 显示侧边栏。
#[cfg(desktop)]
#[tauri::command]
fn sidebar_show(app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window(SIDEBAR_LABEL)
        .ok_or_else(|| "侧边栏窗口未创建".to_string())?;

    // 首次显示时贴右边缘，避免遮挡工作区
    if !win.is_visible().unwrap_or(false) {
        if let Ok(Some(monitor)) = win.primary_monitor() {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let logical_w = size.width as f64 / scale;
            let logical_h = size.height as f64 / scale;
            let wsize = win
                .outer_size()
                .map(|s| (s.width as f64 / scale, s.height as f64 / scale))
                .unwrap_or((380.0, 620.0));
            let x = (logical_w - wsize.0 - 16.0).max(0.0);
            let y = ((logical_h - wsize.1) / 2.0).max(0.0);
            let _ = win.set_position(tauri::LogicalPosition::new(x, y));
        }
    }

    win.show().map_err(|e| e.to_string())?;
    win.set_focus().ok();
    win.set_always_on_top(true).ok();
    Ok(())
}

/// 隐藏侧边栏。
#[cfg(desktop)]
#[tauri::command]
fn sidebar_hide(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(SIDEBAR_LABEL) {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 切换侧边栏显隐。
#[cfg(desktop)]
#[tauri::command]
fn sidebar_toggle(app: AppHandle) -> Result<bool, String> {
    let win = app
        .get_webview_window(SIDEBAR_LABEL)
        .ok_or_else(|| "侧边栏窗口未创建".to_string())?;
    let visible = win.is_visible().unwrap_or(false);
    if visible {
        win.hide().map_err(|e| e.to_string())?;
        Ok(false)
    } else {
        sidebar_show(app)?;
        Ok(true)
    }
}

/// 显示并聚焦主窗口。
#[cfg(desktop)]
#[tauri::command]
fn main_show(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(MAIN_LABEL) {
        win.show().map_err(|e| e.to_string())?;
        win.unminimize().ok();
        win.set_focus().ok();
    } else {
        create_main_window(&app).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 构建系统托盘（仅桌面端）。
///
/// 移动端没有托盘图标这个系统概念，`tauri::tray` 模块在 Android 目标上
/// 根本不存在（连 `use` 都会编译失败），所以整段圈进 `cfg(desktop)`。
#[cfg(desktop)]
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_main = MenuItem::with_id(app, "open_main", "打开主界面", true, None::<&str>)?;
    let toggle_side = MenuItem::with_id(app, "toggle_sidebar", "显示/隐藏侧边栏", true, None::<&str>)?;
    let start_review = MenuItem::with_id(app, "start_review", "开始复习", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出 WordWise", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open_main, &toggle_side, &start_review, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("WordWise · 背单词与 AI 讲解")
        .menu(&menu)
        // 左键单击切换侧边栏，符合「随时查词」的使用习惯
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_main" => {
                let _ = main_show(app.clone());
            }
            "toggle_sidebar" => {
                let _ = sidebar_toggle(app.clone());
            }
            "start_review" => {
                let _ = main_show(app.clone());
                let _ = app.emit("app://start-review", ());
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                let _ = sidebar_toggle(app.clone());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

/* ---------------- 移动端替身 ---------------- */
/*
   这四个命令在移动端仍然要**存在**，因为 `build_invoke_handler` 里按名字
   引用了它们，而前端的按钮也不分平台。直接 cfg 掉会编译失败；留一个
   「能过但一点就崩」的实现更糟 —— 所以这里给出明确返回 Err 的替身，
   前端拿到的是一句人话，而不是一个 ReferenceError。

   注意返回的是 Err 而不是 Ok(())：静默成功会让界面以为侧边栏已经弹出来了。

   ★ 替身必须定义在**本模块根、且不加 `pub`**，不能另起一层 `mod`。
   原因在 `tauri::command` 生成的 `__cmd__xxx` / `__tauri_command_name_xxx`
   这两个 macro_rules 宏上，它们带不带 `#[macro_export]` 由函数可见性决定
   （tauri-macros 的 wrapper.rs）：

       pub fn  → 加 `#[macro_export]` → 宏被挂到 **crate 根**，
                 在 windows.rs 里反而看不见，得 `use crate::__cmd__xxx`
       私有 fn → 不加 → 宏留在定义它的模块内，同模块里（包括展开后的
                 `generate_handler!`）可以直接按名调用

   桌面端那四个是真身且都是**私有 fn**，走的是第二条路；替身原来是
   `pub fn` 且藏在 `mod mobile_stubs` 里 —— 宏被搬到 crate 根、定义又在
   别的模块，两条路都不占，于是编译报：

       error: cannot find macro `__cmd__sidebar_show` in this scope

   挪到模块根、去掉 `pub`，就和桌面端完全同构了。
*/

/// 移动端调用这四个命令时统一给出的话。
#[cfg(mobile)]
const MOBILE_WINDOW_UNAVAILABLE: &str = "侧边栏是桌面端能力，移动端没有常驻窗口";

#[cfg(mobile)]
#[tauri::command]
fn sidebar_show(_app: AppHandle) -> Result<(), String> {
    Err(MOBILE_WINDOW_UNAVAILABLE.to_string())
}

#[cfg(mobile)]
#[tauri::command]
fn sidebar_hide(_app: AppHandle) -> Result<(), String> {
    Err(MOBILE_WINDOW_UNAVAILABLE.to_string())
}

#[cfg(mobile)]
#[tauri::command]
fn sidebar_toggle(_app: AppHandle) -> Result<bool, String> {
    Err(MOBILE_WINDOW_UNAVAILABLE.to_string())
}

/// 移动端只有一个窗口，且它一定在前台 —— 无事可做，返回成功。
#[cfg(mobile)]
#[tauri::command]
fn main_show(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(MAIN_LABEL) {
        win.set_focus().ok();
    }
    // 抑制未使用告警：标签常量在替身里只为保持语义对称而保留
    let _ = SIDEBAR_LABEL;
    Ok(())
}

/// 应用主入口。
///
/// 桌面端与移动端的差别集中在两处：
///  - **数据目录怎么定**：桌面端在构造 Builder 之前就能定下来（环境变量 /
///    便携标记 / 指针文件 / 老数据 / exe 同级 / 系统目录）；移动端必须等到
///    setup 里 Tauri 给出应用私有目录才有正确答案。
///  - **窗口与托盘**：桌面端专有，见本文件顶部说明。
pub fn run_app() {
    // ---- 桌面端：数据目录先定下来，再交给 Tauri 托管 ----
    #[cfg(desktop)]
    let desktop_state = {
        let (data_dir, data_dir_source) = resolve_data_dir_ex();
        println!(
            "WordWise 数据目录：{}（{}）",
            data_dir.display(),
            data_dir_source.label()
        );
        match AppState::with_source(data_dir, data_dir_source) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("初始化失败：{}", e);
                show_fatal(&format!("WordWise 启动失败：\n{}", e));
                None
            }
        }
    };
    #[cfg(desktop)]
    if desktop_state.is_none() {
        return;
    }

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(build_invoke_handler());

    #[cfg(desktop)]
    {
        if let Some(state) = desktop_state {
            builder = builder.manage(state);
        }
    }

    builder = builder.setup(|app| {
        // ---- 移动端：数据目录只能在这一步定 ----
        //
        // ★ 不能复用桌面端那套解析：Android 上 `std::env::current_exe()`
        //   返回的是 `/system/bin/app_process*`，`dirs::data_dir()` 也不可靠，
        //   照搬会得到「数据写进系统目录」这种既没权限又不可能备份的结果。
        #[cfg(mobile)]
        {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("取不到应用数据目录：{e}"))?;
            println!("WordWise 数据目录：{}（应用私有目录）", data_dir.display());
            let state =
                AppState::with_source(data_dir, DataDirSource::System).map_err(|e| e.to_string())?;
            app.manage(state);
        }

        let handle = app.handle().clone();

        #[cfg(desktop)]
        {
            create_main_window(&handle)?;
            // 侧边栏预先创建但隐藏，点托盘时才显示
            if let Err(e) = create_sidebar_window(&handle) {
                eprintln!("创建侧边栏失败：{}", e);
            }
            if let Err(e) = build_tray(&handle) {
                eprintln!("创建托盘失败：{}", e);
            }
        }

        // 首次启动且词库为空时，写入示例词库，保证开箱即用
        let st = handle.state::<Arc<AppState>>();
        let lang = st.cfg().target_lang.clone();
        if st.db.word_count(&lang).unwrap_or(0) == 0 {
            let entries = crate::seed::demo_words(&lang);
            let now = crate::timeutil::now_ts();
            let _ = st.db.bulk_upsert_words(&entries, now);
            for e in &entries {
                let _ = st
                    .db
                    .upsert_state(&crate::models::StudyState::new(&e.word, &lang, now));
            }
            println!("已写入 {} 条示例词库", entries.len());
        }

        // 用户开过「自动启动」时，后台悄悄把本地模型服务拉起来。
        // 这里只是派发线程，不会阻塞窗口显示。
        //
        // ★ 移动端不走这条路：llama-server 是外部二进制，而 Android 从
        //   API 29 起禁止 App 执行自己数据目录里的可执行文件（W^X）。
        //   照搬桌面端逻辑只会得到一串启动失败，而不是可用的本地模型 ——
        //   这就是「APK 版本模型能力下降」的技术根因，不是参数没调好。
        #[cfg(desktop)]
        commands::localllm::spawn_autostart(st.inner().clone());

        Ok(())
    });

    // 主窗口关闭时退到托盘，保持「随时查词」能力可用（移动端没有这回事）
    #[cfg(desktop)]
    {
        builder = builder.on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == MAIN_LABEL => {
                // ★ 关闭行为必须在这里也尊重「关闭时缩小到托盘」开关——
                //   此前这里无条件隐藏，Alt+F4 / 任务栏关闭永远缩托盘，
                //   设置里的开关形同虚设（只有自绘 X 按钮那条路认开关）。
                let state = window.app_handle().state::<Arc<AppState>>();
                if state.cfg().study.close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();

                    // 仅当用户开启了侧边栏常驻时才自动唤出，避免突兀弹出
                    let keep_sidebar = state.cfg().sidebar_always_on_top;
                    if keep_sidebar {
                        let _ = sidebar_show(window.app_handle().clone());
                    }
                } else {
                    // 直接退出：与自绘 X 的「退出」同一出口（托盘注销、子进程回收）
                    api.prevent_close();
                    window.app_handle().exit(0);
                }
            }
            _ => {}
        });
    }

    let app = builder
        .build(tauri::generate_context!())
        .expect("WordWise 构建失败");

    #[cfg(desktop)]
    app.run(|_handle, event| {
        // ★ 退出时必须回收托管的 llama-server 子进程。
        //   漏掉这一步，用户关掉应用后本地模型服务会变成孤儿进程，
        //   继续占着上 GB 内存 —— 在低配机器上等于「关不掉」。
        if let tauri::RunEvent::Exit = event {
            commands::localllm::cleanup_on_exit();
        }
    });

    #[cfg(mobile)]
    app.run(|_handle, _event| {});
}

/// 致命错误弹窗（不依赖任何 GUI 库）。
///
/// 桌面端专有：移动端的启动失败走 setup 返回 Err，由宿主系统记录日志。
#[cfg(desktop)]
fn show_fatal(msg: &str) {
    #[cfg(windows)]
    {
        use std::process::Command;
        // 用 PowerShell 弹一个原生消息框，避免引入额外依赖
        let script = format!(
            "[System.Windows.Forms.MessageBox]::Show('{}','WordWise',0,16)",
            msg.replace('\'', "''")
        );
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!(
                "Add-Type -AssemblyName System.Windows.Forms; {}",
                script
            )])
            .spawn();
    }
    #[cfg(not(windows))]
    {
        eprintln!("{}", msg);
    }
}
