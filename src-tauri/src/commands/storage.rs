//! 数据与模型的存放位置。
//!
//! 背景：早期版本把数据**无条件**放 `%APPDATA%\WordWise`（C 盘），于是
//! 「把软件装到 D 盘」也拦不住模型（单档最大 1.1 GB）写进 C 盘 ——
//! 用户明明有别的盘，却只能看着系统盘被塞满，而且**没有任何入口能改**。
//!
//! 这一层把三件事交还给用户：
//!
//! 1. **看得见**：数据目录 / 模型目录 / 推理引擎各在哪、各占多少、
//!    引擎是随包带的还是要联网下载、三档模型缺哪档；
//! 2. **搬得走**：换到别的盘。可选把现有数据一起迁过去（默认勾上），
//!    数据库走 `VACUUM INTO` 一致性快照，**原目录一个文件都不删**；
//! 3. **退得回**：一键恢复默认（往指针文件里写 `default`，不删任何东西）。
//!
//! 目录的选择规则本身在 `state::resolve_data_dir_with`，
//! 这里只负责展示与改指针文件。

use crate::commands::localllm::app_dir;
use crate::commands::maint::human_size;
use crate::localllm;
use crate::state::{self, AppState, LOCATION_FILE, PORTABLE_FILE};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

/* ============================================================
   小工具
   ============================================================ */

/// 目录递归占用：`(字节数, 文件数)`。
///
/// 用 `symlink_metadata` 并显式跳过链接/联接：Windows 目录联接（junction）
/// 在旧版 Rust 里会被当成普通目录，递归进去就会打转或跨盘扫全盘。
fn dir_size(dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
            if md.file_type().is_symlink() {
                continue;
            }
            if md.is_dir() {
                stack.push(p);
            } else if md.is_file() {
                bytes += md.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// 递归复制目录。返回 `(复制的文件数, 复制的字节数)`。
///
/// 目标已存在的同名文件**跳过、不覆盖**：目标目录里可能是用户自己
/// 放进去的东西，覆盖掉的代价远大于「少搬一个文件」。
fn copy_tree(src: &Path, dst: &Path, skip: &[&str]) -> Result<(u64, u64), String> {
    if !src.is_dir() {
        return Ok((0, 0));
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("无法创建 {}：{e}", dst.display()))?;

    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![src.to_path_buf()];

    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if skip.iter().any(|s| name == *s) {
                continue;
            }
            let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
            if md.file_type().is_symlink() {
                continue;
            }
            let Ok(rel) = p.strip_prefix(src) else { continue };
            let out = dst.join(rel);

            if md.is_dir() {
                std::fs::create_dir_all(&out)
                    .map_err(|e| format!("无法创建 {}：{e}", out.display()))?;
                stack.push(p);
            } else if md.is_file() {
                if out.exists() {
                    continue;
                }
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("无法创建 {}：{e}", parent.display()))?;
                }
                std::fs::copy(&p, &out).map_err(|e| format!("复制 {} 失败：{e}", p.display()))?;
                files += 1;
                bytes += md.len();
            }
        }
    }
    Ok((files, bytes))
}

/// 用户输入归一化：空 / `default` → `None`（= 恢复默认）。
fn norm_input(raw: &str) -> Option<PathBuf> {
    let s = raw.trim_start_matches('\u{feff}').trim();
    if s.is_empty() || s.eq_ignore_ascii_case("default") {
        return None;
    }
    Some(PathBuf::from(s))
}

/// 写指针文件。内容为空字符串 = 恢复默认（见 `state::read_pointer`）。
///
/// ★ **不删文件**：本机的安全策略会把删除动作 fail-closed 拦下来
/// （跨盘回收站不支持），写 `default` 一样能达到「恢复默认」的效果。
fn write_pointer(file: &Path, value: &str) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建 {}：{e}", dir.display()))?;
    }
    std::fs::write(file, value).map_err(|e| {
        format!(
            "写入 {} 失败：{e}\n（程序所在目录不可写时会这样，试试把 WordWise 装到用户目录，或用管理员身份运行）",
            file.display()
        )
    })
}

