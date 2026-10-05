# ============================================================
#  获取随包分发的 llama.cpp 推理引擎 → vendor\llama
#
#  为什么需要这个脚本：
#    vendor\llama（约 118 MB 的第三方二进制）**不进 git**，所以任何一次
#    干净的 clone 都缺引擎。缺了不会编译失败，而是「一键部署」静默回退到
#    联网下载 —— 那条路在国内被限速到约 40 KB/s，33 MB 要下十几分钟。
#    因此本地开发和 CI 都需要一个可靠的方式把它补齐。
#
#  用法（项目根目录）：
#    .\scripts\fetch_engine.ps1                  下载并解压（版本号自动从源码读）
#    .\scripts\fetch_engine.ps1 -Force           已存在也重新拉一遍
#    .\scripts\fetch_engine.ps1 -UseMirror       走 gh-proxy 镜像（国内直连 GitHub 不通时）
#    .\scripts\fetch_engine.ps1 -ZipPath D:\x.zip   用本地已有的压缩包，完全不联网
#    .\scripts\fetch_engine.ps1 -Arch arm64      取 ARM64 包（默认 x64）
#
#  退出码：0 = 引擎就位；1 = 失败（调用方应当中止构建）
#
#  说明：
#    - 版本号只有 **一个** 来源：src-tauri\src\localllm\mod.rs 的 ENGINE_TAG。
#      这里刻意不重复写死版本号，免得源码升级了、包名还是旧的 → 下载 404。
#    - 解压时**原样放出压缩包里的全部文件**，不做裁剪。
#      上游包里另有 llama-cli / llama-bench 等一堆命令行工具和它们各自的
#      *-impl.dll（约占 2 MB 解压后体积，压缩后不到 1 MB），这些**不是**
#      llama-server.exe 的运行时依赖。本机早先 vendor\llama 是手工裁到 23 个
#      文件的，但那套「留哪些」的规则没写下来、也无从校验；一旦漏掉一个
#      真依赖，表现是引擎启动失败而不是构建报错，很难查。
#      所以这里宁可贵一点也要完整：解压即所得，不会有漏项。
#    - 本文件必须保存为「UTF-8 with BOM」，否则 Windows PowerShell 5.1
#      会把中文按 ANSI 解码导致语法错误。
# ============================================================

[CmdletBinding()]
param(
    # 引擎版本标记（如 b11414）。留空则从 Rust 源码读 ENGINE_TAG。
    [string]$Tag = "",
    [ValidateSet("x64", "arm64")]
    [string]$Arch = "x64",
    # 引擎落到哪里，默认 <项目根>\vendor\llama
    [string]$OutDir = "",
    # 显式指定下载地址（默认用 llama.cpp 官方 release）
    [string]$Url = "",
    # 用本地已有的 zip，不联网
    [string]$ZipPath = "",
    # 走国内镜像前缀（与 src-tauri\src\localllm\mod.rs 的 ENGINE_MIRRORS 同源）
    [switch]$UseMirror,
    # 已存在也重来一遍
    [switch]$Force
)

$ErrorActionPreference = "Stop"
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

# 关掉进度条：Expand-Archive 会为每个文件画一次进度条，CI 日志会被刷成
# 几千行；PS 5.1 下绘制进度条本身也会明显拖慢解压。
$ProgressPreference = "SilentlyContinue"

function Write-Step($m) { Write-Host ""; Write-Host "==> $m" -ForegroundColor Cyan }
function Write-Ok($m)   { Write-Host "  [OK] $m" -ForegroundColor Green }
function Write-Warn2($m){ Write-Host "  [!] $m" -ForegroundColor Yellow }
function Write-Err2($m) { Write-Host "  [X] $m" -ForegroundColor Red }
function Write-Dim($m)  { Write-Host "     $m" -ForegroundColor DarkGray }

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Definition)
if (-not $OutDir) { $OutDir = Join-Path $Root "vendor\llama" }

# ---------- 1. 确定版本号 ----------
Write-Step "确定引擎版本"

