//! 系统输入法桥（Java 侧见 `android/java/org/llt/timetable/ImeBridge.java`）。
//!
//! 为什么要自己接一层：`NativeActivity` 的窗口里没有一个带 `InputConnection` 的
//! View，输入法只能退回「发按键」，而按键一次只带一个字符——只有 ASCII 能糊过去。
//! 中文要经拼音合成、候选，最后整词 `commitText("北京")` 上屏，没有
//! `InputConnection` 就整个丢掉；winit 0.30 的 Android 后端也不产生
//! `WindowEvent::Ime`，egui-winit 因此永远收不到合成串。
//!
//! 链路：输入法 → Java `InputConnection` → 这里的 `native*` 函数 → 事件队列 →
//! `llt_gui` 每帧 `drain()` 取走，转成 `egui::Event` 推给输入框。

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};

use android_activity::AndroidApp;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM, NativeMethod};

use llt_gui::egui;
use llt_gui::{ImeBridge as ImeBridgeTrait, ImeInput};

/// Java 侧的类（`android/app/src/main/java/org/llt/timetable/ImeBridge.java`）。
const CLASS: &str = "org/llt/timetable/ImeBridge";

/// 等界面来取的事件队列。
///
/// 用全局而不是挂在实例上：Java 的 `native` 方法是静态的，拿不到实例；一个进程
/// 只有一个 Activity，也不需要按实例区分。
static QUEUE: Mutex<Vec<ImeInput>> = Mutex::new(Vec::new());

/// `install` 时记下的 Activity，供后面重新抢焦点用。
static APP: OnceLock<AndroidApp> = OnceLock::new();

/// 装好的 `ImeBridge` 类引用：焦点一变就要用，不必每次重新解析。
static CLASS_REF: OnceLock<GlobalRef> = OnceLock::new();

/// 界面句柄：输入法上屏后催一帧，否则界面空闲时不会重绘，中文字要等下一次
/// 别的事件才出现（实测约 1 秒，观感就是「提交候选词后卡一下」）。
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

fn push(input: ImeInput) {
    match QUEUE.lock() {
        Ok(mut queue) => queue.push(input),
        // 队列里不可能 panic（只是 Vec 的 push），真出事也不要连带把输入法拖垮。
        Err(err) => err.into_inner().push(input),
    }
    // 事件是在输入法的线程里来的，egui 不会自己醒。
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
}

// —— Java 调过来的原生方法 ——

extern "system" fn native_commit(mut env: JNIEnv, _this: JObject, text: JString) {
    let value = env.get_string(&text).map(String::from).unwrap_or_default();
    if !value.is_empty() {
        push(ImeInput::Commit(value));
    }
}

extern "system" fn native_compose(mut env: JNIEnv, _this: JObject, text: JString) {
    let value = env.get_string(&text).map(String::from).unwrap_or_default();
    push(ImeInput::Preedit(value));
}

extern "system" fn native_delete(_env: JNIEnv, _this: JObject, before: jint, after: jint) {
    push(ImeInput::Delete {
        before: before.max(0) as usize,
        after: after.max(0) as usize,
    });
}

extern "system" fn native_enter(_env: JNIEnv, _this: JObject) {
    push(ImeInput::Enter);
}

/// 用户自己收起了软键盘（返回键、键盘上的「收起」）。
///
/// Java 侧从窗口 insets 的 IME 可见性变化里看出来的（见 `ImeBridge.onApplyWindowInsets`）。
extern "system" fn native_ime_hidden(_env: JNIEnv, _this: JObject) {
    push(ImeInput::Hidden);
}

/// 把输入法接上：注册原生方法，再让 Java 侧把 1×1 的 View 挂到 DecorView 上。
///
/// 失败不阻断启动——退化成修复前的样子（ASCII 能用、中文不能用），
/// 而不是连界面都起不来。
pub fn attach(app: &AndroidApp) -> Option<Arc<Bridge>> {
    let _ = APP.set(app.clone());

    let registered = with_class(app, |env, class| {
        let methods = [
            NativeMethod {
                name: "nativeCommit".into(),
                sig: "(Ljava/lang/String;)V".into(),
                fn_ptr: native_commit as *mut c_void,
            },
            NativeMethod {
                name: "nativeCompose".into(),
                sig: "(Ljava/lang/String;)V".into(),
                fn_ptr: native_compose as *mut c_void,
            },
            NativeMethod {
                name: "nativeDelete".into(),
                sig: "(II)V".into(),
                fn_ptr: native_delete as *mut c_void,
            },
            NativeMethod {
                name: "nativeEnter".into(),
                sig: "()V".into(),
                fn_ptr: native_enter as *mut c_void,
            },
            NativeMethod {
                name: "nativeImeHidden".into(),
                sig: "()V".into(),
                fn_ptr: native_ime_hidden as *mut c_void,
            },
        ];
        env.register_native_methods(class, &methods)?;

        let activity = activity(app);
        env.call_static_method(
            class,
            "install",
            "(Landroid/app/Activity;)V",
            &[JValue::Object(&activity)],
        )?;
        Ok(())
    });

    match registered {
        Ok(()) => {
            eprintln!("输入法桥已接上（Java ImeBridge）");
            Some(Arc::new(Bridge))
        }
        Err(err) => {
            // 常见原因：Java 类改名没同步，或 Gradle 没把 java/ 编进去。
            eprintln!("输入法桥接失败，中文输入将不可用：{err}");
            None
        }
    }
}

