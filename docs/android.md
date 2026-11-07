# llt-gui 的 Android 方案

把 `gui/` 里那套 egui 界面装进 APK，在 Android 上跑起来。本文记方案、环境、
构建方式和已知限制；界面本身的交互规则仍以 [`../gui/README.md`](../gui/README.md) 为准。

> **实现状态**：已验证可运行。Android 13（API 33，x86_64）模拟器上确认了启动、
> 解包数据库、中文显示、触屏切页 / 输入 / 查询 / 打开详情 / 经由整页、系统栏与
> 软键盘避让、横竖屏切换。真机和 arm64 只做了编译，没有实机验证。

---

## 1 总体结构

```
gui/            桌面 + 共用库（lib `llt_gui`：界面、配置、字体、主题、启动函数）
android/        独立的 cargo 包（cdylib `llt_gui_android`）：Android 入口 + 打包配置
```

`android/` 有自己的 `[workspace]`，并且被根 `Cargo.toml` 的 `exclude` 排除：

- 桌面构建不受影响，`cargo build --workspace --release` 仍然只产出 `llt.exe` /
  `llt-gui.exe`（不会多一个用不上的 `.dll`）；
- Android 包只在交叉编译时构建，`cargo test/clippy --workspace` 不会去碰 NDK。

```
android/
  Cargo.toml        cdylib + [package.metadata.android]（cargo apk 读它）
  src/lib.rs        android_main：解包数据库 → 调 llt_gui::run_with
  res/values/strings.xml   应用名（中文）
  assets/           由 build.ps1 生成：timetable/llt-timetable.sqlite + version
  build.ps1         一键打包
  keystore/         build.ps1 -Release 首次运行生成的自用签名证书
```

### 桌面与 Android 的接口

`gui/src/lib.rs` 只暴露三样东西给 Android：

| 项 | 作用 |
|---|---|
| `options() -> eframe::NativeOptions` | 窗口/渲染配置；Android 端补上 `android_app` 即可 |
| `run_with(options, db, config, insets)` | 启动界面（数据库与配置路径由调用方决定） |
| `Insets` / `ScreenInsets` | 系统栏缩进：桌面是固定 0，Android 是回调 |

界面代码不 `cfg(target_os)` 分支去认平台，只有字体目录和"系统栏缩进要不要问"这两处不一样。

---

## 2 运行时行为

### 2.1 数据库

APK 里带一份 `assets/timetable/llt-timetable.sqlite`（约 41 MB，压缩后 APK 里约 13 MB）。
首次启动时解到应用私有目录：

```
/data/user/0/org.llt.timetable/files/llt-timetable.sqlite
/data/user/0/org.llt.timetable/files/llt-timetable.version
/data/user/0/org.llt.timetable/files/llt-gui.json          ← 界面配置
```

- 版本标记 `assets/timetable/version` 来自 `out/timetable/meta.json` 的 `data_version`；
  装的应用版本换了（APK 自带的数据版本变了）就重新解一遍，没变就直接复用。
- 先写 `*.sqlite.tmp` 再改名：中途被杀掉也不会留下半个库。
- 私有目录不需要任何权限，也不用动态申请存储权限。

### 2.2 配置

`llt-gui.json` 落在同一目录。桌面是"绿色版：配置跟 exe 走"，Android 没有这个语义，
所以 `config::resolve` 的隐式查找在 Android 上不参与——路径由 `android/src/lib.rs` 直接给出。
字段、去抖写盘、坏文件回退的行为与桌面完全一致（见 [`gui-config.md`](gui-config.md)）。

### 2.3 字体

Android 没有 fontconfig，`/usr/share/fonts` 也不存在；系统字体都在 `/system/fonts`：
中文是 `NotoSansCJK-Regular.ttc`（集合里选 `NotoSansCJKsc-Regular`）。`gui/src/fonts.rs`
的候选目录里补了 `/system/fonts` 与 `/product/fonts`，其余逻辑不变。

真正的粗体（标题、车次号、表头）在 Android 上大概率找不到独立的粗体字形文件：
候选链会退化成普通字重，不会出豆腐块。要让粗体在 Android 上也是真粗体，需要往 assets
里塞一份思源黑体 Bold 子集（约 1–2 MB），见 §6。

### 2.4 系统栏与软键盘

Android 的窗口是 edge-to-edge 的：内容会画到状态栏、导航栏、刘海底下。egui/eframe
只在 iOS 上填 `safe_area_insets`，Android 这边自己问 Java 侧：

```rust
activity.getWindow().getDecorView().getRootWindowInsets()
```

