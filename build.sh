#!/usr/bin/env bash
# ============================================================
#  WordWise 一键构建脚本（Git Bash / WSL）
#
#  用法：
#    ./build.sh                完整构建 + Inno Setup 打包
#    ./build.sh --clean        先清理再构建
#    ./build.sh --no-installer 跳过安装程序
#    ./build.sh --debug        构建 debug 版
#    ./build.sh --upload       构建后上传到 GitHub
# ============================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

# 清空代理：部分机器上的 HTTP_PROXY 指向不可用端口，会让 cargo 静默卡死。
# 如需保留代理（例如公司内网），设置 WORDWISE_KEEP_PROXY=1 即可跳过。
if [[ "${WORDWISE_KEEP_PROXY:-0}" != "1" ]]; then
  unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy
fi

CLEAN=0
SKIP_INSTALLER=0
DEBUG=0
UPLOAD=0

for arg in "$@"; do
  case "$arg" in
    --clean)         CLEAN=1 ;;
    --no-installer)  SKIP_INSTALLER=1 ;;
    --debug)         DEBUG=1 ;;
    --upload)        UPLOAD=1 ;;
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "未知参数：$arg"; exit 1 ;;
  esac
done

# ---------- 颜色 ----------
C_RESET=$'\033[0m'; C_CYAN=$'\033[36m'; C_GREEN=$'\033[32m'
C_YELLOW=$'\033[33m'; C_RED=$'\033[31m'; C_GRAY=$'\033[90m'

step()  { echo ""; echo "${C_CYAN}==> $1${C_RESET}"; }
ok()    { echo "  ${C_GREEN}[OK]${C_RESET} $1"; }
warn()  { echo "  ${C_YELLOW}[!]${C_RESET} $1"; }
err()   { echo "  ${C_RED}[X]${C_RESET} $1"; }

echo "${C_GRAY}================================================================${C_RESET}"
echo "  WordWise 构建脚本"
echo "${C_GRAY}================================================================${C_RESET}"

# ---------- 1. 定位 cargo ----------
step "检查 Rust 工具链"

CARGO_BIN=""

# 优先：环境变量显式指定
if [[ -n "${CARGO:-}" && -x "${CARGO:-}" ]]; then
  CARGO_BIN="$CARGO"
fi

# 其次：toolchain 内的真实二进制（rustup shim 在受限环境下可能是 0 字节）
if [[ -z "$CARGO_BIN" && -n "${RUSTUP_HOME:-}" && -d "$RUSTUP_HOME/toolchains" ]]; then
  for tc in "$RUSTUP_HOME"/toolchains/*/bin/cargo.exe "$RUSTUP_HOME"/toolchains/*/bin/cargo; do
    if [[ -s "$tc" ]]; then CARGO_BIN="$tc"; break; fi
  done
fi

# 再次：CARGO_HOME 下的 shim（校验非空）
if [[ -z "$CARGO_BIN" && -n "${CARGO_HOME:-}" ]]; then
  for c in "$CARGO_HOME/bin/cargo.exe" "$CARGO_HOME/bin/cargo"; do
    if [[ -s "$c" ]]; then CARGO_BIN="$c"; break; fi
  done
fi

# 最后：PATH 中的 cargo
if [[ -z "$CARGO_BIN" ]]; then
  if command -v cargo >/dev/null 2>&1; then CARGO_BIN="$(command -v cargo)"; fi
fi

if [[ -z "$CARGO_BIN" ]]; then
  err "未找到可用的 cargo，请先安装 Rust：https://rustup.rs/"
  exit 1
fi

# 让 cargo 能找到 rustc
export PATH="$(dirname "$CARGO_BIN"):$PATH"
if [[ -x "$(dirname "$CARGO_BIN")/rustc.exe" ]]; then
  export RUSTC="$(dirname "$CARGO_BIN")/rustc.exe"
elif [[ -x "$(dirname "$CARGO_BIN")/rustc" ]]; then
  export RUSTC="$(dirname "$CARGO_BIN")/rustc"
fi

ok "cargo: $CARGO_BIN"

# ---------- 2. 清理 ----------
if [[ $CLEAN -eq 1 ]]; then
  step "清理构建产物"
  rm -rf src-tauri/target installer/output 2>/dev/null || true
  ok "已清理"
fi

