//! Android 入口（`cdylib`）。
//!
//! 只做平台胶水：算好目录、把系统输入法与文件选择器接上，然后把界面交给 `llt_gui`。
//! 界面、下载解码、备份逻辑都在共用库里，这里不出现任何 UI 代码。
//!
//! **APK 不内置数据库**（数据包 41 MB，随包发会让 APK 翻倍，而且版本很快过时）：
//! 第一次启动时库不存在，界面会落在「数据」页引导用户下载或导入备份。

mod files;
mod ime;

use std::path::PathBuf;

use android_activity::AndroidApp;
use jni::objects::JObject;
use jni::sys::jint;

/// 软键盘弹出时，布局要不要跟着收缩。
///
/// `false`（当前）：键盘只是**浮在上面**，底栏和界面其它元素都不动——打字时位置稳定，
/// 代价是键盘会盖住底栏，要按 查询/切页 得先收键盘。
/// `true`：把键盘高度算进底部缩进，整个界面收缩到键盘上方（键盘一弹一收，列表位置会跳）。
const SHRINK_FOR_IME: bool = false;

/// 私有目录里的文件名。
const DB_FILE: &str = "llt-timetable.sqlite";
const CONFIG_FILE: &str = "llt-gui.json";

/// android-activity 规定的入口名，由 `NativeActivity` 在新线程里调用。
///
/// `eprintln!` 已经被 android-activity 重定向到 logcat，出问题用
/// `adb logcat -s RustStdoutStderr` 就能看到。
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let files_dir = app.internal_data_path().unwrap_or_else(|| {
        eprintln!("取不到私有目录，数据库与配置将落在当前目录");
        PathBuf::from(".")
    });
    let db_path = files_dir.join(DB_FILE);
    let config_path = files_dir.join(CONFIG_FILE);

    // 目录约定：
    // - 库和配置在私有目录（filesDir），不需要任何权限；
    // - 备份的导入/导出走系统文件选择器，Java 侧临时拷到 filesDir/import；
    // - 临时目录用 cacheDir（Android 没有可写的 /tmp）。
    let cache_dir = cache_dir(&app).unwrap_or_else(|| files_dir.join("cache"));
    let import_dir = files_dir.join("import");
    let _ = std::fs::create_dir_all(&import_dir);

    let options = eframe::NativeOptions {
        android_app: Some(app.clone()),
        ..llt_gui::options()
    };

    // 系统栏缩进用回调：窗口刚创建时 `getRootWindowInsets()` 还是 null，
    // 软键盘弹出 / 收起也会改变它（见 `llt_gui::Insets`）。
    let last = std::sync::Mutex::new(llt_gui::ScreenInsets::default());
    let probe_app = app.clone();
    let insets = llt_gui::Insets::probe(move || {
        let found = system_bar_insets(&probe_app);
        if let Ok(mut last) = last.lock()
            && *last != found
        {
            *last = found;
            eprintln!("系统栏缩进：{found:?}");
        }
        found
    });

    // 输入法桥：NativeActivity 没有 InputConnection，中文要自己接（见 `ime.rs`）。
    let ime = ime::attach(&app);
    // 文件选择器：导入/导出备份要弹系统的 SAF（见 `files.rs` 与 MainActivity）。
    let picker = files::attach(&app);

    let platform = llt_gui::Platform {
        insets,
        ime: ime.map(|bridge| bridge as std::sync::Arc<dyn llt_gui::ImeBridge>),
        // 触屏：控件与行高用大一号的一套，点击热区也放宽。
        pointer: llt_gui::theme::Pointer::Touch,
        dirs: llt_gui::data::DataDirs {
            import: import_dir.clone(),
            export: import_dir,
            temp: cache_dir,
        },
        files: picker.map(|picker| picker as std::sync::Arc<dyn llt_gui::files::FilePicker>),
    };

    if let Err(err) = llt_gui::run_with(options, db_path, config_path, platform) {
        eprintln!("界面启动失败：{err}");
    }
}

/// 应用 cache 目录（与 files 同级）。
///
/// 直接问 Java 的 `getCacheDir()`：android-activity 只给了 `internal_data_path()`。
fn cache_dir(app: &AndroidApp) -> Option<PathBuf> {
    let text = with_activity_string(app, "getCacheDir").ok()??;
    // `getCacheDir()` 返回 File，先取路径字符串再包成 PathBuf。
    Some(PathBuf::from(text))
}

/// 调 Activity 上返回 `File` 的无参方法，拿到绝对路径。
fn with_activity_string(app: &AndroidApp, method: &str) -> jni::errors::Result<Option<String>> {
    // SAFETY: `android_main` 期间 Activity 指针由 android-activity 保证有效。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let file = env
        .call_method(&activity, method, "()Ljava/io/File;", &[])?
        .l()?;
    if file.is_null() {
        return Ok(None);
    }
    let path = env
        .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    if path.is_null() {
        return Ok(None);
    }
    let text: String = env.get_string(&jni::objects::JString::from(path))?.into();
    Ok(Some(text))
}