/// 目标目录可用性检查：能建、能写。
///
/// 只判「目录存在」不够 —— `C:\Program Files\...` 存在但只读，
/// 那样会在第一次下载模型时才抛出一个很晚、很难懂的错误。
fn ensure_writable(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("无法创建 {}：{e}", dir.display()))?;
    let probe = dir.join(".wordwise-write-test");
    std::fs::write(&probe, b"ok")
        .map_err(|e| format!("{} 不可写：{e}\n（换一个盘或换一个目录试试）", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 拒绝「新目录套在旧目录里 / 旧目录套在新目录里」。
///
/// 不拦的话迁移会**自己复制自己**：往正在遍历的树里写新文件，
/// 轻则无限膨胀，重则把盘写满。
fn reject_nesting(cur: &Path, target: &Path, what: &str) -> Result<(), String> {
    if target.starts_with(cur) {
        return Err(format!(
            "新的{what}不能是当前{what}的子目录。\n当前：{}\n新的：{}\n请换一个不在里面的位置。",
            cur.display(),
            target.display()
        ));
    }
    if cur.starts_with(target) {
        return Err(format!(
            "新的{what}不能是当前{what}的上级目录。\n当前：{}\n新的：{}\n请换一个位置。",
            cur.display(),
            target.display()
        ));
    }
    Ok(())
}

/* ============================================================
   磁盘列表（Windows）
   ============================================================ */

/// 直接调 kernel32 取本机盘符与剩余空间。
///
/// 为什么不用现成 crate：只需要两个函数，为它引一个依赖（还会带一串
/// 传递依赖）不划算；`dirs` 也不提供「列出所有盘」的能力。
/// 而「别写 C 盘」这件事必须让用户**看见**其它盘有多少空间才谈得上选。
#[cfg(windows)]
mod win_drives {
    pub struct Drive {
        pub letter: char,
        /// `"fixed"` / `"removable"`
        pub kind: &'static str,
        pub total: u64,
        pub free: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;

    pub fn list() -> Vec<Drive> {
        // SAFETY: 三个都是无副作用的查询函数；传入的宽字符串以 NUL 结尾，
        // 出参都是本栈上初始化的 u64。
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root: Vec<u16> = format!("{letter}:\\")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();

            let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
                DRIVE_FIXED => "fixed",
                DRIVE_REMOVABLE => "removable",
                // 光驱、网络盘、没插卡的读卡器都不列：选了也写不进去
                _ => continue,
            };

            let mut free_to_caller = 0u64;
            let mut total = 0u64;
            let mut total_free = 0u64;
            let ok = unsafe {
                GetDiskFreeSpaceExW(
                    root.as_ptr(),
                    &mut free_to_caller,
                    &mut total,
                    &mut total_free,
                )
            };
            if ok == 0 {
                // 空的读卡器槽会走到这里
                continue;
            }
            out.push(Drive {
                letter,
                kind,
                total,
                free: total_free,
            });
        }
        out
    }
}

#[cfg(windows)]
fn drives_json() -> Vec<serde_json::Value> {
    win_drives::list()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "letter": d.letter.to_string(),
                "root": format!("{}:\\", d.letter),
                "kind": d.kind,
                "kind_label": if d.kind == "removable" { "可移动磁盘" } else { "本地磁盘" },
                // 界面上给系统盘打个标：用户要的就是「别放这里」
                "system": d.letter == 'C',
                "total_bytes": d.total,
                "free_bytes": d.free,
                "free_text": human_size(d.free),
            })
        })
        .collect()
}

#[cfg(not(windows))]
fn drives_json() -> Vec<serde_json::Value> {
    Vec::new()
}

/* ============================================================
   一、看：当前都放在哪、占多少
   ============================================================ */

