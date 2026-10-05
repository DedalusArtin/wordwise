use std::path::PathBuf;

/// 声明前端目录，让 Cargo 在其变化时重跑本构建脚本。
///
/// 为什么必须手动做这件事：
///   `tauri.conf.json` 里 `frontendDist = "../src"`，位于 **`src-tauri` 包目录之外**。
///   `tauri_build::build()` 只会声明两个 `rerun-if-changed`：
///     - `src-tauri/tauri.conf.json`
///     - `src-tauri/capabilities`
///   它**不会**声明 `frontendDist`。
///   而 Cargo 默认只监测「包目录内的文件」，包外的 `../src` 完全不在视野里。
///
/// 后果（曾真实发生）：改了 `src/index.html` 或 `src/js/*.js` 之后跑
/// `cargo build --release`，Cargo 判定"没有变化"直接跳过，exe 里内嵌的
/// **仍是旧前端**——新增的 `set-enable-proxy`、`syncProxyFields` 等标识符
/// 在产物里一个都搜不到，而 Rust 侧的改动却已生效，极易误判为"已编译最新"。
///
/// 这里显式声明整棵前端目录树，前端一改就必然重建。
fn declare_frontend_rerun(frontend: &PathBuf) {
    if !frontend.is_dir() {
        println!(
            "cargo:warning=前端目录不存在，跳过 rerun-if-changed 声明：{}",
            frontend.display()
        );
        return;
    }

    // 目录本身也声明一次（Cargo 支持对目录整体监测）
    println!("cargo:rerun-if-changed={}", frontend.display());

    let mut stack = vec![frontend.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            println!("cargo:rerun-if-changed={}", path.display());
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
}

fn main() {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("src");
    declare_frontend_rerun(&frontend);

    tauri_build::build()
}