# ---------- 3. 校验前端 ----------
step "校验前端资源"
MISSING=()
for f in src/index.html src/css/app.css; do
  [[ -f "$f" ]] || MISSING+=("$f")
done
if [[ ${#MISSING[@]} -gt 0 ]]; then
  err "缺少文件：${MISSING[*]}"
  exit 1
fi
JS_COUNT=$(ls -1 src/js/*.js 2>/dev/null | wc -l)
ok "index.html + app.css + ${JS_COUNT} 个 JS 模块"

# ---------- 4. 构建 ----------
if [[ $DEBUG -eq 1 ]]; then
  step "编译 Rust 后端（debug）"
  "$CARGO_BIN" build
  EXE_DIR="src-tauri/target/debug"
else
  step "编译 Rust 后端（release）"
  "$CARGO_BIN" build --release
  EXE_DIR="src-tauri/target/release"
fi

EXE="$EXE_DIR/wordwise.exe"
[[ -f "$EXE" ]] || EXE="$EXE_DIR/wordwise"
if [[ ! -f "$EXE" ]]; then
  err "编译产物未生成"
  exit 1
fi
ok "生成 $EXE（$(du -h "$EXE" | cut -f1)）"

if [[ $DEBUG -eq 1 ]]; then
  echo ""
  echo "${C_GREEN}Debug 构建完成：$EXE${C_RESET}"
  exit 0
fi

# ---------- 5. Inno Setup ----------
if [[ $SKIP_INSTALLER -eq 1 ]]; then
  warn "已跳过安装程序打包"
  echo "${C_GREEN}构建完成：$EXE${C_RESET}"
  exit 0
fi

step "使用 Inno Setup 打包安装程序"

find_iscc() {
  local cands=(
    # 本机已确认可用的路径
    "/g/Programming/07-utils/Inno Setup 7/ISCC.exe"
    "/g/Programming/07-utils/Inno Setup 6/ISCC.exe"
    "/c/Users/$USER/AppData/Local/Programs/Inno Setup 6/ISCC.exe"
    "/c/Users/$USER/AppData/Local/Programs/Inno Setup 7/ISCC.exe"
    # 常规安装位置
    "/c/Program Files (x86)/Inno Setup 7/ISCC.exe"
    "/c/Program Files/Inno Setup 7/ISCC.exe"
    "/c/Program Files (x86)/Inno Setup 6/ISCC.exe"
    "/c/Program Files/Inno Setup 6/ISCC.exe"
    "/g/tools_office/Inno Setup 6/ISCC.exe"
  )
  for c in "${cands[@]}"; do
    [[ -f "$c" ]] && { echo "$c"; return 0; }
  done
  # 兜底：从 PATH 或注册表查找
  command -v ISCC.exe 2>/dev/null || command -v iscc 2>/dev/null || return 1
}

if ! ISCC="$(find_iscc)"; then
  warn "未找到 Inno Setup 的 ISCC.exe，已跳过安装程序打包"
  echo "  ${C_GRAY}下载：https://jrsoftware.org/isdl.php${C_RESET}"
  echo ""
  echo "${C_GREEN}可执行文件已生成：$EXE${C_RESET}"
  exit 0
fi

ok "ISCC: $ISCC"
RELEASE_ABS="$(cd "$EXE_DIR" && pwd -W 2>/dev/null || cd "$EXE_DIR" && pwd)"
"$ISCC" "/DMySourceDir=$RELEASE_ABS" "installer/wordwise.iss"

SETUP=$(ls -t installer/output/*-Setup-*.exe 2>/dev/null | head -1 || true)
if [[ -n "$SETUP" ]]; then
  ok "安装程序：$SETUP（$(du -h "$SETUP" | cut -f1)）"
else
  warn "未找到安装程序产物"
fi

# ---------- 6. 上传 ----------
if [[ $UPLOAD -eq 1 ]]; then
  step "上传到 GitHub"
  if command -v python >/dev/null 2>&1; then
    python scripts/upload_github.py --public
  elif command -v python3 >/dev/null 2>&1; then
    python3 scripts/upload_github.py --public
  else
    err "未找到 Python"
  fi
fi

echo ""
echo "${C_GRAY}================================================================${C_RESET}"
echo "${C_GREEN}  构建完成${C_RESET}"
echo "${C_GRAY}================================================================${C_RESET}"
[[ -n "${SETUP:-}" ]] && echo "  安装程序：$SETUP"
echo "  可执行文件：$EXE"