/// 数据与模型的存放位置总览。
#[tauri::command]
pub fn cmd_storage_info(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let data_dir = state.data_dir.clone();
    let src = state.data_dir_source;
    let exe = app_dir();

    let (db_main, db_wal) = state.db.disk_usage();
    let (data_bytes, data_files) = dir_size(&data_dir);

    let default_models = data_dir.join("models");
    let models = localllm::models_dir(&data_dir);
    // 模型目录可能被单独挪到了别的盘；那样它的占用就不算在数据目录里，
    // 否则「合计」会把同一批文件算两遍。
    let models_custom = models != default_models;
    let (models_bytes, models_files) = dir_size(&models);

    let bundled = localllm::bundled_engine_dir(&exe);
    let engine = localllm::find_engine(&exe, &data_dir);
    let engine_from_bundle = engine.as_ref().map(|e| e.starts_with(&bundled)).unwrap_or(false);
    let (engine_bytes, engine_files) = engine.as_deref().map(dir_size).unwrap_or((0, 0));

    // 清单里有什么 vs 磁盘上真有什么，分开列 —— 用户才知道自己缺哪一档
    let mut installed = Vec::new();
    let mut missing = Vec::new();
    for m in localllm::models() {
        let has = models.join(&m.file).is_file();
        let row = serde_json::json!({
            "id": m.id,
            "name": m.name,
            "file": m.file,
            "size_bytes": m.size_bytes,
            "size_text": human_size(m.size_bytes),
            "installed": has,
        });
        if has {
            installed.push(row);
        } else {
            missing.push(row);
        }
    }

    let total_bytes = data_bytes
        + if models_custom { models_bytes } else { 0 }
        + engine_bytes;

    let portable_file = exe.join(PORTABLE_FILE);
    Ok(serde_json::json!({
        // ---- 数据目录 ----
        "data_dir": data_dir.display().to_string(),
        "source": src,
        "source_label": src.label(),
        "travels_with_app": src.travels_with_app(),
        // 环境变量是「外部强制指定」：这种情况下设置页改不动，要如实告诉用户
        "from_env": matches!(src, state::DataDirSource::Env),
        "pointer_file": exe.join(LOCATION_FILE).display().to_string(),
        "portable": portable_file.is_file(),
        "portable_file": portable_file.display().to_string(),
        "env_data_dir": std::env::var("WORDWISE_DATA_DIR").unwrap_or_default(),
        "exe_dir": exe.display().to_string(),
        "data_bytes": data_bytes,
        "data_text": human_size(data_bytes),
        "data_files": data_files,

        // ---- 数据库 ----
        "db_path": state.db.path().display().to_string(),
        "db_bytes": db_main,
        "db_text": human_size(db_main),
        "db_wal_text": human_size(db_wal),

        // ---- 模型目录 ----
        "models_dir": models.display().to_string(),
        "models_dir_default": default_models.display().to_string(),
        "models_custom": models_custom,
        "models_bytes": models_bytes,
        "models_text": human_size(models_bytes),
        "models_files": models_files,
        "models_pointer_file": data_dir.join(localllm::MODELS_POINTER).display().to_string(),
        "models_env": std::env::var(localllm::MODELS_DIR_ENV).unwrap_or_default(),
        "models_installed": installed,
        "models_missing": missing,

        // ---- 推理引擎 ----
        "engine_dir": engine.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
        "engine_ready": engine.is_some(),
        "engine_from_bundle": engine_from_bundle,
        "engine_bytes": engine_bytes,
        "engine_files": engine_files,
        "engine_text": human_size(engine_bytes),

        // ---- 其它 ----
        "total_text": human_size(total_bytes),
        "drives": drives_json(),
    }))
}

/* ============================================================
   二、换：数据目录
   ============================================================ */

#[tauri::command]
pub async fn cmd_set_data_dir(
    state: State<'_, Arc<AppState>>,
    path: String,
    migrate: bool,
) -> Result<serde_json::Value, String> {
    let st = state.inner().clone();
    // 可能要搬 GB 级的模型文件，绝不能占着 UI 线程（界面会「未响应」）
    tauri::async_runtime::spawn_blocking(move || set_data_dir_impl(&st, &path, migrate))
        .await
        .map_err(|e| format!("迁移任务异常退出：{e}"))?
}

