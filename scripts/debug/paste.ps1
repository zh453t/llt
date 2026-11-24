<#
.SYNOPSIS
  写剪贴板并 Ctrl+V 粘贴到 llt-gui（中文可靠）；结束后尽量恢复原剪贴板文本。
.DESCRIPTION
  默认先置前，避免粘错窗口；-NoFocus 可保留旧行为。-Text 为必填。
.EXAMPLE
  pwsh -File paste.ps1 -Text "北京"
#>
param(
  [Parameter(Mandatory = $true)][string]$Text,
  [string]$ProcessName = 'llt-gui',
  [switch]$NoFocus
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')

if (-not $NoFocus) {
  $proc = Get-LltWindow -ProcessName $ProcessName
  if (-not (Set-LltForeground -Handle $proc.MainWindowHandle)) { throw '无法把 llt-gui 置前，已中止。' }
}
Send-LltText -Text $Text
