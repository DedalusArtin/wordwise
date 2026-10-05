//! 数据库维护（需求 5：让「本地小型数据库」变得可见、可控）。
//!
//! 应用一直是单文件 SQLite（`%APPDATA%\WordWise\wordwise.db`），但用户
//! 看不到它、也管不了它。这一层把三件事摆到设置页上：
//!
//! - **看得见**：文件位置、真实体积、每张表多少行；
//! - **管得住**：一键整理（VACUUM 回收空洞）、一键备份（一致性快照）；
//! - **验得了**：`PRAGMA integrity_check`，出问题时能自证清白。
//!
//! 全部是本地同步操作，不联网、不上传。

use crate::state::AppState;
use crate::timeutil;
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 把字节数转成「12.3 MB」。
///
/// `pub(crate)`：存放位置面板（`commands::storage`）也要用同一套口径，
/// 两处显示不一样用户会以为哪里算错了。
pub(crate) fn human_size(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let f = n as f64;
    if f >= MB {
        format!("{:.2} MB", f / MB)
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

/// 数据库基本信息。
#[tauri::command]
pub fn cmd_db_info(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let (main, wal) = state.db.disk_usage();
    let tables: Vec<serde_json::Value> = state
        .db
        .table_counts()
        .map_err(err)?
        .into_iter()
        .map(|(name, rows)| {
            serde_json::json!({
                "name": name,
                // 表名给用户看的是「这张表是干什么的」，不是英文表名
                "label": table_label(&name),
                "rows": rows,
            })
        })
        .collect();

    let total_rows: i64 = tables
        .iter()
        .map(|t| t.get("rows").and_then(|v| v.as_i64()).unwrap_or(0))
        .sum();

    Ok(serde_json::json!({
        "path": state.db.path().display().to_string(),
        "data_dir": state.data_dir.display().to_string(),
        "size_bytes": main,
        "size_text": human_size(main),
        "wal_bytes": wal,
        "wal_text": human_size(wal),
        "tables": tables,
        "total_rows": total_rows,
        // 备份目录：导出与「打开所在文件夹」都用它
        "export_dir": state.data_dir.join("exports").display().to_string(),
        // 引擎与模型的落点（本地大模型一键部署用），提前告诉用户装在哪
        "models_dir": state.data_dir.join("models").display().to_string(),
        "now": timeutil::now_text(),
    }))
}

/// 表名 → 中文说明。
fn table_label(name: &str) -> &'static str {
    match name {
        "words" => "词库",
        "study_state" => "学习进度",
        "review_log" => "复习日志",
        "config" => "应用设置",
        "dict_sources" => "词典源",
        "dict_cache" => "词典缓存",
        "search_log" => "搜索历史",
        "wordbooks" => "分级词库",
        "wordbook_words" => "词库归属",
        "import_log" => "导入记录",
        "trans_history" => "翻译历史",
        "trans_cache" => "翻译缓存",
        "word_edges" => "知识图谱关系",
        "explain_store" => "AI 讲解存档",
        _ => "其它",
    }
}

/// 维护动作的结果。
#[derive(Debug, serde::Serialize)]
pub struct MaintResult {
    pub action: String,
    pub ok: bool,
    pub message: String,
    /// 执行前后的体积，前端可以直接显示「省了多少」
    pub before_text: String,
    pub after_text: String,
    /// 备份路径（仅 backup 动作有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 执行一次维护动作。
///
/// `action` 取值：
/// - `vacuum`   整理数据库（回收删除留下的空洞）
/// - `check`    完整性检查
/// - `backup`   导出一份一致性快照到数据目录
#[tauri::command]
pub fn cmd_db_maintain(
    state: State<'_, Arc<AppState>>,
    action: String,
) -> Result<MaintResult, String> {
    let (before, _) = state.db.disk_usage();
    let before_text = human_size(before);

    match action.as_str() {
        "vacuum" => {
            state.db.vacuum().map_err(err)?;
            let (after, _) = state.db.disk_usage();
            let saved = before.saturating_sub(after);
            let msg = if saved > 0 {
                format!("整理完成，回收了 {}", human_size(saved))
            } else {
                "整理完成，数据库已经很紧凑".to_string()
            };
            Ok(MaintResult {
                action,
                ok: true,
                message: msg,
                before_text,
                after_text: human_size(after),
                path: None,
            })
        }
        "check" => {
            let (status, detail) = state.db.integrity_check();
            Ok(MaintResult {
                action,
                ok: status == "ok",
                message: detail,
                before_text: before_text.clone(),
                after_text: before_text,
                path: None,
            })
        }
        "backup" => {
            let dir = state.data_dir.join("backups");
            std::fs::create_dir_all(&dir).map_err(err)?;
            let out = dir.join(format!(
                "wordwise-{}.db",
                crate::srs::day_key(timeutil::now_ts()).replace('-', "")
            ));
            state.db.backup_to(&out).map_err(err)?;
            let sz = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            Ok(MaintResult {
                action,
                ok: true,
                message: format!("已备份数据库快照（{}）", human_size(sz)),
                before_text,
                after_text: human_size(sz),
                path: Some(out.display().to_string()),
            })
        }
        other => Err(format!("不支持的维护动作：{other}")),
    }
}

/// 打开一个本地目录或文件所在的文件夹。
///
/// 单独做一个 `cmd_open_dir` 而不是复用 `cmd_open_url`：那个命令只允许
/// 打开 http(s)（防止被当成任意命令执行），这里要开的是本地路径。
/// 实现上仍然只把它交给系统 shell，且**强制要求路径存在**。
#[tauri::command]
pub fn cmd_open_dir(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("路径不存在：{path}"));
    }
    // 传文件时打开它所在的文件夹（用户点「打开所在文件夹」的直觉）
    let target = if p.is_dir() {
        p.clone()
    } else {
        p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
    };

    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(target.as_os_str())
            .spawn()
            .map_err(|e| format!("打开文件夹失败：{e}"))?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(target.as_os_str())
            .spawn()
            .map_err(|e| format!("打开文件夹失败：{e}"))?;
    }
    Ok(())
}
