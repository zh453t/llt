<#
.SYNOPSIS
    把 llt-gui 打成 Android APK（cargo apk）。

.DESCRIPTION
    步骤：
      1. 检查 out/timetable/llt-timetable.sqlite（没有就先 `cargo run -p llt --release -- build`）；
      2. 把数据库和版本标记复制到 android/assets/timetable/（打包时进 APK）；
      3. 编译 java/ 下的输入法桥（ImeBridge）成 assets/ime.dex——清单里是
         `hasCode=false`（cargo-apk 的 native 应用就是这样），应用自己的 ClassLoader
         是空的，所以这个 dex 由 Rust 侧用 InMemoryDexClassLoader 在运行时装进去；
      4. 用 cargo apk 交叉编译并打包。

    需要：Rust 的 aarch64-linux-android / x86_64-linux-android target、
    Android SDK（$env:ANDROID_HOME 或默认的 %LOCALAPPDATA%\Android\Sdk）、
    SDK 里的 NDK（$env:ANDROID_NDK_ROOT，未设置时取 ndk/ 下最新一版）、
    cargo-apk（cargo install cargo-apk）、JDK（javac / keytool / d8 / apksigner 都要 java）。

.PARAMETER Release
    用 release 配置构建（默认 debug，装到设备上更慢但编译快）。

.PARAMETER SkipAssets
    不刷新 assets（数据库没变时省一次 41 MB 复制）。

.EXAMPLE
    pwsh -File android/build.ps1 -Release
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$SkipAssets
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $root 'out/timetable'
$dbFile = Join-Path $outDir 'llt-timetable.sqlite'
$metaFile = Join-Path $outDir 'meta.json'

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
Write-Host "SDK : $env:ANDROID_HOME"
Write-Host "NDK : $env:ANDROID_NDK_ROOT"

# apksigner / 打包工具是 .bat，要能找到 java。
if (-not $env:JAVA_HOME -and -not (Get-Command java -ErrorAction SilentlyContinue)) {
    $jbr = Join-Path $env:ProgramFiles 'Android/Android Studio/jbr'
    if (-not (Test-Path (Join-Path $jbr 'bin/java.exe'))) {
        throw "找不到 java：设置 `$env:JAVA_HOME（或安装 Android Studio）"
    }
    $env:JAVA_HOME = $jbr
}
if ($env:JAVA_HOME) {
    $env:PATH = (Join-Path $env:JAVA_HOME 'bin') + [IO.Path]::PathSeparator + $env:PATH
    Write-Host "JAVA: $env:JAVA_HOME"
}

# —— 数据库 ——
if (-not (Test-Path $dbFile)) {
    throw "找不到 $dbFile，先运行：cargo run -p llt --release -- build"
}
if (-not $SkipAssets) {
    $assets = Join-Path $PSScriptRoot 'assets/timetable'
    New-Item -ItemType Directory -Force $assets | Out-Null
    Copy-Item $dbFile (Join-Path $assets 'llt-timetable.sqlite') -Force

    # 版本标记：界面里显示的「数据 2026-11-08」也来自同一份 meta.json。
    $version = (Get-Content $metaFile -Raw | ConvertFrom-Json).data_version
    if (-not $version) { throw "$metaFile 里没有 data_version" }
    Set-Content (Join-Path $assets 'version') $version -NoNewline -Encoding utf8
    Write-Host "assets: $version + $((Get-Item (Join-Path $assets 'llt-timetable.sqlite')).Length / 1MB) MB"
}

# —— 输入法桥的 dex ——
# cargo-apk 直接编译 NativeActivity 应用（清单 hasCode=false），塞 classes.dex 进 APK
# 也没用：应用自己的 ClassLoader 是空的，loadClass 一律 ClassNotFoundException。
# 所以把 dex 放进 assets，运行时用 InMemoryDexClassLoader（API 26+）装进来。
$buildTools = Get-ChildItem (Join-Path $env:ANDROID_HOME 'build-tools') -Directory |
    Sort-Object Name -Descending | Select-Object -First 1
if (-not $buildTools) { throw "找不到 build-tools（SDK Manager 里装 build-tools;34.x）" }
$platforms = Get-ChildItem (Join-Path $env:ANDROID_HOME 'platforms') -Directory |
    Sort-Object Name -Descending
$androidJar = Join-Path $platforms[0].FullName 'android.jar'
if (-not (Test-Path $androidJar)) { throw "找不到 $androidJar（SDK Manager 里装 platforms;android-34）" }

$classDir = Join-Path $PSScriptRoot 'target/ime-classes'
$dexDir = Join-Path $PSScriptRoot 'target/ime-dex'
Remove-Item $classDir, $dexDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $classDir, $dexDir | Out-Null

# 数组要用 @() 包住再 @ 展开：只有一个文件时 .FullName 是字符串，@ 展开会拆成字符。
$sources = @(Get-ChildItem (Join-Path $PSScriptRoot 'java') -Recurse -Filter *.java |
    Select-Object -ExpandProperty FullName)
if ($sources.Count -eq 0) { throw "java/ 下没有 .java（输入法桥丢了？）" }
Write-Host "javac -> $classDir"
& javac -encoding utf-8 --release 11 -classpath $androidJar -d $classDir @sources
if ($LASTEXITCODE -ne 0) { throw "javac 失败（exit $LASTEXITCODE）" }

$classes = @(Get-ChildItem $classDir -Recurse -Filter *.class |
    Select-Object -ExpandProperty FullName)
& (Join-Path $buildTools.FullName 'd8.bat') --release --min-api 26 `
    --lib $androidJar --output $dexDir @classes
if ($LASTEXITCODE -ne 0) { throw "d8 失败（exit $LASTEXITCODE）" }

$imeDex = Join-Path $PSScriptRoot 'assets/ime.dex'
Copy-Item (Join-Path $dexDir 'classes.dex') $imeDex -Force
Write-Host "输入法桥：assets/ime.dex ($([math]::Round((Get-Item $imeDex).Length / 1KB, 1)) KB)"

# —— 打包 ——
Push-Location $PSScriptRoot
try {
    # release 包需要签名：没有证书就用 keytool 生成一份自用证书（见 Cargo.toml 的 signing.release）。
    $keystore = Join-Path $PSScriptRoot 'keystore/llt-release.jks'
    if ($Release -and -not (Test-Path $keystore)) {
        New-Item -ItemType Directory -Force (Split-Path -Parent $keystore) | Out-Null
        Write-Host "生成自用签名证书：$keystore"
        & keytool -genkeypair -keystore $keystore -alias llt -keyalg RSA -keysize 2048 `
            -validity 10000 -storepass llt-release -keypass llt-release -dname "CN=llt-gui" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "keytool 生成证书失败（exit $LASTEXITCODE）" }
    }

    $apkArgs = @('apk', 'build')
    if ($Release) { $apkArgs += '--release' }
    Write-Host "cargo $($apkArgs -join ' ')"
    & cargo @apkArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo apk 失败（exit $LASTEXITCODE）" }

    $profile = if ($Release) { 'release' } else { 'debug' }
    $apk = Join-Path $PSScriptRoot "target/$profile/apk/llt-gui.apk"
    if (-not (Test-Path $apk)) {
        throw "没找到 $apk，检查 cargo apk 的输出"
    }

    $size = [math]::Round((Get-Item $apk).Length / 1MB, 1)
    Write-Host "APK : $apk ($size MB)"
    Write-Host "安装：adb install -r `"$apk`""
} finally {
    Pop-Location
}
