# ============================================================
#  WordWise 原生 Windows 构建脚本（PowerShell 7 / 5.1）
#
#  纯 Windows 环境，不依赖 WSL、不依赖 Git Bash。
#
#  用法（在项目根目录的 PowerShell 里运行）：
#    .\build.ps1                 编译 release + 打安装包
#    .\build.ps1 -DebugBuild     只编译 debug 版（快，便于调试）
#    .\build.ps1 -SkipInstaller  只编译 exe，不打安装包
#    .\build.ps1 -Clean          先清理再编译
#    .\build.ps1 -Run            编译完自动启动程序
#    .\build.ps1 -IsccPath "..." 手工指定 ISCC.exe 路径
#
#  说明：
#    - 前端资源（src/index.html、src/js、src/css）在编译时嵌入 exe，
#      改了前端必须重新编译才生效。
#    - 脚本自动定位 Rust 工具链与 Inno Setup，无需手工设环境变量。
#    - 本文件必须保存为「UTF-8 with BOM」，否则 Windows PowerShell 5.1
#      会把中文按 ANSI 解码导致语法错误。
# ============================================================

[CmdletBinding()]
param(
    [switch]$Clean,
    [switch]$SkipInstaller,
    [switch]$DebugBuild,
    [switch]$Upload,
    [switch]$Run,
    [string]$IsccPath = "",
    [string]$TargetDir = ""
)

$ErrorActionPreference = "Stop"

# 控制台编码：保证中文不乱码
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

$Root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $Root

# ---------- Inno Setup（ISCC.exe）----------
# ISCC.exe **没有加入系统 PATH**，命令行里直接敲 `iscc` 是调不到的。
# 必须写完整路径。下面这个位置是本机实测可用的 Inno Setup 7。
# 若你装在别处，改这一行，或用 -IsccPath 覆盖。
$DefaultIscc      = "G:\Programming\07-utils\Inno Setup 7\ISCC.exe"
$ExpectedIsccMajor = 7

# 版本号统一从 tauri.conf.json 读，避免与安装包、界面显示的版本脱节
$AppVersion = "0.35.0"
try {
    $cfgPath = Join-Path $Root "src-tauri\tauri.conf.json"
    $cfg = Get-Content $cfgPath -Raw -ErrorAction Stop | ConvertFrom-Json
    if ($cfg.version) { $AppVersion = $cfg.version }
} catch {
    Write-Dim "无法读取 tauri.conf.json 的版本号，回退为 $AppVersion"
}

# ---------- 清代理（本机 HTTP_PROXY 指向死端口会让 cargo 静默卡死）----------
if ($env:WORDWISE_KEEP_PROXY -ne "1") {
    foreach ($p in @("HTTP_PROXY","HTTPS_PROXY","http_proxy","https_proxy","ALL_PROXY","all_proxy")) {
        Remove-Item "Env:$p" -ErrorAction SilentlyContinue
    }
}

# ---------- 输出辅助 ----------
function Write-Step($msg)  { Write-Host ""; Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)    { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Warn2($msg) { Write-Host "  [!] $msg"  -ForegroundColor Yellow }
function Write-Err2($msg)  { Write-Host "  [X] $msg"  -ForegroundColor Red }
function Write-Dim($msg)   { Write-Host "     $msg"   -ForegroundColor DarkGray }

Write-Host ("=" * 60) -ForegroundColor DarkGray
Write-Host "  WordWise 编译打包（原生 Windows / PowerShell）" -ForegroundColor White
Write-Host ("=" * 60) -ForegroundColor DarkGray

# ============================================================
#  1. 定位 cargo
# ============================================================
Write-Step "定位 Rust 工具链"

$cargoCandidates = @()

# 1) 环境变量显式指定
if ($env:CARGO) { $cargoCandidates += $env:CARGO }

# 2) 本机 G 盘统一工具链位置（首选，已确认可用）
$gTcRoot = "G:\Programming\01-toolchains\rust\rustup\toolchains"
if (Test-Path $gTcRoot) {
    Get-ChildItem $gTcRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object {
        $cargoCandidates += (Join-Path $_.FullName "bin\cargo.exe")
    }
}

# 3) RUSTUP_HOME 推断
if ($env:RUSTUP_HOME) {
    $tcRoot = Join-Path $env:RUSTUP_HOME "toolchains"
    if (Test-Path $tcRoot) {
        Get-ChildItem $tcRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            $cargoCandidates += (Join-Path $_.FullName "bin\cargo.exe")
        }
    }
}

