# llt-gui 的 Android 方案

把 egui 界面装进 APK，在 Android 上跑起来，并且能在应用里**下载数据包、解码建库、
导入导出备份**。界面交互规则仍以 [`../gui/README.md`](../gui/README.md) 为准。

> **实现状态**：模拟器（Android 13 / x86_64）与真机（vivo V2509A / Android 17 / arm64）
> 都跑通了。真机上验过：覆盖安装保留旧库、中文界面与字体、系统栏让位、车站查询、
> 检查更新（20261108 → 20261114）、导出备份到系统「下载」。

---

## 1 分层：谁负责什么

```
crates/lltdata   数据管线：下载 / 解包 / 解码 / 写库 / 版本比对 / 本地库体检
crates/lltdb     只读查询层
gui/             界面（含「数据」页）+ Platform 抽象；桌面可执行文件 llt-gui
android/         平台胶水：入口、系统栏缩进、输入法桥、文件选择器 + Gradle 打包
cli/             参数解析 + 文本渲染（llt）
```

三条边界：

- **管线与平台无关**，三端共用（CLI、桌面 GUI、Android），解码格式一变不会出现两份实现。
- **界面不分平台**：`gui/` 里没有 `cfg(target_os)`，差异全走 `Platform` 结构体
  （`insets` / `ime` / `pointer` / `dirs` / `files`）。
- **`android/` 只放平台代码**：`android-activity`、JNI、Java 源码、清单、Gradle 工程。
  它不参与根 workspace（`exclude = ["android"]`），桌面构建不会因为 Android 多编译一个
  用不上的 `.dll`。

## 2 运行时行为

### 2.1 数据从哪来

**APK 不内置数据库。** 数据包 41 MB，随包发会让 APK 翻倍，而且很快过时（官方几乎每天
更新）。第一次启动时库不存在，界面直接落在「数据」页：

- **下载并重建**：`lltdata::build` 在后台线程跑——查版本接口 → 下载 2.3 MB 数据包
  （zip）→ 解包 64 个分片 → 解码 → 在库目录里写临时文件再 `rename`（正在读旧库的进程
  不会读到半个库）→ 界面重开数据库。
- **导入备份**：系统文件选择器（SAF）挑一份 `.sqlite`，校验结构版本与必需表 → 把当前库
  先另存一份 → 原子替换。
- **检查更新**：拉 `android.ver`，用 `lltdata::decide` 给结论（与 `llt check` 同一套判断）。

### 2.2 目录

| 用途 | 位置 |
|---|---|
| 数据库 | `/data/user/0/org.llt.timetable/files/llt-timetable.sqlite` |
| 配置 | `/data/user/0/org.llt.timetable/files/llt-gui.json` |
| 导入暂存（SAF 选中的文件拷到这里） | `.../files/import/` |
| 导入前的自动备份 | 同目录，`llt-timetable-<数据版本>-<时间>.sqlite` |
| 构建临时目录 | `/data/user/0/org.llt.timetable/cache/`（没有可写的 `/tmp`） |
| 导出 | 由用户在系统「保存到」里决定（下载、网盘、U 盘…） |

全部不需要存储权限；私有目录与 `cacheDir` 由系统给，SAF 走 `ContentResolver`。

### 2.3 系统栏与软键盘

Android 的窗口是 edge-to-edge 的，内容会画到状态栏、导航栏、刘海底下。egui/eframe 只在
iOS 上填 `safe_area_insets`，Android 这边自己问 Java 侧
（`getRootWindowInsets()` + `WindowInsets.Type.ime()`），把上下左右与键盘高度以**物理像素**
交给 `Platform::insets`，界面缩进根 `Ui` 并换算成逻辑点。最多每 150 ms 问一次：
键盘、导航栏、旋转屏幕都不是应用能直接收到的事件，而帧本来就跟着事件走。

