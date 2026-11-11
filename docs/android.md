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
`gui/src/fonts.rs` 的候选目录里补了 `/system/fonts` 与 `/product/fonts`。

**粗体随 APK 带一份**：系统里只有 `NotoSansCJK-Regular.ttc`，没有粗体字形，标题、
表头、车次号这些本该加粗的地方在 Android 上会退化成普通字重（`RichText::strong()` 只改
颜色，而我们把 `override_text_color` 设成了正文色，连颜色也看不出来）。

做法：用 Noto Sans SC Bold（OFL-1.1）按「数据库里的全部汉字 + 源码里的界面文案 +
GB2312 一级字库」做子集，得到 **919 KB** 的 `llt-bold.otf`，放在
`android/app/src/main/assets/fonts/`；启动时解到 `files/fonts/`（大小不一致才重写），
路径通过 `Platform::font_dirs` 交给 `fonts::install`，候选表里排在第一位。
许可证随包放在同一个目录的 `OFL.txt`。

> 等宽族也补了中文回退：egui 默认的 `Monospace` 只有 Hack（纯 ASCII），
> 「数据 / 路径」这类等宽文字里的汉字会变豆腐块，所以把中文（含粗体）追加到
> `Monospace` 链尾部——ASCII 仍走 Hack 等宽对齐，汉字回退到中文字体。

### 2.5 中文输入法

`NativeActivity` 的窗口里没有带 `InputConnection` 的 View，输入法只能「发按键」，
中文合成与候选整词上屏会整个丢掉。所以 `MainActivity.onCreate` 里挂一个 1×1 的透明
`ImeBridge` View 抢焦点，用自定义 `InputConnection` 收 `commitText` / `setComposingText`，
经 JNI 交给 Rust，再由界面转成 `egui::Event::Ime`。详见 `android/src/ime.rs` 与
`android/app/src/main/java/org/llt/timetable/ImeBridge.java`。

**为什么键盘会自己收起来**（已修）：egui 每次 `Memory::request_focus` 都会调
`interrupt_ime`，egui-winit 于是在帧末 `set_ime_allowed(false)` + `(true)`——在 Android 上
就是 `hide_soft_input` 收键盘，而「再弹」走 `showSoftInput(DecorView)`，因为焦点在我们的
桥 View 上被系统忽略。触发 `request_focus` 的一共有三种日常操作，三种都要补弹：

| 操作 | 为什么算 request_focus |
|---|---|
| 点一下输入框 | 点击会请求焦点 |
| 焦点换到另一个输入框（出发 → 到达、按 Enter 跳格） | 新控件请求焦点，`Some(id)` 变成 `Some(other_id)` |
| 打了一个字 | 输入事件本身会经过 egui 的焦点路径 |

界面里记的是**焦点控件 id**（不是「有没有焦点」），三者任一发生时都标记「补弹两次」，
40 ms 后重新请求一帧、再 `showSoftInput`；`ensure_shown` 只 show、**不** `restartInput`
（后者会让键盘闪一下）。

#### 用户自己收起键盘（返回键 / 键盘上的「收起」）

这条路径系统只改窗口 insets：输入法不通知 `InputConnection`，egui 更不知道。于是输入框
一直保持焦点（光标还在闪），上面那套「补弹」和下一次点击又会把键盘弹回来——实测按返回
收起后 5 秒内自己弹回。

现在 `ImeBridge` 盯着窗口 insets 的 IME 可见性（API 30+ 用 `WindowInsets.Type.ime()`，
更老的系统比较 `systemWindowInsetBottom` 与 stable inset），真收起来时回调
`nativeImeHidden()`；界面收到 `ImeInput::Hidden` 就 `Memory::stop_text_input()` 放开焦点，
并把「补弹」计数清零。点回输入框（新的焦点请求）照常弹键盘。

难点是把「用户收的」和「我们自己抖的」分开，否则切个输入框键盘就再也不回来：

| logcat 现象 | 归属 |
|---|---|
| 焦点一变，22 ms 里「显示 → 收起 → 显示 → 收起」 | winit 的 `set_ime_allowed(false)+(true)` 加 `restartInput`，自己抖的 |
| 先一条 `onRequestHide … fromUser false`，1 ms 后 insets 收起 | 我们自己 `setActive(false)`（egui 焦点丢了） |
| 只有 insets 收起，前后没有任何 `onRequestHide` | 用户按的 |

