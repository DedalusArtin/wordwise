# ============================================================
#  WordWise 构建入口（双击运行 / PowerShell 里运行）
#
#  双击本文件 → 默认执行 release 编译 + 打包安装程序
#  也可在 PowerShell 里带参数运行，参数直接透传给 build.ps1
# ============================================================

# 保证编码
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

$Root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $Root

Write-Host ""
Write-Host "  WordWise 构建" -ForegroundColor Cyan
Write-Host "  ------------------------------------------------" -ForegroundColor DarkGray
Write-Host "  1  Release 编译 + 安装包   (约 8-12 分钟)"
Write-Host "  2  Debug 编译（快，用于快速验证）"
Write-Host "  3  Release 编译，不打安装包"
Write-Host "  4  先清理再完整编译"
Write-Host "  0  退出"
Write-Host ""

$choice = Read-Host "  请选择 [1]"
if ([string]::IsNullOrWhiteSpace($choice)) { $choice = "1" }

$psArgs = @()
switch ($choice) {
    "0" { exit 0 }
    "2" { $psArgs += "-DebugBuild" }
    "3" { $psArgs += "-SkipInstaller" }
    "4" { $psArgs += "-Clean" }
    default { }
}

& (Join-Path $Root "build.ps1") @psArgs
$rc = $LASTEXITCODE

Write-Host ""
Write-Host "  ------------------------------------------------" -ForegroundColor DarkGray
if ($rc -ne 0) {
    Write-Host "  构建失败（退出码 $rc）" -ForegroundColor Red
    Write-Host "  请把上面的报错复制发给我。" -ForegroundColor Yellow
} else {
    Write-Host "  构建完成" -ForegroundColor Green
}
Write-Host ""
if ($Host.Name -eq "ConsoleHost") { Read-Host "  按回车退出" | Out-Null }
