<#
.SYNOPSIS
  通用输入：click / dblclick / drag / type（中文走剪贴板）/ key（原始 SendKeys 按键）。
.DESCRIPTION
  默认也会先置前（与其余脚本一致）；-NoFocus 可保留「不抢焦点」的旧行为。
.EXAMPLE
  pwsh -File mouse.ps1 -Action click -X 600 -Y 600
  pwsh -File mouse.ps1 -Action type -Text "北京"
  pwsh -File mouse.ps1 -Action key -Text "{ENTER}"
#>
param(
  [ValidateSet('click', 'dblclick', 'drag', 'type', 'key')][string]$Action = 'click',
  [int]$X = 0, [int]$Y = 0, [int]$X2 = 0, [int]$Y2 = 0,
  [string]$Text = '',
  [string]$ProcessName = 'llt-gui',
  [switch]$NoFocus
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

$h = [IntPtr]::Zero
if (-not $NoFocus) {
  $proc = Get-LltWindow -ProcessName $ProcessName
  $h = $proc.MainWindowHandle
  if (-not (Set-LltForeground -Handle $h)) { throw '无法把 llt-gui 置前，已中止（避免误输入）。' }
}

switch ($Action) {
  'click'    { Invoke-LltClick -X $X -Y $Y -Handle $h }
  'dblclick' { Invoke-LltClick -X $X -Y $Y -Handle $h; Invoke-LltClick -X $X -Y $Y -Handle $h }
  'drag'     { Invoke-LltDrag -X1 $X -Y1 $Y -X2 $X2 -Y2 $Y2 -Steps 20 -StepMs 25 -HoldMs 600 -Handle $h }
  'type' {
    if ([string]::IsNullOrEmpty($Text)) { throw '-Action type 需要 -Text。' }
    Send-LltText -Text $Text
  }
  'key' {
    if ([string]::IsNullOrEmpty($Text)) { throw '-Action key 需要 -Text。' }
    Send-LltKey -Keys $Text
  }
}
