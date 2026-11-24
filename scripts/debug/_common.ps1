<#
  本机 GUI 调试脚本的公共 Win32 封装（由同目录脚本 dot-source，不要单独运行）。
  统一处理：DPI 感知、找带窗口的进程、置前、窗口可见矩形、点击/拖拽（含长按门槛）、
  中文输入、以及「用户操作打断」的前置校验。
#>

if (-not ('LltWin32' -as [type])) {
  Add-Type @"
using System;
using System.Runtime.InteropServices;
public class LltWin32 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int height, bool repaint);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, IntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
}
"@
}
# 所有坐标都是物理像素；统一开启 DPI 感知，避免各脚本行为不一致。
[LltWin32]::SetProcessDPIAware() | Out-Null

function Get-LltWindow {
  param([string]$ProcessName = 'llt-gui')
  $procs = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
  if ($procs.Count -eq 0) { throw "找不到 $ProcessName 进程；请先启动 GUI。" }
  # 多实例时只取带窗口的那个，避免 MainWindowHandle 为 0 导致误点。
  $proc = $procs | Where-Object { $_.MainWindowHandle.ToInt64() -ne 0 } | Select-Object -First 1
  if (-not $proc) { throw "找不到正在运行且带窗口的 $ProcessName 进程；请先启动 GUI。" }
  return $proc
}

function Get-LltWindowRect {
  param([IntPtr]$Handle)
  $rect = New-Object LltWin32+RECT
  # DWMWA_EXTENDED_FRAME_BOUNDS = 9，排除不可见边框；失败再退回 GetWindowRect。
  $hr = [LltWin32]::DwmGetWindowAttribute($Handle, 9, [ref]$rect, 16)
  if ($hr -ne 0) { [LltWin32]::GetWindowRect($Handle, [ref]$rect) | Out-Null }
  return $rect
}

function Set-LltForeground {
  param([IntPtr]$Handle, [int]$Attempts = 3)
  if ($Handle.ToInt64() -eq 0) { throw '窗口句柄为空（0），请把 GUI 启动完整后再试。' }
  # 远程会话里置前经常第一次失败，自动重试几次，避免一次偶发失败就中止。
  for ($i = 1; $i -le $Attempts; $i++) {
    $fg = [LltWin32]::GetForegroundWindow()
    $myThread = [LltWin32]::GetCurrentThreadId()
    $fgPid = 0
    $fgThread = [LltWin32]::GetWindowThreadProcessId($fg, [ref]$fgPid)
    $attached = $false
    if ($fgThread -ne 0 -and $fgThread -ne $myThread) {
      $attached = [LltWin32]::AttachThreadInput($myThread, $fgThread, $true)
    }
    try {
      [LltWin32]::BringWindowToTop($Handle) | Out-Null
      [LltWin32]::SetForegroundWindow($Handle) | Out-Null
      # ALT 脉冲：绕过 Windows 前台锁，远程会话里比单纯 SetForegroundWindow 可靠。
      [LltWin32]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero)
      [LltWin32]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
      Start-Sleep -Milliseconds 200
      if ([LltWin32]::GetForegroundWindow().ToInt64() -eq $Handle.ToInt64()) { return $true }
    }
    finally {
      if ($attached) { [LltWin32]::AttachThreadInput($myThread, $fgThread, $false) | Out-Null }
    }
    Start-Sleep -Milliseconds 150
  }
  Write-Warning '置前失败：当前前台窗口不是 llt-gui，合成输入可能无效，请重试。'
  return $false
}

function Assert-LltForeground {
  param([IntPtr]$Handle)
  if ([LltWin32]::GetForegroundWindow().ToInt64() -ne $Handle.ToInt64()) {
    throw '前台窗口已变化（可能被用户操作打断），请重试。'
  }
}

