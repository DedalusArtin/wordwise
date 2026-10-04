//! WordWise —— 基于本地 LM Studio 的可视化背单词与 AI 讲解一体化桌面软件。
//!
//! 模块划分：
//! - `models`   数据模型（词条、学习状态、配置）
//! - `srs`      记忆调度引擎（遗忘曲线 / SM-2 改良）
//! - `db`       SQLite 存储层
//! - `dict`     词典源抽象与 JSON 字段映射（小语种扩展点）
//! - `llm`      LM Studio / OpenAI 兼容客户端
//! - `search`   在线搜索与知识获取
//! - `net`      HTTP 基础设施
//! - `state`    应用共享状态
//! - `commands` Tauri 命令层
//! - `seed`     内置示例词库
//! - `timeutil` 系统时间工具

pub mod commands;
pub mod db;
pub mod dict;
pub mod llm;
pub mod models;
pub mod net;
pub mod search;
pub mod seed;
pub mod srs;
pub mod state;
pub mod timeutil;
pub mod windows;

/// 库入口：供 main.rs 调用。
pub fn run() {
    windows::run_app();
}