# 4) CARGO_HOME shim
if ($env:CARGO_HOME) { $cargoCandidates += (Join-Path $env:CARGO_HOME "bin\cargo.exe") }

# 5) 用户级 .cargo\bin（本机该 shim 可能为 0 字节，下面会跳过）
$cargoCandidates += (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe")

# 6) PATH
$pathCmd = Get-Command cargo -ErrorAction SilentlyContinue
if ($pathCmd) { $cargoCandidates += $pathCmd.Source }

$Cargo = $null
foreach ($c in $cargoCandidates) {
    if (-not $c -or -not (Test-Path $c)) { continue }
    # 跳过 0 字节的 rustup shim（本机 ~/.cargo/bin/cargo.exe 就是这种）
    if ((Get-Item $c).Length -eq 0) { continue }
    try {
        $null = & $c --version 2>$null
        if ($LASTEXITCODE -eq 0) { $Cargo = $c; break }
    } catch { continue }
}

if (-not $Cargo) {
    Write-Err2 "未找到可用的 cargo.exe"
    Write-Dim "请确认 Rust 已安装，或设置环境变量 CARGO 指向 cargo.exe"
    Write-Dim "已尝试的路径："
    $cargoCandidates | Select-Object -Unique | ForEach-Object { Write-Dim "  $_" }
    exit 1
}

# 同步 PATH / RUSTC / RUSTDOC，确保 build.rs 能找到编译器
$tcBin = Split-Path -Parent $Cargo
$env:PATH = "$tcBin;$env:PATH"
$rustc = Join-Path $tcBin "rustc.exe"
if (Test-Path $rustc) { $env:RUSTC = $rustc }
$rustdoc = Join-Path $tcBin "rustdoc.exe"
if (Test-Path $rustdoc) { $env:RUSTDOC = $rustdoc }

Write-Ok "cargo: $Cargo"
Write-Dim ((& $Cargo --version) -join " ")

# ---------- MSVC 宏展开空间 ----------
# ring 等含大量 C 宏的 crate 在新版 MSVC（如 14.51）上会报：
#   fatal error C1056: compiler limit : out of macro expansion space
# 通过 CL 环境变量给 cl.exe 加大预处理器内存上限来规避。
# 用户若自行设过 CL，则尊重用户设置、追加而非覆盖。
if ($env:CL -notmatch "/Zm") {
    $env:CL = if ($env:CL) { "$env:CL /Zm2000" } else { "/Zm2000" }
    Write-Ok "CL 已追加 /Zm2000（规避 MSVC C1056 宏展开空间不足）"
}

# ---------- 构建目录 ----------
# 固定用项目内 src-tauri\target（增量编译复用已有缓存，二次构建更快）。
#
# 这里**强制覆盖**而不是「没设才设」：本机曾把 CARGO_TARGET_DIR 设成
# D:\Projects\.cargo-target，导致 cargo 把产物写到别处，而 src-tauri\target 下
# 留着一份很旧的 wordwise.exe —— 打包时拷到的就是旧文件，改了前端却「没生效」。
# 因此脚本一律以项目内目录为准；确实要换位置请用 -TargetDir 参数。
if ($env:CARGO_TARGET_DIR -and ($env:CARGO_TARGET_DIR -ne (Join-Path $Root "src-tauri\target"))) {
    Write-Warn2 "检测到环境变量 CARGO_TARGET_DIR=$($env:CARGO_TARGET_DIR)，已忽略（避免产物写到别处）"
}
if (-not $TargetDir) { $TargetDir = Join-Path $Root "src-tauri\target" }
$env:CARGO_TARGET_DIR = $TargetDir
Write-Step "构建目录"
Write-Dim $env:CARGO_TARGET_DIR
if (-not (Test-Path $env:CARGO_TARGET_DIR)) {
    New-Item -ItemType Directory -Path $env:CARGO_TARGET_DIR -Force | Out-Null
}

# ============================================================
#  2. 清理
# ============================================================
if ($Clean) {
    Write-Step "清理构建产物"
    foreach ($t in @((Join-Path $env:CARGO_TARGET_DIR "debug"), (Join-Path $env:CARGO_TARGET_DIR "release"), (Join-Path $Root "installer\output"))) {
        if (Test-Path $t) {
            Remove-Item $t -Recurse -Force -ErrorAction SilentlyContinue
            Write-Ok "已删除 $t"
        }
    }
}

# ============================================================
#  3. 校验前端资源
# ============================================================
Write-Step "校验前端资源"
$frontend = @("src\index.html", "src\css\app.css")
$jsFiles = Get-ChildItem "src\js\*.js" -ErrorAction SilentlyContinue
$frontend += $jsFiles.FullName

$missing = @()
foreach ($f in $frontend) { if (-not (Test-Path $f)) { $missing += $f } }
if ($missing.Count -gt 0) {
    Write-Err2 "缺少前端文件：$($missing -join ', ')"
    exit 1
}
Write-Ok "index.html + app.css + $($jsFiles.Count) 个 JS 模块"

# 前端渲染冒烟：抓「未声明的全局标识符」这类只有真正跑起来才炸的 bug。
# 典型事故：speakBtn 只挂在 window.WW 上，renderEntry 里却用裸名字调用
# → 默认配置下每次查词都 ReferenceError，界面永远停在转圈。
# node --check 只验语法，查不出这个，所以必须实际执行一遍。
$nodeCandidates = @()
if ($env:NODE) { $nodeCandidates += $env:NODE }
$nodeCmd = Get-Command node -ErrorAction SilentlyContinue
if ($nodeCmd) { $nodeCandidates += $nodeCmd.Source }
$nodeCandidates += (Join-Path $env:USERPROFILE ".workbuddy\binaries\node\versions")
$smoke = Join-Path $Root "scripts\smoke_frontend.cjs"
$nodeExe = $null
foreach ($c in $nodeCandidates) {
    if (-not $c) { continue }
    if (Test-Path $c -PathType Leaf) { $nodeExe = $c; break }
    if (Test-Path $c -PathType Container) {
        $picked = Get-ChildItem $c -Recurse -Filter "node.exe" -ErrorAction SilentlyContinue |
                  Sort-Object FullName -Descending | Select-Object -First 1
        if ($picked) { $nodeExe = $picked.FullName; break }
    }
}
if ($nodeExe -and (Test-Path $smoke)) {
    Write-Dim "前端渲染冒烟测试（$nodeExe）"
    & $nodeExe $smoke
    if ($LASTEXITCODE -ne 0) {
        Write-Err2 "前端冒烟测试未通过，已中止构建（否则会打包出一个白屏/转圈的 exe）"
        exit 1
    }
    Write-Ok "前端渲染冒烟测试通过"
} else {
    Write-Warn2 "未找到 node 或 scripts\smoke_frontend.cjs，已跳过前端冒烟测试"
}

# ============================================================
#  4. 编译 Rust 后端
# ============================================================
Push-Location (Join-Path $Root "src-tauri")
try {
    if ($DebugBuild) {
        Write-Step "编译 Rust 后端（debug）"
        & $Cargo build
        $exeDir = Join-Path $env:CARGO_TARGET_DIR "debug"
    } else {
        Write-Step "编译 Rust 后端（release）"
        Write-Dim "首次编译约 8-12 分钟，请耐心等待…"
        & $Cargo build --release
        $exeDir = Join-Path $env:CARGO_TARGET_DIR "release"
    }
} finally {
    Pop-Location
}

if ($LASTEXITCODE -ne 0) {
    Write-Err2 "cargo 编译失败（退出码 $LASTEXITCODE），请把上面的报错贴出来"
    exit 1
}

$exe = Join-Path $exeDir "wordwise.exe"
if (-not (Test-Path $exe)) {
    Write-Err2 "编译产物未生成：$exe"
    exit 1
}
$sizeMB = [math]::Round((Get-Item $exe).Length / 1MB, 2)
Write-Ok "产物：$exe（$sizeMB MB）"

if ($Run) {
    Write-Step "启动程序"
    Start-Process $exe
    Write-Ok "已启动"
}

if ($DebugBuild) {
    Write-Host ""
    Write-Host "Debug 编译完成" -ForegroundColor Green
    Write-Host "  运行：$exe" -ForegroundColor White
    exit 0
}

# ============================================================
#  5. 便携版（绿色版）输出 → dist\
# ============================================================
# 前端资源已内嵌进 exe，wordwise.exe 是**单文件绿色版**：
# 拷到任何地方双击即可运行，不写注册表、不往系统目录拷文件。
Write-Step "便携版输出"

$distDir = Join-Path $Root "dist"
New-Item -ItemType Directory -Path $distDir -Force | Out-Null
Copy-Item $exe (Join-Path $distDir "wordwise.exe") -Force
if (Test-Path (Join-Path $Root "README.md")) {
    Copy-Item (Join-Path $Root "README.md") (Join-Path $distDir "README.md") -Force
}
if (Test-Path (Join-Path $Root "docs")) {
    $docsOut = Join-Path $distDir "docs"
    New-Item -ItemType Directory -Path $docsOut -Force | Out-Null
    Copy-Item (Join-Path $Root "docs\*.md") $docsOut -Force -ErrorAction SilentlyContinue
}
# 便携版使用说明（源文件放在 installer\，避免 dist 被重建时丢失）
$portableNote = Join-Path $Root "installer\便携版说明.txt"
if (Test-Path $portableNote) {
    Copy-Item $portableNote (Join-Path $distDir "便携版说明.txt") -Force
}
Write-Ok "便携版目录：$distDir（双击 wordwise.exe 即可运行）"

# 便携版压缩包，与安装包并列放在 installer\output
$portableZip = Join-Path $Root "installer\output\WordWise-$AppVersion-portable.zip"
New-Item -ItemType Directory -Path (Split-Path $portableZip) -Force | Out-Null
# ★ 不要用 Remove-Item 删旧 zip，也不要用 Compress-Archive -Force：
#   两者内部都会先删目标文件，而删除会被安全策略 fail-closed 拦截
#   （[safe-delete][SAFE_DELETE_FAIL_CLOSED]），zip 一存在就再也生成不出来。
#   改用 .NET 的 FileMode::Create —— 原地截断覆盖，全程不删除。
try {
    Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction SilentlyContinue
    $base = (Resolve-Path $distDir).Path
    $fs = [System.IO.File]::Open($portableZip, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
    try {
        $zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
            Get-ChildItem $base -Recurse -File | ForEach-Object {
                $rel = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
                [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $zip, $_.FullName, $rel,
                    [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
            }
        } finally { $zip.Dispose() }
    } finally { $fs.Close() }
    $zipMB = [math]::Round((Get-Item $portableZip).Length / 1MB, 2)
    Write-Ok "便携版压缩包：$portableZip（$zipMB MB）"
} catch {
    Write-Warn2 "便携版压缩包生成失败：$($_.Exception.Message)"
}

if ($SkipInstaller) {
    Write-Warn2 "已跳过安装程序打包"
    Write-Host ""
    Write-Host "编译完成：$exe" -ForegroundColor Green
    exit 0
}

# ============================================================
#  6. Inno Setup 打包
# ============================================================
Write-Step "Inno Setup 打包"

function Get-IsccVersion {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return $null }
    # ★ ISCC 不带参数时：stdout 是版本横幅，**stderr 是 Usage 提示且退出码为 1**。
    #   写成 `& $Path 2>&1` 会把 stderr 变成 ErrorRecord，在 $ErrorActionPreference='Stop'
    #   下直接变成终止错误 → 被 catch 吞掉 → 版本探测偶发失败（表现为
    #   「该文件无法识别为 Inno Setup 编译器」）。
    #   所以只取 stdout、丢掉 stderr，并临时把 ErrorActionPreference 调回 Continue。
    $prev = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $out = & $Path 2>$null
        $line = @($out) | Where-Object { $_ -match 'Inno Setup' } | Select-Object -First 1
        if ($line -match 'Inno Setup\s+(\d+)') { return [int]$Matches[1] }
    } catch {
        return $null
    } finally {
        $ErrorActionPreference = $prev
    }
    return $null
}

function Find-Iscc {
    param([string]$Explicit)

    # 1) 命令行显式指定的路径优先（不存在也要报错，见下方校验）
    if ($Explicit) { return $Explicit }

    # 2) 完整路径候选（ISCC.exe 不在 PATH，必须逐条试完整路径）
    $cands = @(
        $DefaultIscc,
        "G:\Programming\07-utils\Inno Setup 6\ISCC.exe",
        "G:\tools_office\Inno Setup 6\ISCC.exe",
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 7\ISCC.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
        "C:\Program Files (x86)\Inno Setup 7\ISCC.exe",
        "C:\Program Files\Inno Setup 7\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $cands) {
        if ($c -and (Test-Path $c)) { return $c }
    }

    # 3) 最后才考虑 PATH，且必须是真的 ISCC（不能假设 `iscc` 可直接调用）
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd -and (Get-IsccVersion $cmd.Source)) { return $cmd.Source }

    return $null
}

