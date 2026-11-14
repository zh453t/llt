# AGENTS.md

## 目录

- `crates/lltdata`：时刻表数据解析 / 下载更新。
- `crates/lltdb`：SQLite 查询（站站、车次、车站、停站、经由）。
- `cli`：命令行（`llt`）。
- `gui`：桌面 + Android 界面（egui / eframe）。
- `android`：APK 壳（独立 Cargo 包，不参与 workspace）。
- `docs`：android / 配置 / 数据格式说明。
- `out/timetable/llt-timetable.sqlite`：运行时数据库。

## 电脑端

```powershell
cargo build -p llt-gui            # release 加 --release
target\debug\llt-gui.exe          # 在仓库根运行，自动找 out/timetable 的库
cargo fmt -p llt-gui
cargo check -p llt-gui
cargo test -p llt-gui --lib
```

- 配置：`<exe 同目录>\llt-gui.json`（debug 在 `target\debug\`）。
- 可加 `--db <path>` 或设 `LLT_DB` 指定库。

## Android

```powershell
# 环境：ANDROID_HOME、ANDROID_NDK_ROOT、JAVA_HOME、GRADLE_HOME（或 PATH 里有 gradle）
# Gradle 位于 C:\Users\Zhang\Programs\gradle-9.9.0\bin
pwsh -File android/build.ps1 -Release          # -SkipRust 只重打包 Java
adb install -r android\app\build\outputs\apk\release\app-release.apk

adb shell am force-stop org.llt.timetable
adb shell am start -n org.llt.timetable/.MainActivity
adb logcat -s RustStdoutStderr                 # Rust 侧日志
adb exec-out screencap -p > out\s.png          # 截屏
```

模拟器：

```powershell
emulator -avd llt -no-window -gpu swiftshader_indirect -no-audio -no-snapshot
```

WSL 下所有命令前加 `pwsh -Command '...'`。

## GUI 功能
详见 `gui/AGENTS.md`

## 提交规范
见 `CONTRIBUTING.md`