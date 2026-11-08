//! 应用级动作：把「返回」的最后一步交回系统。
//!
//! Android 的左/右边缘滑动与返回键都是同一个返回事件（winit 映射成
//! `Key::BrowserBack`）。界面自己的页面（经由、车次详情）关完之后，剩下的含义就是
//! 「离开应用」，这里调 `activity.finish()`——和系统默认行为一致。
//!
//! 没有走 `NativeActivity` 的默认处理是因为 android-activity 把输入事件接进了自己的
//! 队列：不显式 finish 的话，返回键会变成「什么也不发生」。

use std::sync::Arc;

use android_activity::AndroidApp;
use jni::JavaVM;
use jni::objects::JObject;

use llt_gui::AppControl;

/// 给界面用的控制句柄。
pub fn attach(app: &AndroidApp) -> Arc<dyn AppControl> {
    Arc::new(Control { app: app.clone() })
}

struct Control {
    app: AndroidApp,
}

impl AppControl for Control {
    fn exit(&self) {
        if let Err(err) = finish(&self.app) {
            eprintln!("退出应用失败：{err}");
        }
    }
}

fn finish(app: &AndroidApp) -> jni::errors::Result<()> {
    // SAFETY: `android_main` 期间这两个指针由 android-activity 保证有效。
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    env.call_method(&activity, "finish", "()V", &[])?;
    Ok(())
}
