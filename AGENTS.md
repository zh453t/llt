# AGENTS.md — 项目手册（给编码 Agent）

使用者向的说明在 [`README.md`](README.md)，界面细节与约束在 [`gui/AGENTS.md`](gui/AGENTS.md)，
提交规范在 [`CONTRIBUTING.md`](CONTRIBUTING.md)。动手前至少读前两份。

## 1 结构与依赖

| 路径 | 包 | 作用 |
|---|---|---|
| `crates/lltdata` | `lltdata` | 数据管线：下载 / 解包 / 解码 / 写库 / 版本比对 |
| `crates/lltdb` | `lltdb` | 只读查询层（站站 / 车次 / 车站 / 停站 / 经由 / 交路 / 餐车），只依赖 `rusqlite` 与 `jiff` |
| `cli` | `llt` | 命令行：`build` / `check` / `query` |
| `gui` | `llt-gui`（lib `llt_gui`） | egui 界面，桌面可执行 + 给 Android 复用的库 |
| `android` | `llt-gui-android` | NativeActivity 壳、JNI、Gradle；**独立 Cargo 包，不在 workspace** |
| `docs` | — | 逆向报告、Android、配置方案 |
| `scripts` | — | 本仓维护的 PowerShell 脚本：`scripts/installer`（桌面安装包）、`scripts/debug`（GUI 调试） |
| `out` / `tools` / `test` | — | 被 `.gitignore` 忽略的本机目录 |

依赖方向：`llt → lltdb`、`llt → lltdata → lltdb`、`llt-gui → lltdb`、`llt-gui → lltdata`；
`lltdb` 不碰 GUI / 网络，`lltdata` 不碰 GUI，`gui` 里**没有** `cfg(target_os)`。

- `out/`：运行时数据与产物（`out/timetable/llt-timetable.sqlite`、导出的 CSV、
  `out/data/an`、`out/installer/*.exe`、调试截图）。`out/timetable/README.md` 是本机的数据说明，
  **不在版本库**。
- `tools/`：Python 版解码脚本，本机保留、不在版本库。不要把「新增文件」放这两处，也不要在
  提交的文档里让读者去找版本库里没有的文件。
- `scripts/`：与架构相关、值得进版本库的脚本（安装包与 GUI 调试），改动就跟着提交。

## 2 常用命令（原生 Windows + pwsh）

```powershell
# 桌面（在仓库根跑，找不到库时界面会落到「数据」页）
cargo build -p llt-gui              # release 加 --release
target\debug\llt-gui.exe
cargo fmt -p llt-gui
cargo check -p llt-gui
cargo test -p llt-gui --lib

# 调试通道：egui-mcp 读界面树 / 注入点击输入 / 截图 / 按逻辑点改窗口（缺省关，发布包不含）
$env:EGUI_INSPECTION = '1'
cargo run -p llt-gui --features inspection

# 全量
cargo build --workspace --release   # -> target\release\llt.exe、llt-gui.exe
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # 目前是干净的，提交前跑一遍

# CLI
cargo run -p llt -- check
cargo run -p llt -- build
cargo run -p llt -- query train G1

# Android（需要 ANDROID_HOME / ANDROID_NDK_ROOT / JAVA_HOME；gradle 在
# C:\Users\Zhang\Programs\gradle-9.9.0\bin，必要时加进 PATH）
pwsh -File android/build.ps1 -Release     # -SkipRust 只重打包 Java
adb install -r android\app\build\outputs\apk\release\app-release.apk
adb shell am force-stop org.llt.timetable
adb shell am start -n org.llt.timetable/.MainActivity
adb logcat -s RustStdoutStderr            # Rust 侧日志（println!/eprintln! 都走这里）
adb exec-out screencap -p > out\s.png     # 截屏

# 模拟器（约 45 s 起来；AVD 名 llt。emulator 不在 PATH 时用
# %LOCALAPPDATA%\Android\Sdk\emulator\emulator.exe）
emulator -avd llt -no-window -gpu swiftshader_indirect -no-audio -no-snapshot
```

