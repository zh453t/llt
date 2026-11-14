//! 系统栏（状态栏 / 导航栏）图标颜色。
//!
//! 窗口是 edge-to-edge 的：内容铺到状态栏和导航栏下面，那两条栏本身没有背景色可设，
//! 只剩「图标画白还是画黑」一件事。深色主题下系统默认的白正好；浅色主题下不改就是
//! 白字白底——状态栏上的时间、信号、电量全看不见（实测），必须显式要深色图标。
//!
//! 只有主题真的变了界面才会调一次（对比在 `llt_gui` 那边做），这里只管过桥：
//! `MainActivity.setLightSystemBars(boolean)` 会在 UI 线程上按版本分派到
//! `WindowInsetsController.setSystemBarsAppearance`（API 30+）或
//! `View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR` / `..._LIGHT_NAVIGATION_BAR`。

use std::sync::Arc;

use android_activity::AndroidApp;
use jni::JavaVM;
use jni::objects::{JObject, JValue};

use llt_gui::SystemBars;

/// 给界面用的系统栏句柄。
pub fn attach(app: &AndroidApp) -> Arc<dyn SystemBars> {
    Arc::new(Bars { app: app.clone() })
}

struct Bars {
    app: AndroidApp,
}

impl SystemBars for Bars {
    fn set_light(&self, light: bool) {
        if let Err(err) = call_set_light(&self.app, light) {
            eprintln!("设置系统栏图标颜色失败：{err}");
        }
    }
}

fn call_set_light(app: &AndroidApp, light: bool) -> jni::errors::Result<()> {
    // SAFETY: `android_main` 期间这两个指针由 android-activity 保证有效。
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    env.call_method(
        &activity,
        "setLightSystemBars",
        "(Z)V",
        &[JValue::Bool(u8::from(light))],
    )?;
    Ok(())
}