/// 实现 `llt_gui::ImeBridge`：队列是全局的，所以它只是个空壳。
pub struct Bridge;

impl ImeBridgeTrait for Bridge {
    fn drain(&self) -> Vec<ImeInput> {
        match QUEUE.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(err) => std::mem::take(&mut *err.into_inner()),
        }
    }

    fn set_text_focus(&self, focused: bool) {
        set_active(focused);
    }

    fn ensure_shown(&self) {
        call_static("ensureActive", &[]);
    }

    fn attach_context(&self, ctx: &egui::Context) {
        let _ = EGUI_CTX.set(ctx.clone());
    }
}

/// 调 `ImeBridge` 上的静态方法（需要参数时自己拼 `JValue`）。
fn call_static(method: &str, args: &[JValue<'_, '_>]) {
    let Some(app) = APP.get() else {
        return;
    };
    let result = with_class(app, |env, class| {
        let signature = if args.is_empty() { "()V" } else { "(Z)V" };
        env.call_static_method(class, method, signature, args)?;
        Ok(())
    });
    if let Err(err) = result {
        eprintln!("输入法调用 {method} 失败：{err}");
    }
}

/// 通知 Java 侧「有 / 没有输入框拿着焦点」：抢焦点、弹 / 收键盘。
///
/// 只在焦点变化那一刻调；切页、清空都会重新触发，不必逐帧问。
fn set_active(active: bool) {
    let Some(app) = APP.get() else {
        return;
    };
    let result = with_class(app, |env, class| {
        env.call_static_method(
            class,
            "setActive",
            "(Z)V",
            &[JValue::Bool(u8::from(active))],
        )?;
        Ok(())
    });
    if let Err(err) = result {
        eprintln!("输入法焦点切换失败：{err}");
    }
}

fn activity(app: &AndroidApp) -> JObject<'static> {
    // SAFETY: `android_main` 期间活动指针由 android-activity 保证有效。
    unsafe { JObject::from_raw(app.activity_as_ptr().cast()) }
}

/// 拿到 Java 侧的 `ImeBridge` 类并执行一段 JNI。
///
/// 这个线程不是从 Java 调进来的，`FindClass` 走系统类加载器，找不到应用自己的类；
/// 所以老老实实经 Activity 的 ClassLoader 去 `loadClass`（一次成功后缓存全局引用）。
fn with_class(
    app: &AndroidApp,
    f: impl FnOnce(&mut JNIEnv<'_>, &JClass<'_>) -> jni::errors::Result<()>,
) -> jni::errors::Result<()> {
    // SAFETY: 同 `activity`。
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let result = (|| {
        let class = match CLASS_REF.get() {
            Some(cached) => cached.clone(),
            None => {
                let loader = env
                    .call_method(
                        activity(app),
                        "getClassLoader",
                        "()Ljava/lang/ClassLoader;",
                        &[],
                    )?
                    .l()?;
                let name = env.new_string(CLASS.replace('/', "."))?;
                let class = env
                    .call_method(
                        &loader,
                        "loadClass",
                        "(Ljava/lang/String;)Ljava/lang/Class;",
                        &[JValue::Object(&name)],
                    )?
                    .l()?;
                let class = env.new_global_ref(JClass::from(class))?;
                let _ = CLASS_REF.set(class.clone());
                class
            }
        };
        f(&mut env, class.as_obj().into())
    })();
    if result.is_err() && env.exception_check().unwrap_or(false) {
        // 把 Java 异常栈打到 logcat，否则只看得到一句「Java exception was thrown」。
        let _ = env.exception_describe();
    }
    result
}