fn set_data_dir_impl(st: &AppState, raw: &str, migrate: bool) -> Result<serde_json::Value, String> {
    let pointer = app_dir().join(LOCATION_FILE);
    let cur = st.data_dir.clone();

    let Some(target) = norm_input(raw) else {
        // 恢复默认：写空内容。不删文件 —— 删除会被安全策略拦下，
        // 而且「把指针指回默认」本身就达到目的了。
        write_pointer(&pointer, "")?;
        return Ok(serde_json::json!({
            "ok": true,
            "changed": true,
            "restart_required": true,
            "migrated": false,
            "message": "已恢复默认：数据会跟着软件目录走。重启后生效。",
            "path": "",
        }));
    };

    if !target.is_absolute() {
        return Err(format!(
            "请填写带盘符的完整路径，例如 D:\\WordWiseData。\n现在填的是：{}",
            target.display()
        ));
    }
    if target == cur {
        return Ok(serde_json::json!({
            "ok": true,
            "changed": false,
            "restart_required": false,
            "message": "数据已经在这个目录里了，不用改。",
            "path": target.display().to_string(),
        }));
    }
    reject_nesting(&cur, &target, "数据目录")?;
    ensure_writable(&target)?;

    let has_data = target.join("wordwise.db").is_file();
    if migrate && has_data {
        return Err(format!(
            "{} 里已经有一份 WordWise 数据了。\n\
             想直接切过去用它 → 把「同时迁移现有数据」关掉再点一次；\n\
             想用当前这份覆盖它 → 请先自己备份或清空那个目录。",
            target.display()
        ));
    }

    let mut copied_files = 0u64;
    let mut copied_bytes = 0u64;
    let mut migrated = false;

    if migrate {
        // DB 由 VACUUM INTO 单独处理（直接拷 .db 可能拿到写一半的状态、
        // 或者漏掉还在 WAL 里的已提交数据），所以这里跳过它和它的伴生文件。
        let skip = [
            "wordwise.db",
            "wordwise.db-wal",
            "wordwise.db-shm",
            "wordwise.db-journal",
            // 模型指针要重新算：模型目录可能被单独挪到过别处，
            // 直接照抄指针会让新目录继续指向旧位置。
            localllm::MODELS_POINTER,
        ];
        let (f, b) = copy_tree(&cur, &target, &skip)?;
        copied_files += f;
        copied_bytes += b;

        // 模型目录若被单独设过（在数据目录之外），也一并搬进新数据目录
        let old_models = localllm::models_dir(&cur);
        let new_models = target.join("models");
        if old_models != cur.join("models") && old_models != new_models {
            let (f2, b2) = copy_tree(&old_models, &new_models, &[])?;
            copied_files += f2;
            copied_bytes += b2;
        }

        st.db
            .backup_to(&target.join("wordwise.db"))
            .map_err(|e| format!("导出数据库失败：{e}"))?;
        migrated = true;
    }

    write_pointer(&pointer, &target.display().to_string())?;

    let message = if migrated {
        format!(
            "已把 {} 个文件（{}）迁到新目录，并指向它。\n\
             原目录里的文件**没有删除**，确认新目录一切正常后可以自己清理。\n\
             重启 WordWise 后生效。",
            copied_files,
            human_size(copied_bytes)
        )
    } else {
        "已指向新目录（没有迁移现有数据，新目录会是空的）。\n重启 WordWise 后生效。".to_string()
    };

    Ok(serde_json::json!({
        "ok": true,
        "changed": true,
        "restart_required": true,
        "migrated": migrated,
        "had_data": has_data,
        "copied_files": copied_files,
        "copied_text": human_size(copied_bytes),
        "path": target.display().to_string(),
        "pointer": pointer.display().to_string(),
        "message": message,
    }))
}

/* ============================================================
   三、换：模型目录（可以只挪模型，不动数据）
   ============================================================ */