- 上下左右四个方向都取（横屏时导航条会跑到侧边）；
- 软键盘高度单独取 `WindowInsets.Type.ime()`（API 30+），与系统栏取最大值；
- 结果以**物理像素**交给界面，界面每帧除以 `pixels_per_point` 换算成逻辑点；
- 界面把头 `Insets` 当作回调：**最多每 150 ms 问一次**。键盘弹出、导航栏隐藏、
  旋转屏幕都不是应用能直接收到的事件，而帧本来就跟着事件走，这个频率既够跟手也不逐帧过 JNI；
- 拿到缩进后，根 `Ui` 的矩形缩进安全区，面板、表格、详情、模态窗都跟着排。
  顶栏、底部详情、经由整页都不会再压到系统栏下面。

`egui::Margin` 是 `i8`，所以换算后按 127 点封顶（远超任何系统栏高度）。

### 2.5 触摸尺寸

`Platform::pointer` 决定尺寸，数值全在 `theme.rs`：

| | 鼠标（桌面） | 触摸（Android） |
|---|---|---|
| 控件高度（输入框 / 按钮 / 标签页） | 30 | 44 |
| 列表行高 | 28 | 42 |
| 详情行高（停站表 / 经由表） | 26 | 38 |
| 表头高度 | 30 / 26 | 38 / 34 |
| 可点文字热区外扩 | 2 × 4 | 6 × 8 |
| 滚动条宽度 | 10 | 14 |

行距（`item_spacing.y`）从 6 放到 8，滚动条拉宽到 14 点才拖得住。字号不变：
Android 的逻辑宽度只有约 440 点，字号放大后表格列会先撑破。高度变了以后，
所有“按行数算高度”的地方都走 `theme::detail_row_height() + item_spacing.y` 的
pitch，行贴合（不留半行）仍然成立。

### 2.6 系统输入法

winit 0.30 的 Android 后端没有任何 IME 支持：既不会把输入法的合成串变成
`WindowEvent::Ime`，`set_ime_allowed` 也只是对 DecorView 调一次 `showSoftInput`。
而 `NativeActivity` 的窗口里**没有带 `InputConnection` 的 View**，`dumpsys input_method`
里能看到：

```
mServedView=DecorView{...}[NativeActivity]
mServedInputConnection=null
inputType=0x0
```

输入法拿到这个结论就退回「发按键」那条路——按键一次只带一个字符，ASCII 能糊过去，
中文却要经拼音合成、候选、最后整词 `commitText("北京")` 上屏，于是**完全打不进来**。
`adb shell input text G1` 能用、换成中文输入法就没反应，就是这个原因。

自己在 JNI 里补一层，链路是：

```
输入法 → java/…/ImeBridge.java 的 InputConnection → native 方法
      → android/src/ime.rs 的事件队列 → llt_gui 每帧 drain() → egui::Event
```

几个卡过的点，改之前先看这里：

| 问题 | 处理 |
|---|---|
| 谁接输入法 | `ImeBridge` 是个 1×1 的 View，`addView` 到 DecorView 后 `requestFocus()`，做成「可见、可获焦」的合法输入目标；1×1 不挡 NativeActivity 的 SurfaceView，触摸照旧走 winit |
| `inputType` | 在 `onCreateInputConnection` 里填 `TYPE_CLASS_TEXT`。留 `TYPE_NULL` 时输入法会认为「对面只认按键」，中文输入直接被禁掉 |
| 谁弹键盘 | 焦点已经在这个 View 上，winit 那次「拿 DecorView 去 `showSoftInput`」会被系统忽略（`mInputShown=false`）。所以 `setActive(true/false)` 自己弹 / 收键盘，由界面的「有没有输入框拿到焦点」驱动 |
| 类怎么加载 | 清单里是 `android:hasCode="false"`（cargo-apk 的 native 应用就是这样），应用自己的 ClassLoader 是空的（`DexPathList[[]]`），`loadClass` 一律 `ClassNotFoundException`；塞 `classes.dex` 进 APK 也没用。改成把 dex 放 `assets/ime.dex`，运行时用 `InMemoryDexClassLoader`（API 26+，正好等于 minSdk）装进来 |
| 线程 | View 树只能在 UI 线程动，而 `android_main` 在渲染线程：`install` / `setActive` 内部都 post 到 UI 线程 |
| 上屏后不刷新 | 输入法在系统线程里提交，egui 不会自己醒；界面空闲时中文字要等到下一次别的事件（光标闪烁，约 1 秒）才显示。`push()` 里用交给平台侧的 `egui::Context` 调 `request_repaint()` |
| 实体键盘 / `adb input` | 焦点在我们的 View 上，按键不再经过 NativeActivity。`onKeyDown` 里把退格、回车、可打印字符转给 Rust，`adb shell input text` 与实体键盘因此可用 |

