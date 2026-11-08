<#
.SYNOPSIS
    把 llt-gui 打成 Android APK（cargo ndk + Gradle）。

.DESCRIPTION
    步骤：
      1. Rust cdylib：`cargo ndk` 交叉编译 `android/` 这个包，产物直接放进
         `app/src/main/jniLibs/<abi>/`；
      2. Java + 打包：Gradle（AGP）编 `MainActivity` / `ImeBridge`、合并清单、打 APK；
      3. release 包需要签名：没有证书就用 keytool 生成 `keystore/llt-release.jks`。

    APK **不内置数据库**：41 MB 的数据包不随包发，第一次启动在「数据」页下载或导入。

    需要：Rust 的 aarch64-linux-android / x86_64-linux-android target、Android SDK
    （$env:ANDROID_HOME）、SDK 里的 NDK（$env:ANDROID_NDK_ROOT）、JDK 17+
    （$env:JAVA_HOME）、cargo-ndk、Gradle 8.9+（$env:GRADLE_HOME 或 PATH）。

.PARAMETER Release
    用 release 配置（默认 debug）。

.PARAMETER SkipRust
    跳过 `cargo ndk`（.so 没变时省一次交叉编译）。

.EXAMPLE
    pwsh -File android/build.ps1 -Release
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$SkipRust
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# —— 环境 ——
$java_version = 'n/a'
$java_major = 0
if (-not $env:ANDROID_HOME) {
    $env:ANDROID_HOME = Join-Path $env:LOCALAPPDATA 'Android/Sdk'
}
if (-not (Test-Path $env:ANDROID_HOME)) {
    throw "找不到 Android SDK：$env:ANDROID_HOME（设置 `$env:ANDROID_HOME 后重试）"
}
if (-not $env:ANDROID_NDK_ROOT) {
    $ndkDirs = Get-ChildItem (Join-Path $env:ANDROID_HOME 'ndk') -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending
    if (-not $ndkDirs) {
        throw "找不到 NDK：$env:ANDROID_HOME\ndk 是空的（用 SDK Manager 装 ndk;27.x）"
    }
    $env:ANDROID_NDK_ROOT = $ndkDirs[0].FullName
}
if (-not $env:JAVA_HOME -and -not (Get-Command java -ErrorAction SilentlyContinue)) {
    $jbr = Join-Path $env:ProgramFiles 'Android/Android Studio/jbr'
    if (-not (Test-Path (Join-Path $jbr 'bin/java.exe'))) {
        throw "找不到 java：设置 `$env:JAVA_HOME（或安装 Android Studio）"
    }
    $env:JAVA_HOME = $jbr
}
if ($env:JAVA_HOME) {
    $env:PATH = (Join-Path $env:JAVA_HOME 'bin') + [IO.Path]::PathSeparator + $env:PATH
}
# `java -version` 打到 stderr，且首行形如 `openjdk version "21.0.5" 2024-10-15`。
$javaVersionLine = (& java -version 2>&1 | Select-Object -First 1)
if ($javaVersionLine -match '(\d+)(\.(\d+))?') {
    $java_major = [int]$Matches[1]
    $java_version = $javaVersionLine.ToString().Trim()
}
Write-Host "SDK : $env:ANDROID_HOME"
Write-Host "NDK : $env:ANDROID_NDK_ROOT"
Write-Host "JAVA: $env:JAVA_HOME ($java_version)"

# Android Studio 更新会换掉自带的 jbr（JDK 版本）和 SDK 组件，先把前置条件摊开：
# 失败时不用再去猜是 JDK 太新、还是 platform/build-tools 被移走了。
if ($java_major -gt 23) {
    Write-Warning "JDK $java_major 高于 Gradle 8.13 支持的上限（23）。Studio 更新后如果构建报 "Unsupported class file major version"，请把 `$env:JAVA_HOME 指回旧版 jbr，或把 GRADLE_HOME 换到 Gradle 8.14+。"
}
if (-not (Test-Path (Join-Path $env:ANDROID_HOME 'platforms/android-34'))) {
    Write-Warning "缺少 SDK platform 34（compileSdk）。用 SDK Manager 装 platforms;android-34，或改 app/build.gradle.kts 的 compileSdk。"
}
$buildTools = Get-ChildItem (Join-Path $env:ANDROID_HOME 'build-tools') -Directory -ErrorAction SilentlyContinue
if (-not $buildTools) {
    Write-Warning "缺少 build-tools。用 SDK Manager 装 build-tools;34.0.0。"
}

# Gradle：优先环境变量里指定的，其次 PATH，最后用仓库里的 wrapper。
$gradle = $null
if ($env:GRADLE_HOME) {
    $candidate = Join-Path $env:GRADLE_HOME 'bin/gradle.bat'
    if (Test-Path $candidate) { $gradle = $candidate }
}
if (-not $gradle) {
    $onPath = Get-Command gradle -ErrorAction SilentlyContinue
    if ($onPath) { $gradle = $onPath.Source }
}
if (-not $gradle) {
    $wrapper = Join-Path $PSScriptRoot 'gradlew.bat'
    if (Test-Path $wrapper) { $gradle = $wrapper }
}
if (-not $gradle) {
    throw "找不到 Gradle 8.9+：设置 `$env:GRADLE_HOME，或把 gradle 加进 PATH"
}
Write-Host "GRADLE: $gradle"

Push-Location $PSScriptRoot
try {
    # —— 1) Rust .so ——
    if (-not $SkipRust) {
        $cargoArgs = @('ndk', '-t', 'arm64-v8a', '-t', 'x86_64', '-o', 'app/src/main/jniLibs', 'build')
        if ($Release) { $cargoArgs += '--release' }
        Write-Host "cargo $($cargoArgs -join ' ')"
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo ndk 失败（exit $LASTEXITCODE）" }
    }

    # —— 2) 签名证书（只有 release 需要） ——
    $keystore = Join-Path $PSScriptRoot 'keystore/llt-release.jks'
    if ($Release -and -not (Test-Path $keystore)) {
        New-Item -ItemType Directory -Force (Split-Path -Parent $keystore) | Out-Null
        Write-Host "生成自用签名证书：$keystore"
        & keytool -genkeypair -keystore $keystore -alias llt -keyalg RSA -keysize 2048 `
            -validity 10000 -storepass llt-release -keypass llt-release -dname "CN=llt-gui" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "keytool 生成证书失败（exit $LASTEXITCODE）" }
    }

    # —— 3) Gradle 打包 ——
    $task = if ($Release) { 'assembleRelease' } else { 'assembleDebug' }
    Write-Host "gradle $task"
    & $gradle $task --console=plain
    if ($LASTEXITCODE -ne 0) { throw "gradle 失败（exit $LASTEXITCODE）" }

    $variant = if ($Release) { 'release' } else { 'debug' }
    $apk = Join-Path $PSScriptRoot "app/build/outputs/apk/$variant/app-$variant.apk"
    if (-not (Test-Path $apk)) { throw "没找到 $apk" }
    $size = [math]::Round((Get-Item $apk).Length / 1MB, 1)
    Write-Host "APK : $apk ($size MB)"
    Write-Host "安装：adb install -r `"$apk`""
} finally {
    Pop-Location
}
