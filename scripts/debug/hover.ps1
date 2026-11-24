<#
.SYNOPSIS
  把鼠标移到指定坐标并停留，触发 hover 提示（默认 700ms）。
.EXAMPLE
  pwsh -File hover.ps1 -X 600 -Y 600 -DurationMs 1000
#>
param(
  [int]$X = 0, [int]$Y = 0, [int]$DurationMs = 700,
  [string]$ProcessName = 'llt-gui',
  [switch]$NoFocus
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$h = [IntPtr]::Zero
if (-not $NoFocus) {
  $proc = Get-LltWindow -ProcessName $ProcessName
  $h = $proc.MainWindowHandle
  if (-not (Set-LltForeground -Handle $h)) { throw '无法把 llt-gui 置前，已中止。' }
}

[LltWin32]::SetCursorPos($X, $Y) | Out-Null
Assert-LltCursorAt -X $X -Y $Y -Reapply
Start-Sleep -Milliseconds $DurationMs
