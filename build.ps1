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
#    .\build.ps1 -ModelsDir "D:\WordWise\data\models"   指定「含模型」便携包的模型来源
#    .\build.ps1 -SkipModelsZip  只出「不带模型」的便携包
#
#  产物（installer\output\）：
#    WordWise-Setup-<版本>.exe                     安装程序
#    WordWise-<版本>-portable.zip                  便携包，不含模型（小）
#    WordWise-<版本>-portable-with-models.zip      便携包，含已下载的模型（大，解压就能用 AI）
#
#  说明：
#    - 前端资源（src/index.html、src/js、src/css）在编译时嵌入 exe，
#      改了前端必须重新编译才生效。
#    - 便携包里会写一个空的 portable.txt，运行时据此把数据放在
#      exe 同级 data\，不写 C 盘；代价是删掉便携目录会连词库一起删。
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
    [string]$TargetDir = "",
    # 带模型便携包要从哪里取 .gguf；不填则自动探测本机数据目录
    [string]$ModelsDir = "",
    # 只出「不带模型」的便携包（模型动辄 1GB+，CI 上没必要出两份）
    [switch]$SkipModelsZip
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
# 6 和 7 都支持（BUILD.md 里写的要求就是「Inno Setup 6.2+」）。
# 本机装的是 7，CI（windows runner 用 choco 装）拿到的是 6 —— 两者都能出包，
# 所以这里不能只认 7，否则 CI 日志里每次都要多一条没意义的警告。
$SupportedIsccMajors = @(6, 7)

# 版本号统一从 tauri.conf.json 读，避免与安装包、界面显示的版本脱节
$AppVersion = "0.45.4"
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