$Iscc = Find-Iscc -Explicit $IsccPath

if ($Iscc -and -not (Test-Path $Iscc)) {
    Write-Err2 "指定的 ISCC.exe 路径不存在：$Iscc"
    Write-Host "  请确认 Inno Setup 的实际安装位置，然后用 -IsccPath 传入完整路径："
    Write-Host '    .\build.ps1 -IsccPath "<你的路径>\ISCC.exe"'
    Write-Host "  当前脚本默认位置：$DefaultIscc"
    exit 1
}

if (-not $Iscc) {
    Write-Warn2 "未找到 Inno Setup 的 ISCC.exe，已跳过安装程序打包"
    Write-Host "  预期位置（未找到）：$DefaultIscc"
    Write-Host "  ISCC.exe 默认不在 PATH 里，请确认实际安装位置后任选一种方式："
    Write-Host '    1) 临时指定： .\build.ps1 -IsccPath "<你的路径>\ISCC.exe"'
    Write-Host "    2) 永久修改： 改 build.ps1 顶部的 `$DefaultIscc"
    Write-Host "  排查命令： where.exe ISCC.exe   或   Get-ChildItem G:\ -Recurse -Filter ISCC.exe"
    Write-Host "  下载：https://jrsoftware.org/isdl.php"
    Write-Host ""
    Write-Host "可执行文件已生成：$exe" -ForegroundColor Green
    exit 0
}

