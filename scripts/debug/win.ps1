<#
.SYNOPSIS
  打印 llt-gui 可见矩形；默认只打印不点击（-Action info）。可置前、点击/双击/拖拽。
.DESCRIPTION
  -TopMost 为可选项：置顶仅在本次动作期间生效，结束（含异常）后必定还原，
  避免调试完应用一直悬在其它窗口之上。
.EXAMPLE
  pwsh -File win.ps1
  pwsh -File win.ps1 -Action click -X 600 -Y 600 [-TopMost]
#>
param(
  [int]$X = 0, [int]$Y = 0, [int]$X2 = 0, [int]$Y2 = 0,
  [ValidateSet('info', 'click', 'dblclick', 'drag')][string]$Action = 'info',
  [switch]$TopMost,
  [string]$ProcessName = 'llt-gui'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$proc = Get-LltWindow -ProcessName $ProcessName
$h = $proc.MainWindowHandle
$r = Get-LltWindowRect -Handle $h
Write-Output ("window: {0},{1} {2}x{3}" -f $r.Left, $r.Top, ($r.Right - $r.Left), ($r.Bottom - $r.Top))
if ($Action -eq 'info') { return }

if (-not (Set-LltForeground -Handle $h)) { throw '无法把 llt-gui 置前，已中止（避免误点）。' }
$topMostSet = $false
try {
  if ($TopMost) {
    # HWND_TOPMOST = -1，SWP_NOMOVE | SWP_NOSIZE = 0x0003
    [LltWin32]::SetWindowPos($h, [IntPtr](-1), 0, 0, 0, 0, 0x0003) | Out-Null
    $topMostSet = $true
  }
  Assert-LltForeground -Handle $h
  switch ($Action) {
    'click'    { Invoke-LltClick -X $X -Y $Y -Handle $h }
    'dblclick' { Invoke-LltClick -X $X -Y $Y -Handle $h; Invoke-LltClick -X $X -Y $Y -Handle $h }
    'drag'     { Invoke-LltDrag -X1 $X -Y1 $Y -X2 $X2 -Y2 $Y2 -Steps 30 -StepMs 20 -HoldMs 600 -Handle $h }
  }
}
finally {
  if ($topMostSet) {
    # HWND_NOTOPMOST = -2，无论成功或异常都还原。
    [LltWin32]::SetWindowPos($h, [IntPtr](-2), 0, 0, 0, 0, 0x0003) | Out-Null
  }
}