判定规则：`setActive(false)` 之前先置 `hidingByUs`，这种收起不报；`setActive(true)` /
`ensureActive()` 要**同步**（不能等 `post`，insets 回调会先到 1 毫秒）把 `churnUntilMs`
推到 600 ms 之后，窗口内的收起一律算抖动。剩下没被认领的收起才算用户按的，
`dismissed = true` 之后连 `ensureActive` 也挡住，直到下一次显式 `setActive(true)`。

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
- 开了 **Gradle 配置缓存**（`org.gradle.configuration-cache=true`）：本机实测
  `assembleRelease` 增量 1.84s → 1.45s（配置阶段 1.5s → 1.2s）。改 `build.gradle.kts`
  或 `gradle.properties` 后第一次会重建缓存（多几秒）；怀疑缓存有问题时用
  `--no-configuration-cache` 先排除。
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
| 满屏 `Configuration.setVisible(boolean) has been deprecated` | Gradle 9.9 报的是 **Kotlin Gradle Plugin** 的弃用（AGP 9 默认带内置 Kotlin，即使是纯 Java 工程也会应用 KGP），不是本仓库的脚本 | 默认 `--warning-mode=summary` 只打 3 行汇总；想看细节再加 `--warning-mode all`。要彻底静音可 `--warning-mode none`；**不要**用 `android.builtInKotlin=false` 关内置 Kotlin——那个开关本身也已被 AGP 标记弃用 |
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
- 「模糊」开关贴在输入框旁边（站站在左、车站在右），不再占底栏；站站的两个输入框
  按「窗口宽 − 模糊 − ⇌」两等分，下限 96 点（站名最长 5 个汉字，5×15 点 + 内边距）。
- 横竖屏切换：`ui()` 每次比较可用尺寸，变了就 `request_repaint()` 一次，
  不然旋转后可能还显示旧宽度排的画面，得等下一次触摸才刷新。
- **横屏布局**（触摸、高 < 520 点、宽 ≥ 640 点）：改成左右两栏——左栏是标签页一行
  ＋当前页的控件（站站把「模糊」挪到输入框**下面**一行，两个框因此各宽 130 点左右）
  ＋贴底的动作行，右栏只放结果表。窗口为 851×393 时表格拿到约 290 点高（7–8 行），
  六列（状态/车次/出发站/出发时间/到达站/到达时间）正好铺满，不再横向滚。
  平板（1024×768）不命中这套，仍是竖排 + 底栏。
- **候选站是输入框下方的下拉列表**（`candidates_popup`，车站页与站站页共用），不再是表格上方
  那排 chip。量化过：单字查询（`南`）返回 24 个候选，393 点宽的模拟器上 4 行约 151 点，
  347 点的真机上 5 行约 190 点——竖屏结果区只有 620 点左右，等于吃掉两到三成；
  桌面窗口宽，24 个候选约 1.5 行（60 点/700 点），影响小得多，但同样没必要一直占着。
  现在：点输入框或打字时展开、选完/点别处/Esc 收起，高度上限 `content × 0.35`
  （4–8 行，键盘弹起时也不会顶到键盘上沿），超出的部分自己滚。选完会顺手
  `stop_text_input()`：键盘收起、列表收起，否则新查询又会带出 1 个候选。
  列表用 `Popup::from_response` + `CloseOnClickOutside`，行是整行可点的
  `Button::selected(…)`（未选中不画框，悬停/选中才有底色）。
- **同城组那一行 chip 删掉了**（2026-11，车站页与站站页都删）：它其实只是「把城市名替我打进输入框」——
  `refresh_station` 里点 chip 就是 `station_query = city` 再查一次，而模糊模式下
  输入完整城市名本来就会 `same_city_selection` 精确命中同一个组（实测「成都」→
  成都东/成都南/成都西/犀浦 全部铺进「筛选」）。它白白占一行，还只在模糊模式下出现。
  站站页原来也有一排同城组 chip，同样删掉（见下一条）。
- **站站页的两个输入框各挂一个下拉**：出发/到达哪个没解析出来（`od_missing`）就贴在哪个
  下面，列表是 `od_suggestions`（同一个 `candidates_popup`）。点候选填进对应那一格、重查、
  列表与键盘一起收起。原来这里是解析失败才出现的**两排 chip**（同城组 + 候选站），
  现在结果区不会被它们往下压。模糊模式下想查同城全组，把城市名输全即可
  （`same_city_selection` 精确命中）。