#[tauri::command]
pub async fn cmd_set_models_dir(
    state: State<'_, Arc<AppState>>,
    path: String,
    migrate: bool,
) -> Result<serde_json::Value, String> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || set_models_dir_impl(&st, &path, migrate))
        .await
        .map_err(|e| format!("迁移任务异常退出：{e}"))?
}

fn set_models_dir_impl(st: &AppState, raw: &str, migrate: bool) -> Result<serde_json::Value, String> {
    let data_dir = st.data_dir.clone();
    let pointer = data_dir.join(localllm::MODELS_POINTER);
    let cur = localllm::models_dir(&data_dir);
    let default_dir = data_dir.join("models");

    // 空 / default → 回到 <数据目录>\models
    let target = norm_input(raw).unwrap_or_else(|| default_dir.clone());

    if !target.is_absolute() {
        return Err(format!(
            "请填写带盘符的完整路径，例如 D:\\WordWiseModels。\n现在填的是：{}",
            target.display()
        ));
    }
    if target == cur {
        return Ok(serde_json::json!({
            "ok": true,
            "changed": false,
            // 模型目录是**每次用的时候现读指针**的，不需要重启
            "restart_required": false,
            "message": "模型目录已经是这里了，不用改。",
            "path": target.display().to_string(),
        }));
    }
    reject_nesting(&cur, &target, "模型目录")?;
    ensure_writable(&target)?;

    let mut copied_files = 0u64;
    let mut copied_bytes = 0u64;
    if migrate {
        let (f, b) = copy_tree(&cur, &target, &[])?;
        copied_files = f;
        copied_bytes = b;
    }

    if target == default_dir {
        write_pointer(&pointer, "default")?;
    } else {
        write_pointer(&pointer, &target.display().to_string())?;
    }

    let message = if copied_files > 0 {
        format!(
            "已把 {} 个模型文件（{}）搬到新目录，并指向它。原目录里的文件没有删除，可以自己清理。\n\
             已下载的模型立刻可用，不用重启。",
            copied_files,
            human_size(copied_bytes)
        )
    } else {
        "已指向新的模型目录。下次下载的模型会存到这里，不用重启。".to_string()
    };

    Ok(serde_json::json!({
        "ok": true,
        "changed": true,
        "restart_required": false,
        "migrated": copied_files > 0,
        "copied_files": copied_files,
        "copied_text": human_size(copied_bytes),
        "path": target.display().to_string(),
        "pointer": pointer.display().to_string(),
        "message": message,
    }))
}

/* ============================================================
   四、重启（改数据目录后必须重启才能换库）
   ============================================================ */