`build.ps1` 的第 3 步用 `javac` + `d8` 把 `android/java/` 编成 `assets/ime.dex`，
和数据库一起打进 APK；Java 只是一层薄胶水，界面与状态全在 Rust 侧。

### 2.7 渲染

`eframe` 用 wgpu。`options()` 把 `device_descriptor` 改成用**适配器自己上报的上限**
（`adapter.limits()`），而不是 wgpu 的默认上限：

> 模拟器的 SwiftShader Vulkan 只给 `max_uniform_buffer_binding_size = 16 KiB`，
> 而默认值要 64 KiB，照抄默认值会直接开不了设备：
> `Limit 'max_uniform_buffer_binding_size' value 65536 is better than allowed 16384`。

桌面显卡的上限本来就不低于默认值，所以这样改不改变桌面行为。

---

## 3 环境准备（Windows）

本机实际装好的版本：

| 组件 | 版本 / 位置 |
|---|---|
| Rust | 1.99.1，`rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android` |
| Android SDK | `%LOCALAPPDATA%\Android\Sdk`（platform 34、build-tools 34.0.0、platform-tools、emulator） |
| Android NDK | `%LOCALAPPDATA%\Android\Sdk\ndk\27.3.13750724`（r27d） |
| JDK | Android Studio 自带的 `...\Android Studio\jbr`（21.0.5），`build.ps1` 会自动把它给 apksigner |
| cargo-apk | `cargo install cargo-apk`（0.10.0，已废弃但够用，见 §6） |
| cmdline-tools | `%LOCALAPPDATA%\Android\Sdk\cmdline-tools\latest`（`avdmanager` / `sdkmanager`） |
| 模拟器镜像 | `system-images;android-33;default;x86_64` |

除了 SDK 自带的，另外装的四样：

```powershell
# 1) Rust 的 Android target
rustup target add aarch64-linux-android x86_64-linux-android

# 2) NDK（r27d）：解压到 SDK 的 ndk/ 下，版本号目录名要与 source.properties 一致
#    https://dl.google.com/android/repository/android-ndk-r27d-windows.zip
#    -> %LOCALAPPDATA%\Android\Sdk\ndk\27.3.13750724

# 3) cmdline-tools（Android Studio 的 SDK Manager 也能装；这里是为了命令行建 AVD）
#    https://dl.google.com/android/repository/commandlinetools-win-13114758_latest.zip
#    -> %LOCALAPPDATA%\Android\Sdk\cmdline-tools\latest

# 4) 打包工具
cargo install cargo-apk
```

`build.ps1` 需要的环境变量（不设就用默认值）：

| 变量 | 默认 | 说明 |
|---|---|---|
| `ANDROID_HOME` | `%LOCALAPPDATA%\Android\Sdk` | |
| `ANDROID_NDK_ROOT` | `$ANDROID_HOME\ndk` 下最新一版 | |
| `JAVA_HOME` | `%ProgramFiles%\Android\Android Studio\jbr` | apksigner 需要 |

---

## 4 构建

```powershell
pwsh -File android/build.ps1 -Release     # 打 release 包
pwsh -File android/build.ps1             # 打 debug 包（编译快，安装包大，可 run-as）
```

脚本做四件事：

1. 检查 `out/timetable/llt-timetable.sqlite`（没有就提示先 `llt build`）；
2. 把数据库和版本标记复制到 `android/assets/timetable/`（`-SkipAssets` 可跳过）；
3. 用 `javac` + `d8` 把 `android/java/` 编成 `assets/ime.dex`（系统输入法桥，见 §2.5）；
4. `-Release` 时若没有签名证书，用 keytool 生成 `android/keystore/llt-release.jks`，
   然后 `cargo apk build`，产物在 `android/target/<profile>/apk/llt-gui.apk`。

构建目标 ABI 写在 `android/Cargo.toml` 的 `build_targets`：`arm64-v8a` + `x86_64`
（真机 + 模拟器）。要塞进老机器再加 `armv7-linux-androideabi`。

### 装到设备 / 模拟器

```powershell
adb install -r android/target/release/apk/llt-gui.apk
adb shell am start -n org.llt.timetable/android.app.NativeActivity
adb logcat -s RustStdoutStderr        # Rust 侧的 eprintln!（解包进度、启动错误）
```