- 站站输入行的宽度由「剩余宽度 − ⇌ − 间距 − `TextEdit` 自身内边距」现算：
  之前用固定常量，算出来刚好等于可用宽度，最后 8 点间距就把「到达」挤到了下一行。
- **底栏**（`body()` 里的 `tab_bar_bottom` + `action_bar`）：标签页四等分放最底下，
  上面一行是当前页的动作与计数。凡是「不是输入框、又占表格上方」的控件都收进来了：
  站站是 `模糊 / 查询 + 计数`，车次是 `清空 + 计数`，车站是 `清空 / 模糊 + 计数`。
  （「交换」不在这里：它是两个输入框中间的 `⇌`。）
  数据页不画动作行。
- 头部只剩一行 `标题 + 日期 + 主题`（桌面仍是标题行 + 标签行 + 日期行）。
- 筛选 chip（站站的车站筛选、车站的同城车站勾选）用 `theme::chip_height()`：触摸 30 点、
  桌面 22 点，一行能放更多个，也少占一行高。
- 站站竖屏：`出发 ⇌ 到达` 一行，日期与「模糊站名」同一行（在输入框下、筛选上）。
- 所有表格的滚动条都是**浮动细条**（`ScrollStyle::floating()`，触摸 6 点）：平时不占位、
  几乎看不见，滚动时才浮出来——粗滚动条在手机上要吃掉十几点宽度。
- 底栏按钮比普通控件再高 6 点（`theme::bar_height()`），拇指更好点。
- **站站与车站的结果表用同一套滚动结构**（`app.rs` 的 `scrollable_table`）：
  外层一个横竖都能滚的 `ScrollArea::both` 负责滚动，表格自身
  `drag_to_scroll(Never)`、列一律 `Column::exact`（外层才知道内容有多宽）。
  两个理由：
  1. `TableBuilder` 自带的滚动区**只有纵向**，列宽溢出时它把右边直接裁掉，手指也拖不出
     横向滚动——车站表在手机上「发车」那一列就是这样消失的；
  2. 「外层横向 + 内层纵向」两个 ScrollArea 也不行：内层会把手指拖动整段吃掉。

  列宽写死后宽度是算出来的（`状态 车次 车站 到达 发车` ≈ 400 点，站站六列 ≈ 428 点），
  比视口宽就横向滚，末列（车站的「始发 → 终到」、站站的「到达时间」）吃掉剩余宽度，
  所以横屏右栏、平板上不会在右边留一条空。代价是表头跟着内容一起上下滚
  （不再钉在顶部）——和站站一直以来的行为一致，换来的是「一个容器同时管两个方向」。
- **拖到边界就停，没有橡皮筋**：egui 0.37 的 `KineticScrollStyle::rubber_band` 默认开着
  （iOS 那种越过边界有阻尼、松手弹回）。表格上的表现是「往右/往下拖出一片空白、松手
  回弹」，看起来像 Android 系统的 overscroll，实际是 `ScrollArea` 自己算的——
  `theme::apply_spacing` 里把它关掉，每帧末尾偏移被夹回 `0..=max`，越过边界一格都不动。
  惯性滑行（`decay_time`）保留。
  > 注意：任何**新建**的 `ScrollStyle`（比如 `ScrollStyle::floating()`）都会把
  > `kinetic` 拉回默认值（含 `rubber_band: true`）。改滚动条粗细时要继承
  > `ui.spacing().scroll` 再改，否则橡皮筋会悄悄回来（`scrollable_table` 里踩过）。
- 站站结果在竖屏用六列表：`状态 车次 出发站 出发时间 到达站 到达时间`。固定列宽、
  允许横向溢出；滚动条改成浮动的细条（`ScrollStyle::floating()`，平时不占位、
  几乎看不见）。
- 经由入口写成 `经由 61 途经点 · 1463 km …… 查看 ▸`：入口收起时点数和里程是最有用的
  两个量，所以只留这两项，「车站 N」省掉（那要看「只看车站」勾上以后的条数）。
  经由页抬头里不再重复这一行——那里的点数/里程从表里数得出来，写上去只是把表格往下压。
  入口上下各一条细线（由入口自己画，调用方不要再补 `ui.separator()`，否则双线且留白不等）。