#[tauri::command]
pub fn cmd_restart_app(app: AppHandle) -> Result<serde_json::Value, String> {
    // 走正常退出流程 → `RunEvent::Exit` 里的钩子会回收托管的 llama-server，
    // 不会留下孤儿进程占着上 GB 内存。
    app.request_restart();
    Ok(serde_json::json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wordwise-storage-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn norm_input_treats_blank_and_default_as_reset() {
        assert_eq!(norm_input(""), None);
        assert_eq!(norm_input("   \r\n"), None);
        assert_eq!(norm_input("default"), None);
        assert_eq!(norm_input("Default"), None);
        assert_eq!(norm_input("\u{feff}default"), None);
        assert_eq!(norm_input(" D:\\WW "), Some(PathBuf::from("D:\\WW")));
    }

    #[test]
    fn copy_tree_skips_existing_and_named_files() {
        let base = tmp("copy");
        let src = base.join("src");
        let dst = base.join("dst");
        fs::create_dir_all(src.join("models")).unwrap();
        fs::write(src.join("wordwise.db"), b"live-db").unwrap();
        fs::write(src.join("wordwise.db-wal"), b"wal").unwrap();
        fs::write(src.join("models_dir.txt"), b"D:\\elsewhere").unwrap();
        fs::write(src.join("notes.txt"), b"hello").unwrap();
        fs::write(src.join("models").join("a.gguf"), b"1234567890").unwrap();

        let (files, bytes) = copy_tree(
            &src,
            &dst,
            &["wordwise.db", "wordwise.db-wal", "wordwise.db-shm", "models_dir.txt"],
        )
        .unwrap();

        assert_eq!(files, 2, "只搬 notes.txt 和 models/a.gguf");
        assert_eq!(bytes, 15);
        assert!(dst.join("notes.txt").is_file());
        assert!(dst.join("models").join("a.gguf").is_file());
        assert!(!dst.join("wordwise.db").exists(), "活动数据库不能按文件拷");
        assert!(!dst.join("models_dir.txt").exists(), "模型指针要重算，不能照抄");

        // 再跑一次：同名文件跳过，不重复搬
        let (files2, bytes2) =
            copy_tree(&src, &dst, &["wordwise.db", "wordwise.db-wal", "models_dir.txt"]).unwrap();
        assert_eq!((files2, bytes2), (0, 0), "已存在的文件必须跳过，不覆盖");
    }

    #[test]
    fn dst_existing_file_is_not_overwritten() {
        let base = tmp("no-overwrite");
        let src = base.join("src");
        let dst = base.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("keep.txt"), b"new").unwrap();
        fs::write(dst.join("keep.txt"), b"old").unwrap();

        copy_tree(&src, &dst, &[]).unwrap();
        assert_eq!(fs::read_to_string(dst.join("keep.txt")).unwrap(), "old");
    }

    #[test]
    fn nesting_is_rejected() {
        let cur = PathBuf::from("D:\\WW");
        // 自己复制自己
        assert!(reject_nesting(&cur, &PathBuf::from("D:\\WW\\sub"), "数据目录").is_err());
        // 反过来也不行
        assert!(reject_nesting(&cur, &PathBuf::from("D:\\"), "数据目录").is_err());
        // 平级可以
        assert!(reject_nesting(&cur, &PathBuf::from("E:\\WW"), "数据目录").is_ok());
        // 前缀相同但不是子目录，不能被误杀
        assert!(reject_nesting(&cur, &PathBuf::from("D:\\WW2"), "数据目录").is_ok());
    }

    #[test]
    fn dir_size_counts_recursively() {
        let base = tmp("size");
        fs::create_dir_all(base.join("a").join("b")).unwrap();
        fs::write(base.join("x.bin"), vec![0u8; 100]).unwrap();
        fs::write(base.join("a").join("y.bin"), vec![0u8; 50]).unwrap();
        fs::write(base.join("a").join("b").join("z.bin"), vec![0u8; 25]).unwrap();

        let (bytes, files) = dir_size(&base);
        assert_eq!(bytes, 175);
        assert_eq!(files, 3);
    }

    #[test]
    fn dir_size_on_missing_dir_is_zero() {
        assert_eq!(dir_size(Path::new("Z:\\definitely\\not\\here")), (0, 0));
    }

    #[test]
    fn write_pointer_creates_parent_dirs() {
        let base = tmp("pointer");
        let f = base.join("deep").join("nested").join(LOCATION_FILE);
        write_pointer(&f, "D:\\WW").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "D:\\WW");
        // 空内容 = 恢复默认：`read_pointer` 必须认出来
        write_pointer(&f, "").unwrap();
        assert_eq!(state::read_pointer(&f), None);
    }

    #[test]
    fn ensure_writable_reports_failure_on_a_path_that_is_a_file() {
        let base = tmp("writable");
        let f = base.join("not-a-dir");
        fs::write(&f, b"x").unwrap();
        assert!(ensure_writable(&f).is_err(), "把文件当目录用必须报错，不能静默通过");
    }

    #[test]
    fn drives_json_shape_is_stable() {
        // 非 Windows 上就是空数组；Windows 上至少有 C 盘。
        // 这里只断言「不 panic、返回数组」，因为 CI/开发机的盘符组合不可控。
        let v = drives_json();
        if cfg!(windows) {
            assert!(!v.is_empty(), "Windows 上不可能一个盘都没有");
            for d in &v {
                assert!(d.get("root").is_some());
                assert!(d.get("free_text").is_some());
            }
        } else {
            assert!(v.is_empty());
        }
    }
}