$modRs = Join-Path $Root "src-tauri\src\localllm\mod.rs"
if (-not $Tag) {
    if (-not (Test-Path $modRs)) {
        Write-Err2 "找不到 $modRs，无法读取 ENGINE_TAG；可用 -Tag 显式指定"
        exit 1
    }
    $raw = Get-Content $modRs -Raw -Encoding UTF8
    if ($raw -match 'pub const ENGINE_TAG:\s*&str\s*=\s*"([^"]+)"') {
        $Tag = $Matches[1]
    } else {
        Write-Err2 "在 localllm\mod.rs 里没匹配到 ENGINE_TAG 常量"
        Write-Dim "期望形如： pub const ENGINE_TAG: &str = `"b11414`";"
        exit 1
    }
}
Write-Ok "ENGINE_TAG = $Tag"

# 包名规则必须与 Rust 侧的 engine_asset() 保持一致
$asset = "llama-$Tag-bin-win-vulkan-$Arch.zip"

# ---------- 2. 已经就位就不折腾 ----------
$serverExe = Join-Path $OutDir "llama-server.exe"
if ((Test-Path $serverExe) -and -not $Force) {
    $mb = [math]::Round(((Get-ChildItem $OutDir -File | Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
    Write-Ok "引擎已存在：$OutDir（$mb MB），跳过"
    $stale = Get-ChildItem $OutDir -Filter "llama-b*-bin-win-vulkan-*.zip" -File -ErrorAction SilentlyContinue |
             Where-Object { $_.Name -ne $asset }
    if ($stale) {
        foreach ($s in $stale) { Write-Warn2 "注意：还留着别的版本的包 $($s.Name)" }
    }
    Write-Dim "要强制重来：加 -Force"
    exit 0
}

# ---------- 3. 拿到 zip ----------
Write-Step "获取引擎包 $asset"

$tmpZip = Join-Path ([System.IO.Path]::GetTempPath()) $asset

if ($ZipPath) {
    if (-not (Test-Path $ZipPath)) {
        Write-Err2 "-ZipPath 指向的文件不存在：$ZipPath"
        exit 1
    }
    Copy-Item $ZipPath $tmpZip -Force
    Write-Ok "使用本地压缩包：$ZipPath"
} else {
    # 官方地址；-UseMirror 时改成镜像前缀（国内直连 GitHub 不通时用）
    if (-not $Url) {
        $official = "https://github.com/ggml-org/llama.cpp/releases/download/$Tag/$asset"
        if ($UseMirror) {
            $mirrors = @(
                "https://gh-proxy.com/https://github.com/ggml-org/llama.cpp/releases/download/$Tag/$asset",
                "https://ghfast.top/https://github.com/ggml-org/llama.cpp/releases/download/$Tag/$asset",
                "https://ghproxy.net/https://github.com/ggml-org/llama.cpp/releases/download/$Tag/$asset"
            )
        } else {
            $mirrors = @($official)
        }
    } else {
        $mirrors = @($Url)
    }

    # 下载大文件时一定要关进度条：Windows PowerShell 5.1 下绘制进度条会让
    # Invoke-WebRequest 慢一个数量级（33 MB 能从几秒拖到几分钟）。
    $prevProgress = $ProgressPreference
    $ProgressPreference = "SilentlyContinue"
    $ok = $false
    $lastErr = $null
    try {
        foreach ($u in $mirrors) {
            Write-Dim "尝试 $u"
            try {
                $args = @{ Uri = $u; OutFile = $tmpZip; TimeoutSec = 1800; ErrorAction = "Stop" }
                # PS 5.1 必须显式 -UseBasicParsing（否则依赖 IE 引擎）；
                # PS 7 里该参数已废弃但仍接受。这里先探测再决定，两个版本都安全。
                if ((Get-Command Invoke-WebRequest).Parameters.ContainsKey("UseBasicParsing")) {
                    $args["UseBasicParsing"] = $true
                }
                Invoke-WebRequest @args
                $ok = $true
                break
            } catch {
                $lastErr = $_.Exception.Message
                Write-Warn2 "失败：$lastErr"
            }
        }
    } finally {
        $ProgressPreference = $prevProgress
    }

    if (-not $ok) {
        Write-Err2 "引擎下载失败（所有地址都试过了）"
        Write-Dim "最后一次错误：$lastErr"
        Write-Host ""
        Write-Host "  三条出路，任选一条：" -ForegroundColor White
        Write-Host "    1) 换镜像重试：   .\scripts\fetch_engine.ps1 -UseMirror" -ForegroundColor White
        Write-Host "    2) 用本地文件：   .\scripts\fetch_engine.ps1 -ZipPath <已有的zip>" -ForegroundColor White
        Write-Host "    3) 手工指定地址： .\scripts\fetch_engine.ps1 -Url <可用的完整地址>" -ForegroundColor White
        Write-Dim "注意：镜像/镜像站在国内也时好时坏，若全部不通，就在能联网的机器上下好 zip 再拷过来。"
        Write-Host ""
        Write-Dim "缺引擎不会导致编译失败，只是便携版会「一键部署」回退到联网下载（很慢）。"
        exit 1
    }
}

$sizeMB = [math]::Round((Get-Item $tmpZip).Length / 1MB, 1)
Write-Ok "已获得 $asset（$sizeMB MB）"

# ---------- 4. 解压到 vendor\llama ----------
Write-Step "解压到 $OutDir"

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

# 先把非引擎文件留着（比如上一个版本的 zip），解压只覆盖同名文件
$extractTmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ww-engine-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $extractTmp -Force | Out-Null
try {
    Expand-Archive -Path $tmpZip -DestinationPath $extractTmp -Force

    # 压缩包目前是**平铺**的（llama-server.exe 就在根）；但万一上游哪天改成
    # 套一层目录，这里自动把那一层摊平，免得 build.ps1 找不到 llama-server.exe
    # 而静默产出一个不带引擎的便携包。
    $srcDir = $extractTmp
    if (-not (Test-Path (Join-Path $srcDir "llama-server.exe"))) {
        $nested = Get-ChildItem $extractTmp -Directory -ErrorAction SilentlyContinue |
                  Where-Object { Test-Path (Join-Path $_.FullName "llama-server.exe") } |
                  Select-Object -First 1
        if ($nested) {
            $srcDir = $nested.FullName
            Write-Dim "压缩包内有一层目录，已自动摊平：$($nested.Name)"
        }
    }

    if (-not (Test-Path (Join-Path $srcDir "llama-server.exe"))) {
        Write-Err2 "解压后仍找不到 llama-server.exe，包结构可能变了"
        exit 1
    }

    Get-ChildItem $srcDir -File | ForEach-Object { Copy-Item $_.FullName $OutDir -Force }
} finally {
    Remove-Item $extractTmp -Recurse -Force -ErrorAction SilentlyContinue
}

# 把 zip 也留在目录里（本机原来的做法），便于离线重装；build.ps1 只拷 .exe/.dll
$keepZip = Join-Path $OutDir $asset
if ($tmpZip -ne $keepZip) { Copy-Item $tmpZip $keepZip -Force -ErrorAction SilentlyContinue }

# ---------- 5. 验收 ----------
$dlls = @(Get-ChildItem $OutDir -Filter *.dll -File -ErrorAction SilentlyContinue)
$totalMB = [math]::Round(((Get-ChildItem $OutDir -File | Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
Write-Ok "llama-server.exe + $($dlls.Count) 个 DLL → $OutDir（$totalMB MB）"

if ($dlls.Count -lt 5) {
    Write-Warn2 "DLL 数量偏少（$($dlls.Count)），引擎可能不完整"
    Write-Dim "完整的 Vulkan 包应当同时带全套 ggml-cpu-*.dll 与 ggml-vulkan.dll"
}

Write-Host ""
Write-Host "引擎就位。下一步直接构建即可：" -ForegroundColor White
Write-Host "  .\build.ps1" -ForegroundColor White
Write-Host ""
exit 0