- 经由页（Android）：抬头只留粗体车次（`返回 D6709 ☐只看车站`），路由与「经由」二字
  在手机上省掉；第二行只剩图例（`● 停站 ● 换线 ● 非办客`）。
  线路列遇到 `A/B` 这种跨线站时按 `/` 拆成多行、每条线占一行，
  行高按段数放大（桌面仍是一行 + 省略号，悬停看全）。
  多行用一个带 `
` 的 Label 渲染，**不要**用 `ui.vertical` + 多个 Label：后者在
  表格单元格里会贴顶，上下内边距一多一少（实测 30/58 px，改成单 Label 后与
  普通行一致：上 51、下 46 px）。
- 车次详情是**整页**（列表被替换）：标题栏与「经由」窄条固定在上面，下面只有一个滚动块
  （时刻表 → 杂项）。经由不跟着滚——它是一条入口+摘要，翻长停站表时滑走就得先滚回顶部
  才找得回来。经由页本身仍是整页模态。
- 「设置」页是第四个标签页（原「数据」页）：上面是查询开关（深浅色 / 模糊查询），
  下面是「数据」子章节；整页一个滚动容器，小字 14 点。
- 设置页两行开关共用同一套排版：左边标签列固定宽（`theme::SETTING_LABEL_WIDTH`），
  右边是控件，两个行才对得齐。深浅色三个选项用 `Button` + `frame_when_inactive(true)`：
  `Button::selectable` 只在选中时画边框，未选中的三个字看起来是正文，不像能点。
  模糊开关合成一行 `模糊查询 ☑站站 ☐车站`（车次页没有模糊）。
- 「当前数据」的五条信息统一成「标签 + 等宽值」（`data.rs` 的 `info_row`）：标签列等宽、
  行高一致，路径在值列里换行。原来「数据 / 构建于」是等宽白字连排、「大小 / 位置」是
  灰色小字，读起来像五段互不相干的东西。
- 深浅色（跟随系统 / 浅色 / 深色）存在配置的 `ui.theme` 里，头部按钮与设置页共用同一入口。
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
| 打字后键盘 | 车次页打一个字母、站站按 Enter 跳到到达框，键盘都保持弹出 |
| 键盘弹出时的布局 | 键盘浮在界面之上，底栏与列表**原位不动**（`ADJUST_NOTHING` + 只算 systemBars） |
| 返回手势 / 返回键 | 经由页 → 车次详情 → 退出应用，逐层后退；左右边缘滑动与返回键等价（真机实测） |
| 横竖屏 | 旋转后自动重绘一次（比较可用尺寸），宽屏布局立刻生效 |
| 输入法 | 中文合成、`G12` 输入、回车触发查询都正常 |
| 同城组（删除后） | 模糊 + 输入「成都」：不出现任何 chip 行，筛选里直接给出 成都东/成都南/成都西/犀浦，结果表满高；非模糊 + 「南京」仍只查单站 |
| 站站候选站（下拉） | 出发「北京朝」/到达「南」：不出现任何 chip 行；点哪个输入框就在它下面弹候选（8 个、可滚），点「北京朝阳」填进出发、重查、列表与键盘收起；同一套代码也验证了到达侧 |
| 车站候选站（下拉） | 查询「南」时 24 个候选：点输入框在下方展开（最多 8 行、超出滚动、不压结果表），点「南京」后列表收起、键盘收起、结果表满高；点别处收起；结果为空时提示语改为「点输入框从候选站里挑一个」。桌面 1400×830 上同一套列表贴在输入框下方、宽度跟随输入框 |
| 收起键盘（真机 + 模拟器） | 打字时按返回 / 键盘上的「收起」：键盘不再自己弹回（等 5 秒仍收着），输入框放开焦点（不再有聚焦边框），文字保留；再点输入框键盘照常弹出。切输入框（出发 → 到达）键盘不丢 |
| 真机（vivo / Android 17 / arm64） | 覆盖安装保留原库；查询、详情、经由、底栏、输入法均正常；检查更新 20261108 → 20261114；状态栏 140 px 让开正确 |
| 工具链升级后（JDK 25 / Gradle 9.9 / AGP 9.5.1 / compileSdk 36） | 真机与模拟器都重新安装验证通过；targetSdk 36 的强制 edge-to-edge 下缩进仍然正确；明文 HTTP 的版本接口照常可用 |
| 底栏 | 标签页 + 动作行落在底部；车次页从「输入 + 计数 + 表格」变成「输入 + 表格」，多出约 2 行 |
| 紧凑头部 | 一行头部（标题 + 日期 + 主题），比原来三行多出约 2 行列表空间 |
| 开行计划 | 整页详情尾部（餐车后）内嵌月历：G531（`t.rule=(7,15)`）11-01/02 开行、11-03~05 停开、11-06 起再开行，与 `tools/runchart.py` 逐日一致 |
| 站站结果表滚动（真机 + 模拟器） | 竖屏六列 428 点 > 视口，横着拖能一路拖到「到达时间」；竖着拖能翻到底；到边界后往同方向再拖，表格**一格不动**、不出现空白、松手也不弹 |
| 车站结果表滚动（真机 + 模拟器） | 改前：宽度溢出时直接裁掉「发车」列、且完全不能横向滚（旧版实测截图）。改后：竖屏可横拖到「发车」+`+N天`，竖拖翻 1196 行；两个方向到边界都硬停（`ScrollArea` 的 `content=400 / inner=360.7 → max_offset=39.3`，拖到底后偏移就是 39.3）|
| 横屏 | 站站六列（≈428）在右栏里放得下，末列吃掉剩余宽度，不用横拖；车站五列同样铺满；旋转后旧偏移被夹回合法范围 |
| 经由弹窗宽度（真机） | 横屏开经由（730 宽）→ 转回竖屏（自动关闭）→ 再开：宽度是竖屏的 323，不再沿用横屏那份。原因是 `Area` 按 id 记住尺寸，id 里现在带了屏幕尺寸 |
| 开行计划翻月 | `‹ 2026 年 11 月 ›` 两个按钮 37×37、箭头 19 点加粗；区间文字只在标题下面写一次（日历抬头里不再重复） |
| 设置页 | 深浅色三个选项都是按钮形状（选中带底色）；`模糊查询 ☑站站 ☐车站`；“当前数据”五行为同一格式、行高一致，路径换行 |
| APK 体积 | 13.2 MB（不含数据库）|