- 桌面配置在 exe 旁（debug 是 `target\debug\llt-gui.json`）；`--db` / `--config` /
  `LLT_DB` / `LLT_CONFIG` 都可覆盖。
- 模拟器可 `adb root`，因此能直接读配置：
  `adb shell cat /data/data/org.llt.timetable/files/llt-gui.json`（真机不行）。
- WSL 里跑 Windows 命令要在前面加 `pwsh -Command '...'`。

## 3 验证流程

1. `cargo fmt` / `cargo check` / `cargo test -p llt-gui --lib`（涉及 `lltdb`/`lltdata` 时跑 workspace）。
2. 界面改动**必须跑起来看**，不要只靠 `check`：
   - 桌面：点一遍三种查询 + 车次详情（浅色/深色各看一眼）。用 §4 的 egui-mcp 通道驱动，
     不需要置前窗口，也能按逻辑点改尺寸；验证列宽 / 布局时先量一次"打开前"再量"打开后"。
   - Android：`build.ps1 -Release` + `adb install`，再用 `adb shell input tap / swipe`
     注入手势（触摸屏的「按住」用 `swipe` 配 duration ≥ 800 ms），`screencap` 截图核对；
     Rust 侧要看数据时临时加 `eprintln!` + `logcat -s RustStdoutStderr`，确认后**删掉**。
3. 布局类改动至少覆盖：竖屏 / 横屏、宽窗 / 窄窗（< 820 点切底部面板、< 580 点改上下排列）、
   浅色 / 深色。
4. 汇报里写清「看了什么、怎么看的、结论是什么」；没验证到的部分直接说明。

## 4 环境与设备的坑（都踩过）

- **不要改真机的显示设置**：`wm size` / `wm density` / `settings put user_rotation` 会让用户
  看到「显示被缩小 / 方向被锁」，改完必须 `wm size reset; wm density reset` 并告知。
  需要大屏/横屏布局验证时优先用模拟器。
- 真机 `adb install` 可能弹「用户拒绝权限」（`INSTALL_FAILED_ABORTED`）：需要设备解锁并点安装；
  用 `adb shell dumpsys package org.llt.timetable | grep lastUpdateTime` 确认装的是新包。
- **截屏全黑**：屏幕休眠/锁屏时 `screencap` 返回全黑，先 `input keyevent KEYCODE_WAKEUP`
  或 `wm dismiss-keyguard`；锁屏时 `input swipe` 会拉到通知栏。
- **桌面窗口比屏幕宽**时普通屏幕截图会截不全：用 `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)`
  抓整窗（现成脚本见 §7）；远程会话里合成的鼠标点击常常无效，能用环境变量/重跑二进制验证的就别点。
- 桌面 / 模拟器 / 真机三边的 `pixels_per_point` 不同，别看截图里的像素数字直接当逻辑点用；
  需要精确数值时在代码里 `eprintln!` 打 `Rect`。
- **验证详情页别指望合成点击**：远程会话里 `mouse_event` 经常不生效，而 `SetWindowPos`
  改窗口大小后画面也不一定重绘到截图里。桌面优先走**协议调试**：
  `$env:EGUI_INSPECTION='1' cargo run -p llt-gui --features inspection`（或给
  `Start-Process` 这个环境变量），之后用 `egui-mcp` 读 AccessKit 界面树、注入点击 / 输入、
  截图、按逻辑点改窗口尺寸，不依赖窗口置前。注意：点表格**行**要落在单元格的空白处
  （文字标签会把点击吃掉）；截图里的像素不是逻辑点，要精确值就临时 `eprintln!` 打 `Rect`，
  验完删掉。兜底的老办法仍是临时加 `LLT_DEBUG_TRAIN=<车次号>` 之类的钩子自动打开详情。