function Assert-LltCursorAt {
  param([int]$X, [int]$Y, [int]$Tolerance = 3, [switch]$Reapply, [switch]$WarnOnly)
  # -Reapply：按下鼠标前发现被挪动就重新定位（最常用）。
  # -WarnOnly：拖拽途中不能重设光标，只告警并继续，由使用者核对结果后决定是否重试。
  for ($i = 0; $i -le 2; $i++) {
    $pt = New-Object LltWin32+POINT
    [LltWin32]::GetCursorPos([ref]$pt) | Out-Null
    if ([math]::Abs($pt.X - $X) -le $Tolerance -and [math]::Abs($pt.Y - $Y) -le $Tolerance) { return }
    if (-not $Reapply -or $i -eq 2) { break }
    [LltWin32]::SetCursorPos($X, $Y) | Out-Null
    Start-Sleep -Milliseconds 120
  }
  $msg = "光标被移动（期望 $X,$Y，实际 $($pt.X),$($pt.Y)），请核对结果，必要时重试。"
  if ($WarnOnly) { Write-Warning $msg; return }
  throw $msg
}

function Invoke-LltClick {
  param([int]$X, [int]$Y, [IntPtr]$Handle = [IntPtr]::Zero, [int]$HoldMs = 0)
  if ($Handle.ToInt64() -ne 0) { Assert-LltForeground -Handle $Handle }
  [LltWin32]::SetCursorPos($X, $Y) | Out-Null
  Start-Sleep -Milliseconds 120
  Assert-LltCursorAt -X $X -Y $Y -Reapply
  [LltWin32]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  if ($HoldMs -gt 0) { Start-Sleep -Milliseconds $HoldMs }
  [LltWin32]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 120
}

function Invoke-LltDrag {
  param(
    [int]$X1, [int]$Y1, [int]$X2, [int]$Y2,
    [int]$Steps = 30, [int]$StepMs = 20, [int]$HoldMs = 600,
    [IntPtr]$Handle = [IntPtr]::Zero
  )
  if ($Handle.ToInt64() -ne 0) { Assert-LltForeground -Handle $Handle }
  [LltWin32]::SetCursorPos($X1, $Y1) | Out-Null
  Start-Sleep -Milliseconds 150
  Assert-LltCursorAt -X $X1 -Y $Y1 -Reapply
  [LltWin32]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  # 触摸长按门槛（AGENTS.md：约 0.4s 后才进入拖列宽/换序）；期间光标被碰动只告警，
  # 因为拖拽途中重设光标反而会把内容拖偏。
  Start-Sleep -Milliseconds $HoldMs
  Assert-LltCursorAt -X $X1 -Y $Y1 -WarnOnly
  for ($i = 1; $i -le $Steps; $i++) {
    $nx = [int]($X1 + ($X2 - $X1) * $i / $Steps)
    $ny = [int]($Y1 + ($Y2 - $Y1) * $i / $Steps)
    [LltWin32]::SetCursorPos($nx, $ny) | Out-Null
    Start-Sleep -Milliseconds $StepMs
  }
  Start-Sleep -Milliseconds 100
  [LltWin32]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 150
}

function Send-LltText {
  param([Parameter(Mandatory = $true)][string]$Text)
  Add-Type -AssemblyName System.Windows.Forms
  $prev = $null
  try { $prev = Get-Clipboard -Raw -ErrorAction SilentlyContinue } catch { }
  try {
    # 走剪贴板粘贴：SendKeys 对中文不可靠，也不必转义 + ^ % ~ ( ) { } [ ]。
    Set-Clipboard -Value $Text
    Start-Sleep -Milliseconds 150
    [System.Windows.Forms.SendKeys]::SendWait('^v')
    Start-Sleep -Milliseconds 300
  }
  finally {
    if ($null -ne $prev) { try { Set-Clipboard -Value $prev } catch { } }
  }
}

function Send-LltKey {
  param([Parameter(Mandatory = $true)][string]$Keys)
  Add-Type -AssemblyName System.Windows.Forms
  # SendKeys 特殊字符需转义：+ ^ % ~ ( ) { } [ ]。仅发按键，不用来输中文。
  [System.Windows.Forms.SendKeys]::SendWait($Keys)
  Start-Sleep -Milliseconds 150
}