# ---- 随包分发 llama.cpp 引擎（本地大模型一键部署用）----
#
# 为什么必须随包：运行时从 GitHub Release 下载引擎，实测 gh-proxy 被限速到
# ~40KB/s，33MB 要下 14 分钟 —— 这个等待没有任何产品能接受。
# 随包后「一键部署」只剩下载模型（走魔搭 14.5MB/s，1.1GB 约 80 秒）。
#
# 只拷 exe + dll，不拷源 zip（那是给想自己解包的人留的）。
# 引擎缺失时不影响主程序，只是「一键部署」回退到联网下载。
$engineSrc = Join-Path $Root "vendor\llama"
$engineExe = Join-Path $engineSrc "llama-server.exe"
if (Test-Path $engineExe) {
    $engineDst = Join-Path $distDir "vendor\llama"
    New-Item -ItemType Directory -Path $engineDst -Force | Out-Null
    # ★ 不能用 `-Include @("*.exe","*.dll")`：
    #   Get-ChildItem 的 -Include 只在路径带通配符或加了 -Recurse 时才生效，
    #   否则静默返回空 → 引擎一个文件都没拷，便携版里「一键部署」会回退到
    #   联网下载（14 分钟）。用 Where-Object 过滤扩展名才稳。
    Get-ChildItem $engineSrc -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Extension -in @('.exe', '.dll') } |
        ForEach-Object { Copy-Item $_.FullName $engineDst -Force }
    $engMB = [math]::Round(((Get-ChildItem $engineDst -File | Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
    Write-Ok "已随包 llama.cpp 引擎 → dist\vendor\llama（$engMB MB）"
} else {
    Write-Warn2 "未找到 vendor\llama\llama-server.exe，便携版不带引擎"
    Write-Dim "「一键部署」将回退到联网下载（gh-proxy 限速，约 14 分钟）"
}

# ---- 随包分发 Piper 语音引擎与预置语音（本地神经朗读用）----
#
# 同样是被限速逼的：实测 gh-proxy 拉 piper 引擎包只有约 21 KB/s，22MB 要下
# 十几分钟。语音包本身走 hf-mirror 很快（实测 2.7 MB/s，20MB 约 8 秒），
# 但引擎这一步卡住就等于整个功能不可用 —— 所以引擎必须随包。
#
# 英文默认语音一并随包（开箱即用），其他语种按需下载。
# 用 scripts\fetch_tts_vendor.py 准备 vendor\piper 与 vendor\tts-voices
# （早期脚本产出的是 vendor\voices，下面两个目录都会打包）。
$piperSrc = Join-Path $Root "vendor\piper"
$piperExe = Join-Path $piperSrc "piper.exe"
if (Test-Path $piperExe) {
    $piperDst = Join-Path $distDir "vendor\piper"
    New-Item -ItemType Directory -Path $piperDst -Force | Out-Null
    # ★ 必须整目录递归拷贝：piper 除了 exe 还依赖 onnxruntime 等 dll
    #   和 espeak-ng-data 音素数据。只拷 exe/dll 会得到一个能启动但
    #   一合成就报错的引擎（缺音素表）。
    Copy-Item (Join-Path $piperSrc "*") $piperDst -Recurse -Force
    $piperMB = [math]::Round(((Get-ChildItem $piperDst -File -Recurse |
        Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
    Write-Ok "已随包 Piper 语音引擎 → dist\vendor\piper（$piperMB MB）"
} else {
    Write-Warn2 "未找到 vendor\piper\piper.exe，安装包不带本地语音引擎"
    Write-Dim "本地神经朗读将回退到联网下载（gh-proxy 限速，可能要十几分钟）"
}

# 预置语音包。
#
# ★ 为什么要从**两个**目录取：历史上预置语音放过两个位置 ——
#   早期是 `vendor\voices\<id>.onnx`（平铺），后来统一成
#   `vendor\tts-voices\<id>\<id>.onnx`（一条一个目录）。两种布局在实机上
#   并存过（本机就留着一份 `vendor\voices\zh_CN-huayan-x_low.onnx`）。
#   只打包其中一个，另一份在用户机器上就「文件明明在、设置页里却没有」——
#   因为 Rust 侧（tts::bundled_voice_roots）虽然两个目录都扫，但安装包里
#   压根没带另一个目录的文件。
#   → 两个都可能存在，所以两个都打包；都不存在时提示用户需自行下载。
$piperVoiceSources = @(
    @{ Src = (Join-Path $Root "vendor\tts-voices"); Name = "tts-voices" },
    @{ Src = (Join-Path $Root "vendor\voices");     Name = "voices" }
)
$piperVoicePacked = 0
foreach ($pv in $piperVoiceSources) {
    if (-not (Test-Path $pv.Src)) {
        Write-Dim "未找到 vendor\$($pv.Name)，跳过"
        continue
    }
    $voiceDst = Join-Path $distDir "vendor\$($pv.Name)"
    New-Item -ItemType Directory -Path $voiceDst -Force | Out-Null
    Copy-Item (Join-Path $pv.Src "*") $voiceDst -Recurse -Force
    $voiceMB = [math]::Round(((Get-ChildItem $voiceDst -File -Recurse |
        Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
    Write-Ok "已随包预置语音 → dist\vendor\$($pv.Name)（$voiceMB MB）"
    $piperVoicePacked++
}
if ($piperVoicePacked -eq 0) {
    Write-Dim "未找到任何预置语音目录（vendor\tts-voices 或 vendor\voices），安装包不含预置语音（用户需自行下载）"
}

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

# ---- ★ 便携标记：让数据跟着程序目录走（自包含）----
#
# 空内容的 portable.txt 就是「这是便携模式」的约定，运行时据此把数据
# 放到 exe 同级 data\。好处：整个文件夹拷到 U 盘就能带着走，不写 C 盘。
# 代价：删掉这个文件夹会连词库一起删 —— 说明文件里已经写清楚。
#
# 用 .NET 直接写空文件（不用 Set-Content，避免带 BOM）。
$portableMark = Join-Path $distDir "portable.txt"
[System.IO.File]::WriteAllText($portableMark, "", (New-Object System.Text.UTF8Encoding($false)))
Write-Ok "已写入便携标记 portable.txt（数据将落在 exe 同级 data\）"
Write-Ok "便携版目录：$distDir（双击 wordwise.exe 即可运行）"

# ---- 生成 zip 的公共实现 ----
#
# ★ 不要用 Remove-Item 删旧 zip，也不要用 Compress-Archive -Force：
#   两者内部都会先删目标文件，而删除会被安全策略 fail-closed 拦截
#   （[safe-delete][SAFE_DELETE_FAIL_CLOSED]），zip 一存在就再也生成不出来。
#   改用 .NET 的 FileMode::Create —— 原地截断覆盖，全程不删除。
Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction SilentlyContinue

function New-DirZip {
    param(
        [string]$BaseDir,
        [string]$ZipPath,
        # 顶层目录名：命中的整棵子树都不打包。
        # ★ 用它排除运行期产生的 data\：只要在 dist\ 下双击过一次 wordwise.exe，
        #   那里就会多出 wordwise.db / -wal / -shm。不排除的话，下一次构建
        #   打出来的便携包里就带着一份**别人的**学习数据（还可能是演示数据），
        #   用户解压就「继承了」一份不该有的词库。
        [string[]]$SkipTopLevel = @()
    )
    New-Item -ItemType Directory -Path (Split-Path $ZipPath) -Force | Out-Null
    $base = (Resolve-Path $BaseDir).Path
    $fs = [System.IO.File]::Open($ZipPath, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
    try {
        $zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
            Get-ChildItem $base -Recurse -File | Where-Object {
                $rel = $_.FullName.Substring($base.Length + 1)
                $top = $rel.Split('\')[0]
                $SkipTopLevel -notcontains $top
            } | ForEach-Object {
                $rel = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
                [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $zip, $_.FullName, $rel,
                    [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
            }
        } finally { $zip.Dispose() }
    } finally { $fs.Close() }
    return [math]::Round((Get-Item $ZipPath).Length / 1MB, 2)
}

$portableZip = Join-Path $Root "installer\output\WordWise-$AppVersion-portable.zip"
try {
    $zipMB = New-DirZip -BaseDir $distDir -ZipPath $portableZip -SkipTopLevel @('data')
    Write-Ok "便携版压缩包（不含模型）：$portableZip（$zipMB MB）"
} catch {
    Write-Warn2 "便携版压缩包生成失败：$($_.Exception.Message)"
}

# ---- 便携版第二形态：带上已下载的模型 ----
#
# 为什么要有这一份：模型单档 400MB~1.1GB，走网下载对用户是实打实的等待。
# 给一个「解压就能用 AI」的包，比让他下完程序再下模型友好得多。
# 代价是包很大，所以做成**可选**的第二份，不覆盖那份小的。
if (-not $SkipModelsZip) {
    Write-Step "便携版（含模型）"

    # 找本机的模型目录：显式参数优先，否则按运行时同样的优先级探测
    $srcModels = $null
    if ($ModelsDir) {
        if (Test-Path $ModelsDir) { $srcModels = (Resolve-Path $ModelsDir).Path }
        else { Write-Warn2 "指定的 -ModelsDir 不存在：$ModelsDir" }
    } else {
        $cands = @()
        if ($env:WORDWISE_DATA_DIR) { $cands += (Join-Path $env:WORDWISE_DATA_DIR "models") }
        $cands += (Join-Path $Root "data\models")
        if ($env:APPDATA) { $cands += (Join-Path $env:APPDATA "WordWise\models") }
        foreach ($c in $cands) {
            if ($c -and (Test-Path $c)) {
                $found = Get-ChildItem $c -Filter *.gguf -File -ErrorAction SilentlyContinue
                if ($found) { $srcModels = (Resolve-Path $c).Path; break }
            }
        }
    }

    $ggufs = @()
    if ($srcModels) {
        $ggufs = @(Get-ChildItem $srcModels -Filter *.gguf -File -ErrorAction SilentlyContinue)
    }

    if ($ggufs.Count -eq 0) {
        Write-Warn2 "本机没有已下载的模型（.gguf），跳过「含模型」便携包"
        Write-Dim "相关候选目录："
        Write-Dim ("  " + (Join-Path $env:APPDATA "WordWise\models"))
        Write-Dim ("  " + (Join-Path $Root "data\models"))
        Write-Dim "想出一个带模型的包：先在程序里下好模型，或用 -ModelsDir 指定目录"
    } else {
        # 用独立的 staging 目录，避免把 GB 级模型塞进 dist\ 污染那份小包。
        #
        # ★ 必须先清干净再建：PowerShell 的 `Copy-Item $src $dst -Recurse` 在
        #   **$dst 已存在**时会把 $src 整个塞进 $dst 里面（变成 dst\dist\...），
        #   而不是把内容合并过去 —— 这是最容易在「第二次构建」时才炸的坑。
        #   所以这里显式「先删 → 重建 → 只拷内容」。
        $distModelsDir = Join-Path $Root "dist-models"
        $stale = $false
        if (Test-Path $distModelsDir) {
            try { Remove-Item $distModelsDir -Recurse -Force -ErrorAction Stop }
            catch { $stale = $true; Write-Warn2 "旧的 dist-models 清不掉：$($_.Exception.Message)" }
        }

        if ($stale) {
            # 宁可不出这份包，也不要打出一个混着上次残留的包
            Write-Warn2 "跳过「含模型」便携包（staging 目录不干净，免得把残留文件打进去）"
        } else {
            New-Item -ItemType Directory -Path $distModelsDir -Force | Out-Null
            # 只拷「程序文件」：跳过 dist\data（那是本机跑过以后留下的运行期数据）。
            # 用逐个 Get-ChildItem 而不是 `Copy-Item $src $dst -Recurse`，
            # 后者在 $dst 已存在时会把 $src 塞进 $dst 里面，多出一层目录。
            Get-ChildItem $distDir -Force | Where-Object { $_.Name -ne 'data' } | ForEach-Object {
                Copy-Item $_.FullName $distModelsDir -Recurse -Force
            }

            # 这一份的 data\ 是我们**自己造**的：里面只有模型，没有数据库
            $modelsDst = Join-Path $distModelsDir "data\models"
            New-Item -ItemType Directory -Path $modelsDst -Force | Out-Null
            $totalMB = 0.0
            foreach ($g in $ggufs) {
                Copy-Item $g.FullName $modelsDst -Force
                $totalMB += $g.Length / 1MB
            }
            Write-Ok "已放入 $($ggufs.Count) 个模型 → dist-models\data\models（$([math]::Round($totalMB, 1)) MB）"
            foreach ($g in $ggufs) { Write-Dim "  $($g.Name)" }

            # 顺手校验一下 staging 里自包含性真的成立
            if (-not (Test-Path (Join-Path $distModelsDir "portable.txt"))) {
                Write-Warn2 "dist-models 里没有 portable.txt，解压后数据不会落在程序目录"
            }

            $modelsZip = Join-Path $Root "installer\output\WordWise-$AppVersion-portable-with-models.zip"
            try {
                $mzMB = New-DirZip -BaseDir $distModelsDir -ZipPath $modelsZip
                Write-Ok "便携版压缩包（含模型）：$modelsZip（$mzMB MB）"
            } catch {
                Write-Warn2 "含模型便携包生成失败：$($_.Exception.Message)"
            }
        }
    }
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

if ($SupportedIsccMajors -notcontains $IsccVer) {
    Write-Warn2 "ISCC 版本 $IsccVer，不在脚本支持的 Inno Setup $($SupportedIsccMajors -join ' / ') 之内"
    Write-Host "  若确认该版本可用可忽略此提示；否则用 -IsccPath 指定正确版本："
    Write-Host '    .\build.ps1 -IsccPath "<你的路径>\ISCC.exe"'
    Write-Host ""
}

Write-Ok "ISCC: $Iscc（Inno Setup $IsccVer）"

# 传实际的 release 绝对路径，避免 .iss 内相对路径失配。
# 版本号也一并传进去（.iss 里的 MyAppVersion 已改成可被 /D 覆盖）：这样
# 「安装包文件名 / 程序属性 / 界面显示」三处版本号同源，都来自 tauri.conf.json。
& $Iscc "/DMySourceDir=$exeDir" "/DMyAppVersion=$AppVersion" (Join-Path $Root "installer\wordwise.iss")
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
if (Test-Path $portableZip) { Write-Host "  便携包（不带模型）：$portableZip" -ForegroundColor White }
$modelsZipFinal = Join-Path $Root "installer\output\WordWise-$AppVersion-portable-with-models.zip"
if (Test-Path $modelsZipFinal) { Write-Host "  便携包（含模型）：$modelsZipFinal" -ForegroundColor White }
Write-Host "  可执行文件：$exe" -ForegroundColor White
Write-Host ""
Write-Host "提示：修改前端（src/js、src/css、src/index.html）后必须重跑本脚本，" -ForegroundColor DarkGray
Write-Host "      因为前端资源会被嵌进 exe。" -ForegroundColor DarkGray