**键盘不参与布局**：`MainActivity.onCreate` 里把窗口设成
`SOFT_INPUT_ADJUST_NOTHING`，底部缩进只取 `WindowInsets.Type.systemBars()`（导航栏），
不把 `Type.ime()` 算进去。效果是键盘「浮」在界面之上——底栏、列表、输入框都不移动，
打字时不会因为键盘弹出/收起而整屏跳动。代价是键盘会盖住底栏；三个查询页都是
「边打边查」，所以打字过程中不需要底栏，点一下结果区收键盘即可回到可操作状态。

想改回「界面收缩到键盘上方」，把 `android/src/lib.rs` 里的 `SHRINK_FOR_IME` 改成 `true`
（同时要把 `ADJUST_NOTHING` 去掉或改成 `ADJUST_RESIZE`）。

缩进量用 `egui::epaint::MarginF32`，**不要**用 `egui::Margin`：后者字段是 `i8`，
软键盘在这台机器上是 983 px ≈ 357 点，换算时会被截断到 127——底栏就停在键盘后面不动。
`MarginF32` 自带 `sum()` 与 `Rect - MarginF32`，拼安全矩形一行就能写完。

也没有用 `egui::SafeAreaInsets`：它是 `RawInput.safe_area_insets` 的类型，语义是「被系统
UI 遮住的屏幕区域」，只包含状态栏/导航栏/刘海；我们这里还要把软键盘高度算进去，
而且并不回填 `RawInput`，套一层只会让语义变模糊。

### 2.4 中文字体

Android 没有 fontconfig，系统字体都在 `/system/fonts`（中文是 `NotoSansCJK-Regular.ttc`）。
`gui/src/fonts.rs` 的候选目录里补了 `/system/fonts` 与 `/product/fonts`；真实粗体在多数
ROM 上找不到独立字形文件，会退化成普通字重而不是豆腐块。

### 2.5 中文输入法

`NativeActivity` 的窗口里没有带 `InputConnection` 的 View，输入法只能「发按键」，
中文合成与候选整词上屏会整个丢掉。所以 `MainActivity.onCreate` 里挂一个 1×1 的透明
`ImeBridge` View 抢焦点，用自定义 `InputConnection` 收 `commitText` / `setComposingText`，
经 JNI 交给 Rust，再由界面转成 `egui::Event::Ime`。详见 `android/src/ime.rs` 与
`android/app/src/main/java/org/llt/timetable/ImeBridge.java`。

**为什么点输入框会把键盘弹掉**（已修）：egui 每次点击输入框都会
`Memory::request_focus` → `interrupt_ime`，egui-winit 于是在帧末
`set_ime_allowed(false)` + `(true)`——在 Android 上就是 `hide_soft_input` 收键盘，
而「再弹」走 `showSoftInput(DecorView)`，因为焦点在我们的桥 View 上被系统忽略。
所以点击之后不能立刻 show（那一帧结束时就被收掉了），而是标记「补弹两次」，
40 ms 后重新请求一帧再 `showSoftInput`；并且**不要** `restartInput`
（它会让键盘闪一下）。

### 2.6 文件选择器（SAF）

`ACTION_OPEN_DOCUMENT` / `ACTION_CREATE_DOCUMENT` 的结果只在
`Activity.onActivityResult` 里回来，而 `NativeActivity` 不转发——所以入口是
`MainActivity extends NativeActivity`。字节拷贝也放在 Java 侧（`ContentResolver` 的流是
一等公民，从 Rust 逐块过 JNI 慢一个数量级），Rust 只收「本地路径」/「用户选的位置」。

### 2.7 渲染

wgpu。`options()` 把 `device_descriptor` 改成用适配器自己上报的上限
（`adapter.limits()`）：模拟器的 SwiftShader Vulkan 只给
`max_uniform_buffer_binding_size = 16 KiB`，照抄 wgpu 默认的 64 KiB 会直接开不了设备。

## 3 打包（Gradle）

```
android/
  Cargo.toml            Rust cdylib（libllt_gui_android.so），不参与根 workspace
  src/lib.rs            android_main：目录、Insets、输入法、选择器 → llt_gui::run_with
  src/ime.rs            输入法 JNI
  src/files.rs          SAF 的 JNI
  build.ps1             cargo ndk → gradle 一步到位
  settings.gradle.kts   AGP 版本与仓库（国内镜像在前）
  gradle.properties     非 proxy / 内存
  app/build.gradle.kts  ABI、minSdk、签名、打包
  app/src/main/AndroidManifest.xml
  app/src/main/java/org/llt/timetable/{MainActivity,ImeBridge}.java
  app/src/main/jniLibs/<abi>/libllt_gui_android.so   （cargo ndk 产物，不入库）
```

