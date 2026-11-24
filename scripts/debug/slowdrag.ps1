<#
.SYNOPSIS
  缓慢拖拽：按住 600ms 后再分 150 步移动，模拟触摸屏长按换列序 / 拖列宽。
.EXAMPLE
  pwsh -File slowdrag.ps1 -X 800 -Y 800 -X2 900 -Y2 800
#>
param([int]$X = 0, [int]$Y = 0, [int]$X2 = 0, [int]$Y2 = 0, [string]$ProcessName = 'llt-gui')

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$proc = Get-LltWindow -ProcessName $ProcessName
$h = $proc.MainWindowHandle
if (-not (Set-LltForeground -Handle $h)) { throw '无法把 llt-gui 置前，已中止。' }
Invoke-LltDrag -X1 $X -Y1 $Y -X2 $X2 -Y2 $Y2 -Steps 150 -StepMs 20 -HoldMs 600 -Handle $h
