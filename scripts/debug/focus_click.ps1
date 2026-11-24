<#
.SYNOPSIS
  把 llt-gui 置前并点击 / 双击 / 拖拽（远程会话里最稳：AttachThreadInput + ALT 脉冲）。
.EXAMPLE
  pwsh -File focus_click.ps1 -X 600 -Y 600 -Action click
  pwsh -File focus_click.ps1 -X 600 -Y 600 -X2 900 -Y2 600 -Action drag
#>
param(
  [int]$X = 0, [int]$Y = 0, [int]$X2 = 0, [int]$Y2 = 0,
  [ValidateSet('click', 'dblclick', 'drag')][string]$Action = 'click',
  [string]$ProcessName = 'llt-gui'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$proc = Get-LltWindow -ProcessName $ProcessName
$h = $proc.MainWindowHandle
if (-not (Set-LltForeground -Handle $h)) { throw '无法把 llt-gui 置前，已中止（避免误点）。' }

switch ($Action) {
  'click'    { Invoke-LltClick -X $X -Y $Y -Handle $h }
  'dblclick' { Invoke-LltClick -X $X -Y $Y -Handle $h; Invoke-LltClick -X $X -Y $Y -Handle $h }
  'drag'     { Invoke-LltDrag -X1 $X -Y1 $Y -X2 $X2 -Y2 $Y2 -Steps 30 -StepMs 20 -HoldMs 600 -Handle $h }
}
Write-Output ("foreground is app: " + ([LltWin32]::GetForegroundWindow().ToInt64() -eq $h.ToInt64()))
