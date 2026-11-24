<#
.SYNOPSIS
  移动 / 缩放 llt-gui 窗口，用于验证宽窄布局；输出移动后的可见矩形。
.EXAMPLE
  pwsh -File move.ps1 -Width 1200 -Height 800 -Left 100 -Top 100
#>
param(
  [int]$Width = 1400, [int]$Height = 1900, [int]$Left = 100, [int]$Top = 60,
  [string]$ProcessName = 'llt-gui'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$proc = Get-LltWindow -ProcessName $ProcessName
$h = $proc.MainWindowHandle
[LltWin32]::ShowWindow($h, 9) | Out-Null   # SW_RESTORE
Start-Sleep -Milliseconds 400
if (-not [LltWin32]::MoveWindow($h, $Left, $Top, $Width, $Height, $true)) {
  throw 'MoveWindow 失败，窗口尺寸未改变。'
}
if (-not (Set-LltForeground -Handle $h)) { Write-Warning '窗口已缩放，但置前失败。' }
Start-Sleep -Milliseconds 800
$r = Get-LltWindowRect -Handle $h
Write-Output ("moved to {0},{1} {2}x{3} (visible rect)" -f $r.Left, $r.Top, ($r.Right - $r.Left), ($r.Bottom - $r.Top))
