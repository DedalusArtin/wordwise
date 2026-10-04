# WordWise 一键构建脚本（PowerShell）
#
# 用法：
#   .\build.ps1                  完整构建 + Inno Setup 打包
#   .\build.ps1 -SkipInstaller   只构建 exe，跳过安装程序
#   .\build.ps1 -Clean           构建前先清理
#   .\build.ps1 -Debug           构建 debug 版（更快，便于调试）
#   .\build.ps1 -Upload          构建完成后自动上传到 GitHub

[CmdletBinding()]
param(
    [switch]$Clean,
    [switch]$SkipInstaller,
    [switch]$Debug,
    [switch]$Upload,
    [string]$IsccPath = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $Root

# 清空代理：部分机器上的 HTTP_PROXY 指向不可用端口，会让 cargo 静默卡死。
# 如需保留代理（例如公司内网），设置环境变量 WORDWISE_KEEP_PROXY=1 即可跳过。
if ($env:WORDWISE_KEEP_PROXY -ne "1") {
    $env:HTTP_PROXY  = ""
    $env:HTTPS_PROXY = ""
    $env:http_proxy  = ""
    $env:https_proxy = ""
    $env:ALL_PROXY   = ""
    $env:all_proxy   = ""
}

# ---------- 输出辅助 ----------
function Write-Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn2($msg){ Write-Host "  [!] $msg" -ForegroundColor Yellow }
function Write-Err($msg)  { Write-Host "  [X] $msg" -ForegroundColor Red }

Write-Host "=" * 64 -ForegroundColor DarkGray
Write-Host "  WordWise 构建脚本" -ForegroundColor White
Write-Host "=" * 64 -ForegroundColor DarkGray

# ---------- 1. 定位 cargo ----------
Write-Step "检查 Rust 工具链"

$cargoCandidates = @()
if ($env:CARGO) { $cargoCandidates += $env:CARGO }
if ($env:CARGO_HOME) { $cargoCandidates += (Join-Path $env:CARGO_HOME "bin\cargo.exe") }
# rustup 的 shim 可能为 0 字节（受限环境），因此优先直接使用 toolchain 内的真实二进制
if ($env:RUSTUP_HOME) {
    $tcRoot = Join-Path $env:RUSTUP_HOME "toolchains"
    if (Test-Path $tcRoot) {
        Get-ChildItem $tcRoot -Directory | ForEach-Object {
            $cargoCandidates += (Join-Path $_.FullName "bin\cargo.exe")
        }
    }
}
$cargoCandidates += "cargo"

$Cargo = $null
foreach ($c in $cargoCandidates) {
    try {
        if ($c -eq "cargo") {
            $null = & cargo --version 2>$null
        } elseif (Test-Path $c) {
            if ((Get-Item $c).Length -eq 0) { continue }   # 跳过 0 字节 shim
            $null = & $c --version 2>$null
        } else { continue }
        if ($LASTEXITCODE -eq 0) { $Cargo = $c; break }
    } catch { continue }
}

if (-not $Cargo) {
    Write-Err "未找到可用的 cargo。"
    Write-Host "  请安装 Rust：https://rustup.rs/" -ForegroundColor Gray
    Write-Host "  或设置环境变量 CARGO_HOME / RUSTUP_HOME" -ForegroundColor Gray
    exit 1
}

$rustVer = (& $Cargo --version) -join " "
Write-Ok "使用 $Cargo"
Write-Ok $rustVer

# 同步给 rustc，确保 cargo 能找到编译器
$tcBin = Split-Path -Parent $Cargo
$env:RUSTC = Join-Path $tcBin "rustc.exe"
$env:PATH = "$tcBin;$env:PATH"

# ---------- 2. 清理 ----------
if ($Clean) {
    Write-Step "清理构建产物"
    $targets = @(
        "src-tauri\target",
        "installer\output"
    )
    foreach ($t in $targets) {
        if (Test-Path $t) {
            Remove-Item $t -Recurse -Force -ErrorAction SilentlyContinue
            Write-Ok "已删除 $t"
        }
    }
}

# ---------- 3. 构建前端校验 ----------
Write-Step "校验前端资源"
$frontend = @("src\index.html", "src\css\app.css") + (Get-ChildItem "src\js\*.js" | ForEach-Object { $_.FullName })
$missing = @()
foreach ($f in $frontend) { if (-not (Test-Path $f)) { $missing += $f } }
if ($missing.Count -gt 0) {
    Write-Err "缺少前端文件：$($missing -join ', ')"
    exit 1
}
$jsCount = (Get-ChildItem "src\js\*.js").Count
Write-Ok "index.html + app.css + $jsCount 个 JS 模块"

# ---------- 4. 构建 ----------
$profile = if ($Debug) { "dev" } else { "release" }
Write-Step "编译 Rust 后端（$profile）"
if ($Debug) {
    & $Cargo build
    $exeDir = "src-tauri\target\debug"
} else {
    & $Cargo build --release
    $exeDir = "src-tauri\target\release"
}
if ($LASTEXITCODE -ne 0) {
    Write-Err "编译失败"
    exit 1
}
$exe = Join-Path $exeDir "wordwise.exe"
if (-not (Test-Path $exe)) {
    Write-Err "未生成 $exe"
    exit 1
}
$sizeMB = [math]::Round((Get-Item $exe).Length / 1MB, 2)
Write-Ok "生成 $exe（$sizeMB MB）"

if ($Debug) {
    Write-Host "`nDebug 构建完成。运行：`n  $exe" -ForegroundColor Green
    exit 0
}

# ---------- 5. Inno Setup 打包 ----------
if ($SkipInstaller) {
    Write-Warn2 "已跳过安装程序打包"
    Write-Host "`n构建完成：$exe" -ForegroundColor Green
    exit 0
}

Write-Step "使用 Inno Setup 打包安装程序"

function Find-Iscc {
    param([string]$Explicit)
    if ($Explicit -and (Test-Path $Explicit)) { return $Explicit }
    $cands = @(
        # 本机已确认可用的路径
        "G:\Programming\07-utils\Inno Setup 7\ISCC.exe",
        "G:\Programming\07-utils\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        # 常规安装位置
        "C:\Program Files (x86)\Inno Setup 7\ISCC.exe",
        "C:\Program Files\Inno Setup 7\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe",
        "G:\tools_office\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $cands) { if (Test-Path $c) { return $c } }
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

$Iscc = Find-Iscc -Explicit $IsccPath
if (-not $Iscc) {
    Write-Warn2 "未找到 Inno Setup 的 ISCC.exe，已跳过安装程序打包"
    Write-Host "  下载地址：https://jrsoftware.org/isdl.php" -ForegroundColor Gray
    Write-Host "  安装后重新运行，或用 -IsccPath 指定路径" -ForegroundColor Gray
    Write-Host "`n可执行文件已生成：$exe" -ForegroundColor Green
    exit 0
}

Write-Ok "ISCC: $Iscc"

# 传入实际的 release 目录，避免 .iss 内的相对路径失配
$releaseAbs = (Resolve-Path "src-tauri\target\release").Path
& $Iscc "/DMySourceDir=$releaseAbs" "installer\wordwise.iss"
if ($LASTEXITCODE -ne 0) {
    Write-Err "Inno Setup 打包失败"
    exit 1
}

$setup = Get-ChildItem "installer\output\*-Setup-*.exe" -ErrorAction SilentlyContinue |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($setup) {
    $setupMB = [math]::Round($setup.Length / 1MB, 2)
    Write-Ok "安装程序：$($setup.FullName)（$setupMB MB）"
} else {
    Write-Warn2 "未找到安装程序产物，请检查 installer\output"
}

# ---------- 6. 上传 ----------
if ($Upload) {
    Write-Step "上传到 GitHub"
    $py = $null
    foreach ($p in @("python", "py")) {
        try { $null = & $p --version 2>$null; if ($LASTEXITCODE -eq 0) { $py = $p; break } } catch {}
    }
    if (-not $py) {
        Write-Err "未找到 Python，无法执行上传脚本"
    } else {
        & $py "scripts\upload_github.py" --public
    }
}

Write-Host "`n" + ("=" * 64) -ForegroundColor DarkGray
Write-Host "  构建完成" -ForegroundColor Green
Write-Host ("=" * 64) -ForegroundColor DarkGray
if ($setup) { Write-Host "  安装程序：$($setup.FullName)" -ForegroundColor White }
Write-Host "  可执行文件：$exe" -ForegroundColor White