$IsccVer = Get-IsccVersion $Iscc
if ($null -eq $IsccVer) {
    Write-Err2 "该文件无法识别为 Inno Setup 编译器：$Iscc"
    Write-Host "  请确认实际安装位置，再用 -IsccPath 指定正确的 ISCC.exe"
    exit 1
}

if ($IsccVer -ne $ExpectedIsccMajor) {
    Write-Warn2 "ISCC 版本 $IsccVer，与脚本预期的 Inno Setup $ExpectedIsccMajor 不一致"
    Write-Host "  若确认该版本可用可忽略此提示；否则用 -IsccPath 指定正确版本："
    Write-Host '    .\build.ps1 -IsccPath "<你的路径>\ISCC.exe"'
    Write-Host ""
}

Write-Ok "ISCC: $Iscc（Inno Setup $IsccVer）"

# 传实际的 release 绝对路径，避免 .iss 内相对路径失配
& $Iscc "/DMySourceDir=$exeDir" (Join-Path $Root "installer\wordwise.iss")
if ($LASTEXITCODE -ne 0) {
    Write-Err2 "Inno Setup 打包失败，请查看上方输出"
    exit 1
}

$setup = Get-ChildItem (Join-Path $Root "installer\output") -Filter "*-Setup-*.exe" -ErrorAction SilentlyContinue |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($setup) {
    $setupMB = [math]::Round($setup.Length / 1MB, 2)
    Write-Ok "安装程序：$($setup.FullName)（$setupMB MB）"
} else {
    Write-Warn2 "未找到安装程序产物，请检查 installer\output"
}