/// 读取状态栏 / 导航栏 / 软键盘占的像素宽度。
///
/// 窗口是 edge-to-edge 的（内容画到状态栏下面），而 egui-winit 只在 iOS 上填
/// `safe_area_insets`，所以 Android 这边自己问一下 Java 侧。
/// 低版本用已废弃的 `getSystemWindowInset*`（API 20 起就有）；
/// 软键盘的高度不在那个「系统栏」里，API 30+ 要另外问 `WindowInsets.Type.ime()`。
/// 取不到时返回 0，界面就当没有系统栏（顶栏会压到状态栏下，不致命）。
///
/// 软键盘高度只在 [`SHRINK_FOR_IME`] 为真时才计入底部：默认让键盘浮在界面上，
/// 底栏与列表保持原位不动。
fn system_bar_insets(app: &AndroidApp) -> llt_gui::ScreenInsets {
    read_insets(app).unwrap_or_default()
}

fn read_insets(app: &AndroidApp) -> Option<llt_gui::ScreenInsets> {
    // SAFETY: android-activity 保证这两个指针在 android_main 期间有效。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    let window = env
        .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])
        .ok()?
        .l()
        .ok()?;
    let decor = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])
        .ok()?
        .l()
        .ok()?;
    let window_insets = env
        .call_method(
            &decor,
            "getRootWindowInsets",
            "()Landroid/view/WindowInsets;",
            &[],
        )
        .ok()?
        .l()
        .ok()?;
    if window_insets.is_null() {
        return None;
    }

    let system_inset = |env: &mut jni::JNIEnv<'_>, name: &str| -> u32 {
        env.call_method(&window_insets, name, "()I", &[])
            .ok()
            .and_then(|value| value.i().ok())
            .unwrap_or(0)
            .max(0) as u32
    };
    // 废弃的 `getSystemWindowInset*` 在 adjustResize 下会把键盘高度算进 bottom，
    // 所以底部优先用 `WindowInsets.Type.systemBars()`（API 30+，只含导航栏/状态栏）。
    let bars = system_bar_insets_api30(&mut env, &window_insets);
    let mut top = system_inset(&mut env, "getSystemWindowInsetTop");
    let mut left = system_inset(&mut env, "getSystemWindowInsetLeft");
    let mut right = system_inset(&mut env, "getSystemWindowInsetRight");
    let mut bottom = system_inset(&mut env, "getSystemWindowInsetBottom");
    if let Some(bars) = bars {
        top = top.max(bars[0]);
        left = left.max(bars[1]);
        right = right.max(bars[2]);
        // 底部只认系统栏：键盘默认不算进布局（见 SHRINK_FOR_IME）。
        bottom = bars[3];
    }
    if SHRINK_FOR_IME {
        bottom = bottom.max(ime_inset(&mut env, &window_insets));
    }

    Some(llt_gui::ScreenInsets {
        top,
        bottom,
        left,
        right,
    })
}

/// 软键盘的高度（API 30+ 才有 `WindowInsets.Type.ime()`，低版本返回 0）。
/// `WindowInsets.getInsets(WindowInsets.Type.systemBars())`，API 30+ 才有。
/// 返回 `[top, left, right, bottom]`（像素）；老系统上返回 `None`。
fn system_bar_insets_api30(env: &mut jni::JNIEnv<'_>, window_insets: &JObject<'_>) -> Option<[u32; 4]> {
    let read = (|| -> jni::errors::Result<[jint; 4]> {
        let types = env.find_class("android/view/WindowInsets$Type")?;
        let bars = env.call_static_method(types, "systemBars", "()I", &[])?.i()?;
        let insets = env
            .call_method(
                window_insets,
                "getInsets",
                "(I)Landroid/graphics/Insets;",
                &[jni::objects::JValue::Int(bars)],
            )?
            .l()?;
        if insets.is_null() {
            return Ok([0; 4]);
        }
        let field = |env: &mut jni::JNIEnv<'_>, name: &str| -> jni::errors::Result<jint> {
            env.get_field(&insets, name, "I")?.i()
        };
        Ok([
            field(env, "top")?,
            field(env, "left")?,
            field(env, "right")?,
            field(env, "bottom")?,
        ])
    })();
    match read {
        Ok(values) => Some(values.map(|value| value.max(0) as u32)),
        Err(_) => None,
    }
}

fn ime_inset(env: &mut jni::JNIEnv<'_>, window_insets: &jni::objects::JObject<'_>) -> u32 {
    let mut read = || -> jni::errors::Result<jint> {
        let types = env.find_class("android/view/WindowInsets$Type")?;
        let ime = env.call_static_method(types, "ime", "()I", &[])?.i()?;
        let insets = env
            .call_method(
                window_insets,
                "getInsets",
                "(I)Landroid/graphics/Insets;",
                &[jni::objects::JValue::Int(ime)],
            )?
            .l()?;
        if insets.is_null() {
            return Ok(0);
        }
        env.get_field(&insets, "bottom", "I")?.i()
    };
    read().unwrap_or(0).max(0) as u32
}