- 清单：`INTERNET`（下载）、`usesCleartextTraffic="true"`（官方接口是明文 HTTP）、
  `minSdk 26`、`compileSdk/targetSdk 36`、`hasCode=true`（因为有自己的 Java 类）。
  targetSdk 36 在 Android 15+ 上强制 edge-to-edge，正好与「自己问 WindowInsets」的做法一致。
- 打包的两个 ABI：`arm64-v8a`（真机）+ `x86_64`（模拟器）。
- `.so` 压缩进 APK：两个 ABI 加起来 30 MB，不压 APK 直接 29 MB；压完 13 MB。
- 签名：`build.ps1 -Release` 首次运行用 keytool 生成 `android/keystore/llt-release.jks`
  （不进版本库）；要分发请换自己的证书。

### 为什么不继续用 cargo-apk

cargo-apk 0.10 只打 `hasCode=false` 的纯 NativeActivity：没有自己的 Java 类就没有
`onActivityResult`，SAF 接不上；而且它已经不再维护。换成 Gradle 之后 Java 源码、清单、
ABI、签名都是标准配置，`ImeBridge` 也省掉了
「assets 里的 dex + `InMemoryDexClassLoader`」那一套。

## 4 环境（Windows）

| 组件 | 版本 / 位置 |
|---|---|
| Rust | 1.99.1；`rustup target add aarch64-linux-android x86_64-linux-android` |
| Android SDK | `%LOCALAPPDATA%\Android\Sdk`（platform 36、build-tools 36.0.0、platform-tools） |
| Android NDK | `%LOCALAPPDATA%\Android\Sdk\ndk\27.3.13750724`（r27d） |
| JDK | Android Studio 自带 `jbr`（25.0.4） |
| Gradle | 9.9.0（`GRADLE_HOME` 指向解压目录）；AGP 9.5.1 |
| cargo-ndk | `cargo install cargo-ndk` |
| 模拟器镜像 | `system-images;android-33;default;x86_64` |

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
# NDK：https://dl.google.com/android/repository/android-ndk-r27d-windows.zip
#      -> %LOCALAPPDATA%\Android\Sdk\ndk\27.3.13750724
# Gradle：https://mirrors.cloud.tencent.com/gradle/gradle-9.9.0-bin.zip
#      -> 任意目录，设 $env:GRADLE_HOME
```

`build.ps1` 用的环境变量（不设就用默认值）：`ANDROID_HOME`、`ANDROID_NDK_ROOT`、
`JAVA_HOME`、`GRADLE_HOME`。脚本开头会打印 SDK / NDK / JDK（含版本）/ Gradle 的路径，
并在 JDK 版本高于 Gradle 支持上限、或缺少 platform 36 / build-tools 时直接给出警告。

### 工具链会被 Android Studio 更新改掉

Studio 更新会替换自带的 `jbr`（JDK）、并可能顺带更新 SDK 组件，而这三件事是本仓库
**写死**的：AGP `9.5.1`、Gradle `9.9.0`、`compileSdk/targetSdk 36`、NDK `r27d`。

| 现象 | 原因 | 处理 |
|---|---|---|
| `Unsupported class file major version ...` | Studio 的 jbr 比当前 Gradle 支持的新（Gradle 9.9 已支持 JDK 25） | 升 `GRADLE_HOME` 到新版本，或用 `LLT_GRADLE_JAVA_HOME` 指定一个较旧的 JDK（只给 Gradle 用） |
| `Android Gradle plugin requires Java 17` | jbr 或 `JAVA_HOME` 低于 JDK 17 | 把 `JAVA_HOME`/`LLT_GRADLE_JAVA_HOME` 指到 JDK 17+ |
| `Failed to find Build Tools revision ...` | build-tools 被移走 | SDK Manager 装 `build-tools;36.0.0` |
| `platforms;android-36` 缺失提示 | 同上 | 装 `platforms;android-36`，或改 `app/build.gradle.kts` 的 `compileSdk` |
| `NDK not found` / `cargo ndk` 报找不到 NDK | Studio 重装时删了 NDK | 重新解压 r27d 到 `$ANDROID_HOME
dk.3.13750724`（`build.ps1` 取 `ndk/` 下最新一版） |
| 升级 Studio 时提示「`jbr` 被占用，无法删除」 | Gradle 守护进程是拿 `jbr` 起的 `java.exe`，它活着目录就删不掉 | 先跑 `pwsh -File android/stop-java.ps1`（先试 `gradle --stop`，不行就按路径结束进程），再升级 |
| `gradle --stop` 报 `could not open .../jbr/lib/jvm.cfg` | 升级已经把 `jbr` 删了一半，Gradle 自己都起不来 | 同上，用 `stop-java.ps1` 兜底 |
| 依赖下载全部失败且报 `SSL misconfiguration` | `~/.gradle/gradle.properties` 里的代理没开 | 项目里的 `android/gradle.properties` 已用 `systemProp.*.nonProxyHosts=*` 绕开；要用代理就改那两行 |

**升级前**先停掉占用 `jbr` 的守护进程，否则 Studio 会提示文件被占用删不掉：

```powershell
pwsh -File android/stop-java.ps1     # 先 gradle --stop，失败再按路径结束 java.exe
```

更新完想确认没坏，跑一遍：

```powershell
pwsh -File android/build.ps1 -Release      # 开头会打印 SDK/NDK/JDK/Gradle 版本
adb install -r android/app/build/outputs/apk/release/app-release.apk
```

真机/模拟器上再点一遍「数据 → 检查更新」，能拿到线上版本就说明网络与清单都没问题。

> **本机代理坑**：`~/.gradle/gradle.properties` 里配了 `127.0.0.1:7890`，代理没开时
> Gradle 会报 `SSL misconfiguration`。项目的 `android/gradle.properties` 里用
> `systemProp.http.nonProxyHosts=*` 让它直连；依赖下载优先走阿里云镜像
> （`settings.gradle.kts`），AGP 用 `buildscript classpath` 引入而不是 `plugins {}`
> ——插件标记在部分镜像上解析不稳。

## 5 构建与安装

```powershell
pwsh -File android/build.ps1 -Release    # cargo ndk → gradle assembleRelease
adb install -r android/app/build/outputs/apk/release/app-release.apk
adb logcat -s RustStdoutStderr           # Rust 侧日志（android-activity 重定向到 logcat）
```

调试包（`-Debug`，默认）会带 `.debug` 后缀的包名，可以和发布包共存。

模拟器：

```powershell
avdmanager create avd -n llt -k "system-images;android-33;default;x86_64" -d pixel_5
emulator -avd llt -no-window -gpu swiftshader_indirect -no-audio -no-snapshot
adb shell screencap -p /sdcard/s.png   # 无窗口也能截屏
```

## 6 界面上的 Android 差异

- 控件高度 44、顶栏 36、列表行 38（表头 32）、详情行 34（表头 30）、滚动条 14
  （`theme::Pointer::Touch`），关掉 `selectable_labels` 让整行可点。
- 输入框里的文字水平、垂直居中（`TextEdit::align(CENTER_CENTER)`），只在触摸屏启用。
- **底栏**（`body()` 里的 `tab_bar_bottom` + `action_bar`）：标签页四等分放最底下，
  上面一行是当前页的动作与计数。凡是「不是输入框、又占表格上方」的控件都收进来了：
  站站是 `模糊 / 查询 + 计数`，车次是 `清空 + 计数`，车站是 `清空 / 模糊 + 计数`。
  （「交换」不在这里：它是两个输入框中间的 `⇌`。）
  数据页不画动作行。
- 头部只剩一行 `标题 + 日期 + 主题`（桌面仍是标题行 + 标签行 + 日期行）。
- 筛选 chip（站站的车站筛选、车站的同城车站勾选）用 `theme::chip_height()`：触摸 30 点、
  桌面 22 点，一行能放更多个，也少占一行高。
- 站站结果在竖屏用六列表：`状态 车次 出发站 出发时间 到达站 到达时间`。固定列宽、
  允许横向溢出；表格放在**一个横竖都能滚**的 `ScrollArea::both` 里，
  表格自身 `drag_to_scroll(Never)`——嵌套两个 ScrollArea 时内层会把手指拖动全吃掉，
  结果横向只能拖滚动条（踩过的坑）。
- 车次详情是**整页**（列表被替换），整页只有一个滚动块：
  经由（窄条）→ 时刻表 → 杂项，标题栏固定在上面；经由页是整页模态。
- 「数据」页是第四个标签页，桌面也有（同一份代码，桌面只是没有 SAF）。
- 标签页名称：站站 / 车次 / 车站 / 数据（桌面同一套）。

## 7 验证记录

| 项 | 结果 |
|---|---|
| 首启无数据 | 落在「数据」页，显示「还没有时刻表数据」+ 下载/导入入口 |
| 应用内下载 | 2.3 MB → 数据 20261114 · 车站 3330 · 车次 15131，42.1 MB，约 3 秒 |
| 重开数据库 | 下载完成后自动切回查询页，`G12` 查出 68 条 |
| 导出备份 | 系统「保存到」→ Downloads，`llt-timetable-20261114-20261108-0250.sqlite`（42.1 MB）|
| 导入备份 | 系统选择器选中同一份 → 校验通过 → 「已导入 …（数据 20261114）」|
| 系统栏 / 软键盘 | 状态栏 136 px、导航栏 132 px、键盘 912 px 三种缩进都正确让开了 |
| 键盘弹出时的布局 | 键盘浮在界面之上，底栏与列表**原位不动**（`ADJUST_NOTHING` + 只算 systemBars） |
| 输入法 | 中文合成、`G12` 输入、回车触发查询都正常 |
| 真机（vivo / Android 17 / arm64） | 覆盖安装保留原库；查询、详情、经由、底栏、输入法均正常；检查更新 20261108 → 20261114；状态栏 140 px 让开正确 |
| 工具链升级后（JDK 25 / Gradle 9.9 / AGP 9.5.1 / compileSdk 36） | 真机与模拟器都重新安装验证通过；targetSdk 36 的强制 edge-to-edge 下缩进仍然正确；明文 HTTP 的版本接口照常可用 |
| 底栏 | 标签页 + 动作行落在底部；车次页从「输入 + 计数 + 表格」变成「输入 + 表格」，多出约 2 行 |
| 紧凑头部 | 一行头部（标题 + 日期 + 主题），比原来三行多出约 2 行列表空间 |
| APK 体积 | 13.2 MB（不含数据库）|

## 8 已知限制与后续

1. **只验过一台真机**（vivo / Android 17）。其它 ROM 的字体、刘海、手势导航
   `getRootWindowInsets()` 返回值，以及明文 HTTP 是否被运营商劫持，仍需实测。
2. **返回键**没有接线：按返回不会收起详情/经由页（应改成「先关经由，再收详情，最后退出」）。
3. **剪贴板**：Android 侧没有开 eframe 的 `clipboard`（arboard 无 Android 后端），
   输入框里不能长按粘贴。
4. **键盘盖住底栏**：为了让布局不跳动，键盘是「浮层」而不是挤压布局，打字时看不到
   底栏（三个查询页都边打边查，点结果区收键盘即可）。
4. **粗体是「假」粗体**（见 §2.4）。要真粗体得往 assets 塞一份思源黑体 Bold 子集。
5. **备份校验只到结构层**：查 `application_id`、结构版本、必需表；不做内容哈希。
6. **下载失败只能整份重来**：没有断点续传；镜像只有明文 HTTP，被劫持/不可达时
   只能靠「导入备份」兜底。
7. **没有应用图标**，也没有 32 位 ABI（`armeabi-v7a` / `x86`）。
8. **数据页的日志只有 3 行**：够看「下载 / 解包 / 解码 / 写入」，不做滚动日志。
