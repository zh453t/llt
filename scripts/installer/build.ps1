<#
.SYNOPSIS
  用 Inno Setup 把 target\release\llt-gui.exe 打包成安装程序。

.DESCRIPTION
  - 版本号默认取自 Cargo.toml 的 [workspace.package] version
  - 自动定位 ISCC.exe（也支持 -Iscc 指定）
  - 产物输出到 out\installer\

.EXAMPLE
  pwsh -File scripts/installer/build.ps1
  pwsh -File scripts/installer/build.ps1 -Build          # 先 cargo build --release
  pwsh -File scripts/installer/build.ps1 -BundleDb       # 连数据库一起打包
  pwsh -File scripts/installer/build.ps1 -Version 1.2.3  # 手动指定版本
#>
param(
  [string]$Version,
  [string]$Iscc,
  [switch]$Build,
  [switch]$BundleDb
)

$ErrorActionPreference = 'Stop'

$installerDir = $PSScriptRoot
# 脚本在 scripts\installer\ 下，仓库根在上两级
$repoRoot = Split-Path -Parent (Split-Path -Parent $installerDir)

# 1. 定位 ISCC.exe
if (-not $Iscc) {
  $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
  $candidates = @(
    'C:\Program Files\Inno Setup 7\ISCC.exe',
    'C:\Program Files (x86)\Inno Setup 7\ISCC.exe',
    'C:\Program Files\Inno Setup 6\ISCC.exe',
    'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
  )
  if ($cmd) { $candidates = @($cmd.Source) + $candidates }
  $Iscc = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $Iscc -or -not (Test-Path $Iscc)) {
  throw "找不到 ISCC.exe，请安装 Inno Setup 或用 -Iscc 指定编译器路径。"
}

# 2. 版本号：命令行优先，否则读 Cargo.toml
if (-not $Version) {
  $cargoText = Get-Content -Raw (Join-Path $repoRoot 'Cargo.toml')
  if ($cargoText -match '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    $Version = $Matches[1]
  }
  else {
    throw "无法从 Cargo.toml 读取版本号，请用 -Version 指定。"
  }
}

# 3. 可选：先构建 release
if ($Build) {
  Push-Location $repoRoot
  try {
    & cargo build -p llt-gui --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（退出码 $LASTEXITCODE）。" }
  }
  finally { Pop-Location }
}

# 4. 检查 exe
$exe = Join-Path $repoRoot 'target\release\llt-gui.exe'
if (-not (Test-Path $exe)) {
  throw "找不到 $exe 。请先运行 cargo build -p llt-gui --release，或给本脚本加 -Build。"
}

# 5. 调用 ISCC
$iss = Join-Path $installerDir 'llt-gui.iss'
$defines = @("/dMyAppVersion=$Version")
if ($BundleDb) { $defines += '/dBundleDb=1' }

Write-Host "使用 ISCC: $Iscc"
Write-Host "版本: $Version"
if ($BundleDb) { Write-Host "包含数据库: 是" }

& $Iscc @defines $iss
if ($LASTEXITCODE -ne 0) { throw "ISCC 编译失败（退出码 $LASTEXITCODE）。" }

# 6. 报告产物
$outDir = Join-Path $repoRoot 'out\installer'
Write-Host "`n安装包已生成于: $outDir"
Get-ChildItem $outDir -Filter '*.exe' -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1 |
  ForEach-Object { Write-Host "  $($_.FullName)" }
