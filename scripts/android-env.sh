#!/usr/bin/env bash
# WordWise —— Android 构建环境变量
#
# 用法：source scripts/android-env.sh
#
# 为什么单独抽一个文件：
#   Tauri 的 Android 构建要从三个不同的地方找工具 —— Android SDK（含 NDK）、
#   JDK 17、以及 Rust 的 Android target。这三样都不在系统 PATH 里，
#   每次手动敲不仅容易漏，漏了之后报的错还和真正的原因差很远
#   （例如缺 JAVA_HOME 时 Gradle 报的是 "Unsupported class file major
#   version"，看上去像 JDK 版本不对，其实是根本没找到 JDK）。
#
# 工具链统一装在 G:\Programming\01-toolchains 与 04-runtimes 下，
# 与 C 盘系统环境隔离，删掉目录就是干净卸载。
#
# ★ PATH 里 **工具链的 bin 必须排在 cargo 的 bin 前面**：
#   G:\Programming\...\cargo\bin 下的 cargo.exe 是 rustup 的分发壳，
#   在没有 RUSTUP_HOME 的 shell 里会静默失败（退出码 0，但什么都不输出），
#   这种失败最难排查。工具链目录里的 cargo 是真身，直接用它。

# ★ 必须写成 Windows 盘符形式（G:/...），不能写成 Git Bash 的 /g/...。
#   tauri-cli 内部用 Win32 的 CreateProcess 去 spawn `cargo`，而环境变量是
#   原样继承的——`/g/...` 这样的路径 Windows 解析不了，报错是
#   `%1 is not a valid Win32 application. (os error 193)`，
#   看上去像二进制损坏，其实是路径格式不对。
export RUSTUP_HOME=G:/Programming/01-toolchains/rust/rustup
export CARGO_HOME=G:/Programming/01-toolchains/rust/cargo

export ANDROID_HOME=G:/Programming/01-toolchains/android/sdk
export NDK_HOME=G:/Programming/01-toolchains/android/sdk/ndk/26.3.11579264
export JAVA_HOME=G:/Programming/04-runtimes/java/jdk

export PATH="$RUSTUP_HOME/toolchains/stable-x86_64-pc-windows-msvc/bin:$CARGO_HOME/bin:$PATH"

# 自检：四样东西都必须在
_ww_android_env_check() {
  local missing=0
  for v in RUSTUP_HOME CARGO_HOME ANDROID_HOME NDK_HOME JAVA_HOME; do
    if [ ! -d "${!v}" ]; then echo "[缺失] $v -> ${!v}"; missing=1; fi
  done
  command -v cargo >/dev/null 2>&1 || { echo "[缺失] cargo 不在 PATH"; missing=1; }
  command -v cargo-tauri >/dev/null 2>&1 || { echo "[缺失] cargo-tauri 不在 PATH（cargo install tauri-cli --version '^2' --locked）"; missing=1; }
  if [ "$missing" = "0" ]; then
    echo "Android 构建环境就绪：cargo $(cargo --version | awk '{print $2}') / tauri $(cargo tauri --version | awk '{print $2}') / ndk $(basename "$NDK_HOME")"
  else
    echo "Android 构建环境不完整，见上方 [缺失] 项"
  fi
  return "$missing"
}
_ww_android_env_check
