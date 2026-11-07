//! Android 入口（`cdylib`）。
//!
//! 只做两件事：
//!
//! 1. 把 APK 资源里的数据库（`assets/timetable/llt-timetable.sqlite`）解到应用私有目录
//!    `/data/user/0/org.llt.timetable/files/`，下次启动比对版本标记，没变就直接复用；
//! 2. 把数据库与配置文件的路径交给 `llt_gui` 启动界面。
//!
//! 界面、字体、查询逻辑全在 `llt-gui` 库里；这里不出现任何 UI 代码。
//! 桌面端没有这个包（`android-main` 只由 Android 加载）。

mod ime;

use std::ffi::CString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use android_activity::AndroidApp;
use android_activity::ndk::asset::{Asset, AssetManager};
use jni::sys::jint;

/// APK 里数据库与版本标记的位置（相对 `assets/` 根）。
const DB_ASSET: &str = "timetable/llt-timetable.sqlite";
const VERSION_ASSET: &str = "timetable/version";
/// 输入法桥的 dex（`build.ps1` 编译 `java/` 得到，打进 assets）。
const IME_DEX_ASSET: &str = "ime.dex";

/// 解到应用私有目录后的文件名。
const DB_FILE: &str = "llt-timetable.sqlite";
const VERSION_FILE: &str = "llt-timetable.version";
const CONFIG_FILE: &str = "llt-gui.json";

/// android-activity 规定的入口名，由 `NativeActivity` 在新线程里调用。
///
/// `eprintln!` 已经被 android-activity 重定向到 logcat，出问题用
/// `adb logcat | Select-String llt` 就能看到。
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // 私有目录由系统创建，取不到就退回当前目录（只影响数据库/配置的落点）。
    let files = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("."));
    let db_path = files.join(DB_FILE);
    if let Err(err) = install_db(&app, &files, &db_path) {
        // 不中止启动：界面会显示「找不到数据库」的错误页，至少能看清是哪一步坏了。
        eprintln!("准备数据库失败：{err}");
    }

    let options = eframe::NativeOptions {
        android_app: Some(app.clone()),
        ..llt_gui::options()
    };
    // 缩进用回调：窗口刚创建时 `getRootWindowInsets()` 还是 null，软键盘的
    // 弹出 / 收起也会改变它，所以界面会反复问（见 `llt_gui::Insets`）。
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
    // dex 读不出来不中止启动：界面照常起来，只是打不了中文。
    let ime_dex = read_asset(&app.asset_manager(), IME_DEX_ASSET).ok();
    let ime = ime::attach(&app, ime_dex.as_deref());
    if let Err(err) = llt_gui::run_with(
        options,
        db_path,
        files.join(CONFIG_FILE),
        insets,
        ime.map(|bridge| bridge as std::sync::Arc<dyn llt_gui::ImeBridge>),
    ) {
        eprintln!("界面启动失败：{err}");
    }
}

/// 读取状态栏 / 导航栏 / 软键盘占的像素宽度。
///
/// 窗口是 edge-to-edge 的（内容画到状态栏下面），而 egui-winit 只在 iOS 上填
/// `safe_area_insets`，所以 Android 这边自己问一下 Java 侧。
/// 低版本用已废弃的 `getSystemWindowInset*`（API 20 起就有）；
/// 软键盘的高度不在那个「系统栏」里，API 30+ 要另外问 `WindowInsets.Type.ime()`。
/// 取不到时返回 0，界面就当没有系统栏（顶栏会压到状态栏下，不致命）。
fn system_bar_insets(app: &AndroidApp) -> llt_gui::ScreenInsets {
    read_insets(app).unwrap_or_default()
}

fn read_insets(app: &AndroidApp) -> Option<llt_gui::ScreenInsets> {
    // SAFETY: android-activity 保证这两个指针在 android_main 期间有效。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };

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
    let top = system_inset(&mut env, "getSystemWindowInsetTop");
    let left = system_inset(&mut env, "getSystemWindowInsetLeft");
    let right = system_inset(&mut env, "getSystemWindowInsetRight");
    let bottom = system_inset(&mut env, "getSystemWindowInsetBottom")
        .max(ime_inset(&mut env, &window_insets));

    Some(llt_gui::ScreenInsets {
        top,
        bottom,
        left,
        right,
    })
}

/// 软键盘的高度（API 30+ 才有 `WindowInsets.Type.ime()`，低版本返回 0）。
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

/// 保证私有目录里有一份与 APK 同版本的数据库。
///
/// 只比较版本标记（`assets/timetable/version` 与本地 `llt-timetable.version`），
/// 不做哈希：数据包 41 MB，每次启动算一遍校验和是没必要的开销。
fn install_db(app: &AndroidApp, dir: &Path, db: &Path) -> io::Result<()> {
    let assets = app.asset_manager();

    let wanted = String::from_utf8_lossy(&read_asset(&assets, VERSION_ASSET)?)
        .trim()
        .to_owned();
    if wanted.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("APK 里的 {VERSION_ASSET} 是空的"),
        ));
    }

    let marker = dir.join(VERSION_FILE);
    let installed = fs::read_to_string(&marker)
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    if db.is_file() && installed == wanted {
        return Ok(());
    }

    fs::create_dir_all(dir)?;
    // 先写 `*.tmp` 再改名：中途被杀掉也不会留下半个数据库被 SQLite 当成坏库。
    let tmp = db.with_extension("sqlite.tmp");
    let copied = (|| -> io::Result<()> {
        let mut asset = open_asset(&assets, DB_ASSET)?;
        let mut out = fs::File::create(&tmp)?;
        io::copy(&mut asset, &mut out)?;
        out.sync_all()
    })();
    if let Err(err) = copied {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, db)?;
    fs::write(&marker, &wanted)?;
    eprintln!("已从 APK 解出数据库 {wanted} -> {}", db.display());
    Ok(())
}

fn open_asset(assets: &AssetManager, name: &str) -> io::Result<Asset> {
    let c_name = CString::new(name).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("资源名有 NUL：{name}"))
    })?;
    assets.open(&c_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("APK 里没有 {name}（打包时漏了 assets？见 docs/android.md）"),
        )
    })
}

fn read_asset(assets: &AssetManager, name: &str) -> io::Result<Vec<u8>> {
    let mut asset = open_asset(assets, name)?;
    let mut bytes = Vec::with_capacity(asset.length());
    asset.read_to_end(&mut bytes)?;
    Ok(bytes)
}
