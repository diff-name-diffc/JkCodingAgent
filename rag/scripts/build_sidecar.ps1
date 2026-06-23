# Windows 版 RAG sidecar 构建脚本（PowerShell），逻辑与 build_sidecar.sh 对应。
# 用法（在项目根目录）：
#   powershell -ExecutionPolicy Bypass -File rag\scripts\build_sidecar.ps1
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RagRoot = Resolve-Path "$ScriptDir\.."
$RepoRoot = Resolve-Path "$RagRoot\.."
$BinariesDir = Join-Path $RepoRoot "src-tauri\binaries"
$BinaryName = "rag-server"

Write-Host "==> [1/4] uv 同步依赖"
Push-Location $RagRoot
uv sync --no-dev
if ($LASTEXITCODE -ne 0) { throw "uv sync 失败" }

Write-Host "==> [2/4] PyInstaller 打包（onefile）"
uv run --extra build pyinstaller --noconfirm --clean rag_server.spec
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "PyInstaller 打包失败" }
Pop-Location

$Artifact = Join-Path $RagRoot "dist\$BinaryName.exe"
if (-not (Test-Path $Artifact)) { throw "PyInstaller 产物未找到：$Artifact" }

Write-Host "==> [3/4] 探测 target-triple"
$Arch = $env:PROCESSOR_ARCHITECTURE
if ($Arch -eq "ARM64") {
    $Triple = "aarch64-pc-windows-msvc"
} else {
    $Triple = "x86_64-pc-windows-msvc"
}
Write-Host "    triple = $Triple"

Write-Host "==> [4/4] 安装到 $BinariesDir"
New-Item -ItemType Directory -Force -Path $BinariesDir | Out-Null
$Dest = Join-Path $BinariesDir "$BinaryName-$Triple.exe"
Copy-Item -Force $Artifact $Dest
Write-Host "    -> $Dest"
Write-Host "完成。下次 tauri build/dev 将自动使用此 sidecar。"