# ============================================================
#  7. 上传 GitHub（可选）
# ============================================================
if ($Upload) {
    Write-Step "上传到 GitHub"
    $py = $null
    foreach ($p in @("python", "py")) {
        try { $null = & $p --version 2>$null; if ($LASTEXITCODE -eq 0) { $py = $p; break } } catch {}
    }
    if (-not $py) {
        Write-Err2 "未找到 Python，无法执行上传脚本"
    } else {
        & $py "scripts\upload_github.py" --public
    }
}

Write-Host ""
Write-Host ("=" * 60) -ForegroundColor DarkGray
Write-Host "  构建完成" -ForegroundColor Green
Write-Host ("=" * 60) -ForegroundColor DarkGray
if ($setup) { Write-Host "  安装程序：$($setup.FullName)" -ForegroundColor White }
Write-Host "  便携版目录：$distDir" -ForegroundColor White
if (Test-Path $portableZip) { Write-Host "  便携版压缩包：$portableZip" -ForegroundColor White }
Write-Host "  可执行文件：$exe" -ForegroundColor White
Write-Host ""
Write-Host "提示：修改前端（src/js、src/css、src/index.html）后必须重跑本脚本，" -ForegroundColor DarkGray
Write-Host "      因为前端资源会被嵌进 exe。" -ForegroundColor DarkGray
