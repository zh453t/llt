<#
.SYNOPSIS
    停掉会占用 Android Studio 自带 jbr（JDK）的进程，供 Studio 升级 / 删除目录前使用。

.DESCRIPTION
    Gradle 守护进程是用 `JAVA_HOME`（默认指向 Studio 的 `jbr`）起的 java.exe，
    只要它还活着，`jbr` 目录就被占用，Studio 升级时会提示「文件被占用，无法删除」。

    正常情况下 `gradle --stop` 就能收掉；但 Studio 升级把 `jbr/lib` 删了一半时，
    `gradle --stop` 自己都起不来（`could not open .../lib/jvm.cfg`），这时只能直接结束
    进程——本脚本先试优雅方式，再按可执行文件路径兜底 kill。

.PARAMETER JavaHome
    要清理的 JDK 目录；默认取 `$env:STUDIO_JBR`、`$env:JAVA_HOME`，
    再退回 `%ProgramFiles%\Android\Android Studio\jbr`。

.PARAMETER WhatIf
    只列出会结束的进程，不动手。

.EXAMPLE
    pwsh -File android/stop-java.ps1
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$JavaHome
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$targets = @()
foreach ($candidate in @(
        $JavaHome,
        $env:STUDIO_JBR,
        $env:JAVA_HOME,
        (Join-Path $env:ProgramFiles 'Android/Android Studio/jbr')
    )) {
    if ($candidate -and (Test-Path $candidate)) { $targets += $candidate; break }
}
if (-not $targets) {
    Write-Host "找不到要清理的 JDK 目录（用 -JavaHome 指定）"
    exit 0
}
$jdk = (Resolve-Path $targets[0]).Path
Write-Host "目标 JDK：$jdk"

# 1) 先走 Gradle 自己的关闭通道（守护进程还能跑的时候最快也最干净）。
$gradle = $null
if ($env:GRADLE_HOME) {
    $candidate = Join-Path $env:GRADLE_HOME 'bin/gradle.bat'
    if (Test-Path $candidate) { $gradle = $candidate }
}
if (-not $gradle) {
    $onPath = Get-Command gradle -ErrorAction SilentlyContinue
    if ($onPath) { $gradle = $onPath.Source }
}
if ($gradle -and (Test-Path (Join-Path $jdk 'bin/java.exe'))) {
    if ($PSCmdlet.ShouldProcess('Gradle 守护进程', 'gradle --stop')) {
        try {
            & $gradle --stop 2>&1 | Select-Object -Last 1 | ForEach-Object { Write-Host $_ }
        } catch {
            Write-Warning "gradle --stop 失败（守护进程可能已经跑不起来）：$($_.Exception.Message)"
        }
    }
}

# 2) 兜底：按可执行文件路径找出还在用这个 JDK 的进程并结束。
$victims = Get-CimInstance Win32_Process -Filter "Name='java.exe' OR Name='javaw.exe'" |
    Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($jdk, 'OrdinalIgnoreCase') }

if (-not $victims) {
    Write-Host "没有进程占用该 JDK。"
    exit 0
}
foreach ($process in $victims) {
    $label = "$($process.ProcessId) $($process.ExecutablePath)"
    if ($PSCmdlet.ShouldProcess($label, 'Stop-Process -Force')) {
        try {
            Stop-Process -Id $process.ProcessId -Force -ErrorAction Stop
            Write-Host "已结束：$label"
        } catch {
            Write-Warning "结束 $label 失败：$($_.Exception.Message)"
        }
    }
}

Start-Sleep -Milliseconds 500
$left = Get-CimInstance Win32_Process -Filter "Name='java.exe' OR Name='javaw.exe'" |
    Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($jdk, 'OrdinalIgnoreCase') }
if ($left) {
    Write-Warning "仍有进程占用（可重跑一次或重启系统）：$($left.ProcessId -join ', ')"
} else {
    Write-Host "该 JDK 已无人占用，可以升级 / 删除。"
}