## 5 代码约定（细节见 `gui/AGENTS.md`）

- **界面不分平台**：平台差异只走 `Platform`（`insets` / `ime` / `pointer` / `bars` / `dirs` /
  `files` / `control` / `font_dirs`），新增平台能力往这个结构体加字段。
- **尺寸与阈值**：控件/行高、可点热区、滚动条宽度都从 `theme` 取；宽度阈值集中在
  `WIDE_MIN_WIDTH` / `STACKED_DETAIL_MAX_WIDTH` 等常量，不要写字面量。
- **缩进用 `MarginF32`**：`egui::Margin` 字段是 `i8`，软键盘 357 点会被截断到 127。
- **查询语义在 `lltdb`**：开行判定（区间 + 周期掩码）、跨天、同城组这些口径不要为了界面效果改。
- **状态只存一份**：会话状态进 `ctx.data`（temp），要跨启动的进配置；列宽是「界面状态里的
  工作副本 + 落盘快照」，换序按列名重排，不要整份丢弃。
- **提交**：Conventional Commits，中文，subject ≤ 30 字，一个提交只做一件事；
  纯格式化与逻辑修改分开。

## 6 文档索引

- `docs/llt-format-reverse-engineering.md`：`an.db` 二进制格式与各项判定口径（开行、跨天、同城）。
- `docs/android.md`：APK 构建、系统栏 / 输入法 / 文件选择器的接入方式，验证记录与已知限制。
- `docs/gui-config.md`：`llt-gui.json` 的方案、字段、写入时机与取舍。
- `gui/README.md`：界面行为与实现细节（很长，按需查具体章节）。
- `cli/README.md`：CLI 用法与参数。
- `scripts/installer/README.md`：桌面安装包用法。
- `scripts/debug/README.md`：GUI 调试脚本清单与注意事项。
- `out/timetable/README.md`：本机数据说明（表结构、数据版本；不在版本库）。

## 7 脚本（`scripts/`，进版本库）

装载模板与调试脚本都在 `scripts/` 下，改动跟代码一起提交；两者的完整清单与参数见各自目录的
`README.md`。产物（安装包、截图）仍写被忽略的 `out/`。

### 7.1 `scripts/debug/`（GUI 调试）

- 常用几个：`focus_click.ps1`（置前 + 点击/拖拽，远程会话最稳）、`move.ps1`（改窗口尺寸验宽窄布局）、
  `shot.ps1 -Window`（PrintWindow 抓整窗，窗口比屏幕宽时必用）、`slowdrag.ps1`（长按拖拽）。
- 走协议、不走鼠标的另一条路：`$env:EGUI_INSPECTION='1'; cargo run -p llt-gui --features inspection`
  之后用 `egui-mcp` 直接读控件树 / 按 id 点击 / 按逻辑点改窗口尺寸（界面改动优先用它，
  需要坐标和 `Rect` 时比猜像素可靠）；用法见 `scripts/debug/README.md`。
- 公共逻辑在 `scripts\debug\_common.ps1`（置前、DPI、点击/拖拽、中文粘贴、打断检测），
  各脚本统一调用；`win.ps1` 无参只打印矩形，`-TopMost` 用完自动还原，不会让应用一直置顶。
- 坐标是物理像素不是逻辑点；除 `shot.ps1` 全屏模式外都要求 `llt-gui` 正在运行。
- 调试过程中有时会受用户操作影响，出现点击错位、失效等问题，脚本会自动重试或告警，重试即可。

### 7.2 `scripts/installer/`（Windows 安装包）

- `pwsh -File scripts/installer/build.ps1 [-Build] [-BundleDb] [-Version x.y.z]`；
  版本默认读 `Cargo.toml` 的 `[workspace.package] version`，产物写 `out/installer/`。
- `.iss` 里的源目录 / 输出目录相对脚本位置解析（`..\..\target\release` 等），
  改动脚本层级时这三个 `#define` 要同步改。
