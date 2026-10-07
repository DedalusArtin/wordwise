//! WordWise —— 基于本地 LM Studio 的可视化背单词与 AI 讲解一体化桌面软件。
//!
//! 模块划分：
//! - `models`   数据模型（词条、学习状态、配置）
//! - `srs`      记忆调度引擎（遗忘曲线 / SM-2 改良）
//! - `db`       SQLite 存储层
//! - `dict`     词典源抽象与 JSON 字段映射（小语种扩展点）
//! - `llm`      LM Studio / OpenAI 兼容客户端
//! - `graph`    知识图谱：词关系抽取与 AI 发散
//! - `localllm` 本地大模型一键部署（llama.cpp 托管 + 模型下载）
//! - `search`   在线搜索与知识获取
//! - `net`      HTTP 基础设施
//! - `state`    应用共享状态
//! - `commands` Tauri 命令层
//! - `seed`     内置示例词库
//! - `tts`      本地语音合成（Piper 引擎托管 + 语音包按需下载）
//! - `webview`  应用内网页浏览窗口（在线搜索点开即在此打开）
//! - `timeutil` 系统时间工具

pub mod commands;
pub mod db;
pub mod dict;
pub mod enrich;
pub mod graph;
pub mod llm;
pub mod logging;
pub mod localllm;
pub mod models;
pub mod morph;
pub mod net;
pub mod search;
pub mod seed;
pub mod srs;
pub mod state;
pub mod timeutil;
pub mod translate;
pub mod tts;
pub mod webview;
pub mod windows;

/// 库入口：供 main.rs 调用（桌面端），并由 Android 的 JNI 层回调（移动端）。
///
/// ★ `#[cfg_attr(mobile, tauri::mobile_entry_point)]` 这一行不能少。
///
/// 它做什么：在移动端展开出 `android_binding!` 那一套胶水，其中包含
/// `Java_app_tauri_plugin_PluginManager_handlePluginResponse`
/// 这个 `#[no_mangle] pub extern "C"` 符号 —— Kotlin 侧的 `PluginManager`
/// 靠它把 WebView 的回调交回 Rust。
///
/// 不加会怎样（本轮实测的故障链）：
///   1. 这个 JNI 符号不存在 → tauri-cli 在
///      `android_studio_script.rs::validate_lib()` 里直接报
///      "does not include required runtime symbols"
///      （那条提示会把人往 `mobile_entry_point` 的方向引，但它自己就是答案）；
///   2. 更隐蔽的是 —— 因为**没有任何导出符号引用 `run()`**，`cdylib` 链接时的
///      `--gc-sections` 会把整个 tauri 运行时全部剥掉。实测产物只有
///      **308 KB**、`llvm-nm -D --defined-only` 输出 **0 行**，
///      而正常的移动端 `.so` 是几十 MB 且带一批导出符号。
///      也就是说：APK 能被装出来、能被打开，进程却会在启动瞬间死掉，
///      而且没有任何编译期报错。
///
/// 为什么桌面端不受影响：桌面端的入口是 `main.rs` 直接调用本函数，
/// `mobile_entry_point` 只在 `mobile` 下生效，桌面二进制本来就不需要 JNI。
///
/// 宏内部要求 `TAURI_ANDROID_PACKAGE_NAME_PREFIX` /
/// `TAURI_ANDROID_PACKAGE_NAME_APP_NAME` 两个环境变量，由 `tauri-build`
/// 在 `build.rs`（本项目已调用 `tauri_build::build()`）里注入。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    windows::run_app();
}
