//! 系统文件选择器（SAF）的 JNI 实现（Java 侧见 `MainActivity`）。
//!
//! 为什么要 Java：`ACTION_OPEN_DOCUMENT` / `ACTION_CREATE_DOCUMENT` 的结果只在
//! `Activity.onActivityResult` 里回来，而 `NativeActivity` 不转发它——所以应用入口
//! 换成了 `MainActivity extends NativeActivity`（见 `android/app/src/main/java`）。
//!
//! 分工：Java 侧负责弹选择器 + 用 `ContentResolver` 把字节搬到本地路径，
//! 这里只把这几个原生方法注册上去，并把结果转成 [`FileEvent`] 交给界面。

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use android_activity::AndroidApp;
use jni::objects::{JObject, JString, JValue};
use jni::{JNIEnv, JavaVM, NativeMethod};

use llt_gui::egui;
use llt_gui::files::{FileEvent, FilePicker};

/// 用户选中的结果，等界面来取。
static EVENTS: Mutex<Vec<FileEvent>> = Mutex::new(Vec::new());

/// 界面句柄：选择器在 Java 的 UI 线程和后台拷贝线程里回调，egui 不会自己醒。
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// `install` 时记下的 Activity，后面每次弹选择器都要用。
static APP: OnceLock<AndroidApp> = OnceLock::new();

fn push(event: FileEvent) {
    match EVENTS.lock() {
        Ok(mut events) => events.push(event),
        Err(err) => err.into_inner().push(event),
    }
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

// —— Java 调过来的原生方法 ——

extern "system" fn native_import_ready(mut env: JNIEnv, _this: JObject, path: JString) {
    if let Ok(value) = env.get_string(&path) {
        let value: String = value.into();
        if !value.is_empty() {
            push(FileEvent::Imported(PathBuf::from(value)));
        }
    }
}

extern "system" fn native_export_done(mut env: JNIEnv, _this: JObject, target: JString) {
    if let Ok(value) = env.get_string(&target) {
        let value: String = value.into();
        push(FileEvent::Exported(value));
    }
}

extern "system" fn native_backup_cancelled(_env: JNIEnv, _this: JObject) {
    push(FileEvent::Cancelled);
}

extern "system" fn native_backup_failed(mut env: JNIEnv, _this: JObject, message: JString) {
    let text = env
        .get_string(&message)
        .map(String::from)
        .unwrap_or_else(|_| "文件选择失败".to_owned());
    push(FileEvent::Failed(text));
}

/// 把原生方法注册到 `MainActivity` 上，返回给界面用的选择器。
///
/// 注册失败不阻断启动：界面会退化成「只能从目录里导入/导出到固定目录」，
/// 查询和数据下载都不受影响。
pub fn attach(app: &AndroidApp) -> Option<Arc<Picker>> {
    let _ = APP.set(app.clone());
    let methods = [
        NativeMethod {
            name: "nativeImportReady".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_import_ready as *mut c_void,
        },
        NativeMethod {
            name: "nativeExportDone".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_export_done as *mut c_void,
        },
        NativeMethod {
            name: "nativeBackupCancelled".into(),
            sig: "()V".into(),
            fn_ptr: native_backup_cancelled as *mut c_void,
        },
        NativeMethod {
            name: "nativeBackupFailed".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_backup_failed as *mut c_void,
        },
    ];

    let registered = with_activity(app, |env, activity| {
        let class = env
            .call_method(activity, "getClass", "()Ljava/lang/Class;", &[])?
            .l()?;
        env.register_native_methods(jni::objects::JClass::from(class), &methods)?;
        Ok(())
    });
    match registered {
        Ok(()) => Some(Arc::new(Picker)),
        Err(err) => {
            eprintln!("文件选择器接不上，导入导出只能走目录：{err}");
            None
        }
    }
}

/// 实现 [`FilePicker`]：队列与 Activity 都是全局的，所以它只是个空壳。
pub struct Picker;

impl FilePicker for Picker {
    fn request_import(&self, dest_dir: &Path) {
        let dir = dest_dir.display().to_string();
        if let Err(err) = call_activity("requestImportBackup", &[dir.as_str()]) {
            push(FileEvent::Failed(format!("无法打开文件选择器：{err}")));
        }
    }

    fn request_export(&self, source: &Path, suggested_name: &str) {
        let source = source.display().to_string();
        if let Err(err) = call_activity("requestExportBackup", &[source.as_str(), suggested_name]) {
            push(FileEvent::Failed(format!("无法打开保存对话框：{err}")));
        }
    }

    fn drain(&self) -> Vec<FileEvent> {
        match EVENTS.lock() {
            Ok(mut events) => std::mem::take(&mut *events),
            Err(err) => std::mem::take(&mut *err.into_inner()),
        }
    }

    fn attach_context(&self, ctx: &egui::Context) {
        let _ = CTX.set(ctx.clone());
    }
}

// ---------------------------------------------------------------- JNI 辅助

/// 调 `MainActivity` 上的实例方法；参数全是字符串（最多两个）。
fn call_activity(method: &str, strings: &[&str]) -> jni::errors::Result<()> {
    let app = APP
        .get()
        .ok_or(jni::errors::Error::NullPtr("MainActivity"))?;
    // SAFETY: `android_main` 期间 Activity 指针由 android-activity 保证有效。
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };

    let owned: Vec<JString> = strings
        .iter()
        .map(|value| env.new_string(value))
        .collect::<jni::errors::Result<_>>()?;
    let args: Vec<JValue> = owned.iter().map(|value| JValue::Object(value)).collect();
    let signature = match strings.len() {
        1 => "(Ljava/lang/String;)V",
        2 => "(Ljava/lang/String;Ljava/lang/String;)V",
        _ => "()V",
    };
    env.call_method(&activity, method, signature, &args)?;
    Ok(())
}

fn with_activity(
    app: &AndroidApp,
    f: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<()>,
) -> jni::errors::Result<()> {
    // SAFETY: 同 `call_activity`。
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = f(&mut env, &activity);
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
    }
    result
}
