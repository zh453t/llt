<#
.SYNOPSIS
  截图：默认抓整块虚拟屏幕；-Window 用 PrintWindow + DWM 可见框抓整个 llt-gui 窗口。
.DESCRIPTION
  窗口比屏幕宽时 CopyFromScreen 会截不全；-Window 用 DWM 可见框排除不可见边框，
  已在本机验证 PW_RENDERFULLCONTENT 能抓到 wgpu 渲染内容（非黑图）。
.EXAMPLE
  pwsh -File shot.ps1 -Path out\shot.png
  pwsh -File shot.ps1 -Window -Path out\shot-window.png
#>
param(
  # 默认落在仓库根的 out\（被 .gitignore 忽略），不污染版本库
  [string]$Path = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'out\shot.png'),
  [switch]$Window,
  [string]$ProcessName = 'llt-gui'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_common.ps1')
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$dir = Split-Path -Parent $Path
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

if ($Window) {
  $proc = Get-LltWindow -ProcessName $ProcessName
  $h = $proc.MainWindowHandle
  $rect = Get-LltWindowRect -Handle $h
  $bmp = New-Object System.Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $dc = $g.GetHdc()
  try {
    # PW_RENDERFULLCONTENT = 0x2，GPU 渲染的窗口必须带这个标志
    if (-not [LltWin32]::PrintWindow($h, $dc, 0x00000002)) { throw 'PrintWindow 调用失败' }
  }
  finally {
    $g.ReleaseHdc($dc)
    $g.Dispose()
  }
}
else {
  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bmp.Size)
  $g.Dispose()
}

try { $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png) }
finally { $bmp.Dispose() }
Write-Output $Path