## 8 已知限制与后续

1. **只验过一台真机**（vivo / Android 17）。其它 ROM 的字体、刘海、手势导航
   `getRootWindowInsets()` 返回值，以及明文 HTTP 是否被运营商劫持，仍需实测。
2. **返回是「收到后台」而不是退出进程**：`activity.finish()` 会销毁 Activity 但进程还活着，
   而 winit 的事件循环一个进程只能建一次，再从桌面进来接不上新窗口，界面就是黑的（实测）。
   现在调 `moveTaskToBack(true)`：看起来退出、事件循环保留；系统真把进程杀了，下次启动
   会走完整的 `onCreate` → `android_main`。
3. **横屏左栏会滚动**：控件多的页面（站站的「出发/到达」筛选）在 393 点高的横屏里
   放不进一屏，需要滑一下左栏；没有再把筛选压成一行（会挤掉输入框的宽度）。
4. **剪贴板**：Android 侧没有开 eframe 的 `clipboard`（arboard 无 Android 后端），
   输入框里不能长按粘贴。
5. **键盘盖住底栏**：为了让布局不跳动，键盘是「浮层」而不是挤压布局，打字时看不到
   底栏（三个查询页都边打边查，点结果区收键盘即可）。
6. **结果表的表头会跟着滚**：改用「外层 `ScrollArea::both` + 固定列宽」的副作用——
   表头成了内容的一部分，竖向拖动时一起滑走。想要钉住表头，就只能在「横向可拖」和
   「纵向自带滚动区（带固定表头）」之间二选一；后者横向只能拖滚动条，手机上基本不可用。
7. **粗体是「假」粗体**（见 §2.4）。要真粗体得往 assets 塞一份思源黑体 Bold 子集。
8. **备份校验只到结构层**：查 `application_id`、结构版本、必需表；不做内容哈希。
9. **下载失败只能整份重来**：没有断点续传；镜像只有明文 HTTP，被劫持/不可达时
   只能靠「导入备份」兜底。
10. **没有应用图标**，也没有 32 位 ABI（`armeabi-v7a` / `x86`）。
11. **数据页的日志只有 3 行**：够看「下载 / 解包 / 解码 / 写入」，不做滚动日志。
