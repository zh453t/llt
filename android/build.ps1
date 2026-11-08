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
Write-Host "SDK : $env:ANDROID_HOME"
Write-Host "NDK : $env:ANDROID_NDK_ROOT"
Write-Host "JAVA: $env:JAVA_HOME"

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