模拟器（本机创建命令，实际验证用的就是它）：

```powershell
avdmanager create avd -n llt -k "system-images;android-33;default;x86_64" -d pixel_5
emulator -avd llt -no-window -gpu swiftshader_indirect -no-audio -no-snapshot
```

两个都在 `$ANDROID_HOME`（`emulator\`、`cmdline-tools\latest\bin\`）下；
没有窗口也能截屏：`adb shell screencap -p /sdcard/s.png` + `adb pull`。

---

## 5 验证记录

| 项 | 结果 |
|---|---|
| 启动、中文显示 | 正常（`/system/fonts` 的 Noto Sans CJK） |
| 解包数据库 | `已从 APK 解出数据库 20261108`，第二次启动直接复用 |
| 触屏切页 / 输入 / 查询 | 正常；点输入框弹出系统输入法，`input text` 能查 `G1`（694 条匹配） |
| 触摸尺寸 | 控件 44 / 列表行 42 / 详情行 38 / 滚动条 14：按钮、标签页、结果行一眼看得出是给手指的；窄屏仍只显示必要的列 |
| 中文输入法 | 真机（vivo PD2509，系统输入法 `com.vivo.ai.ime.nex`）：拼音合成串能实时进输入框，提交候选词后中文字立即上屏；`dumpsys input_method` 里 `mServedView=org.llt.timetable.ImeBridge`、`mServedInputConnection=…BridgeConnection`、`inputType=0x1` |
| 详情 / 经由整页 | 正常；窄屏是底部面板，经由是整页模态，返回按钮不再压在状态栏下 |
| 系统栏避让 | 状态栏 136 px、导航栏 132 px 时缩进正确，标题栏和「浅色」按钮完整可见 |
| 软键盘避让 | 键盘弹出时 bottom 缩进从 132 px 变成 912 px，结果表在键盘上方截止 |
| 横竖屏 | 不重建 Activity；横屏自动进入宽屏布局（多出「数据」「开行日期」列），缩进跟着重算 |
| 桌面 | `cargo build --workspace --release`、测试、clippy 全部通过；桌面窗口正常启动 |

---

## 6 已知限制与后续工作

按优先级：

1. **返回键没有接线**。目前按返回键不会收起详情 / 经由页（winit 把 KEYCODE_BACK 当普通键，
   需要确认它是不是映射成 Esc）。合理行为：先关经由页，再收详情，最后才交给系统退出。
2. **剪贴板**。Android 侧没有开 eframe 的 `clipboard`（arboard 没 Android 后端），
   输入框里不能长按复制/粘贴。要用得走 JNI 自己接 `ClipboardManager`；
   输入法桥已经收到 `commitText`，接剪贴板时顺手把 `ClipboardManager.getPrimaryClip()`
   也转成 `ImeInput::Commit` 即可。
3. **输入法光标位置**。winit 的 `set_ime_cursor_area` 在 Android 上是空实现，
   键盘始终停在屏幕底部，不会跟着输入框走；要做得自己给 `ImeBridge` 设
   `setImeOptions` / 手动移动窗口。
4. **粗体是"假"粗体**（见 §2.3）。要么塞子集字体，要么接受。
5. **签名与分发**。`android/keystore/llt-release.jks` 是脚本生成的自用证书，
   密码写在 `android/Cargo.toml` 里。要分发给别人就换自己的 keystore 与密码，
   并且考虑 `apk_name`、版本号（现在是 0.1.0）与商店要求的 targetSdk。
6. **cargo-apk 已废弃**（作者推荐 `xbuild`）。现在够用；等它真不工作了再换，
   换的时候 `[package.metadata.android]` 这些配置基本能照搬。
7. **模拟器与真机都只验证过一台**。其它 ROM（字体、刘海、手势导航、各家输入法）需要再跑：
   重点是 `/system/fonts` 里有没有 `NotoSansCJK-Regular.ttc`，以及 `getRootWindowInsets()`
   在各家的返回值。
8. **后台保存**。Android 可能在没有任何回调的情况下杀进程；现在的配置是"停止输入 0.5 秒"
   就落盘，够用，但没有 `onPause` 兜底（winit 会把 `MainEvent::Pause` 变成 Suspended）。
9. **没有应用图标**。APK 用系统默认图标；加图标要往 `android/res/mipmap-*` 放 png
   并在 metadata 里写 `application.icon`。
10. **i686 / armv7**。老机器和 32 位模拟器没编（`build_targets` 里没有）。
