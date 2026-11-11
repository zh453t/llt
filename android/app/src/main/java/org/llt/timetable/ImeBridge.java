package org.llt.timetable;

import android.app.Activity;
import android.content.Context;
import android.os.Build;
import android.os.SystemClock;
import android.text.InputType;
import android.util.Log;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

/**
 * 系统输入法桥：把输入法的上屏文字交给 Rust 侧的 egui。
 *
 * <p>{@link android.app.NativeActivity} 的窗口里没有一个带 {@link InputConnection} 的
 * View，输入法因此只能退回「发按键」那条路；而按键一次只带一个字符，只有 ASCII 能糊过去。
 * 中文要经过拼音合成、候选，最后以 {@code commitText("北京")} 整词上屏，没有
 * InputConnection 就整个丢掉——这就是「中文输入法失效」的成因。
 *
 * <p>这里挂一个 1×1、透明的 View 到 DecorView 上并抢到焦点，用自己的
 * {@link InputConnection} 收上屏文字，再经 native 方法交给 Rust。View 做成 1×1
 * 是为了不挡住 NativeActivity 自己的 SurfaceView（触摸照旧走 winit）。
 *
 * <p>编译方式见 {@code android/build.ps1}：javac → d8 → 打成一个 dex 塞进 cargo-apk
 * 产出的 APK（cargo-apk 0.10 自己不产 dex，所以就是 {@code classes.dex}）。
 */
public final class ImeBridge extends View {
    private static final String TAG = "ImeBridge";

    /**
     * 我们自己刚请求过显示时，这么长时间内的「收起」都当成抖动。
     *
     * <p>{@code restartInput}（切输入框）和 egui 每次点击后的
     * {@code set_ime_allowed(false)+(true)} 都会让输入法收起再弹出：insets 上是
     * 22 毫秒里「显示 → 收起 → 显示 → 收起」（实测 logcat）。那个「收起」不是用户干的，
     * 不能拿去清焦点，否则切个输入框键盘就再也不回来了。
     */
    private static final long CHURN_MS = 600;

    /** UI 线程上创建的唯一实例；没装上时为 null。 */
    private static ImeBridge sInstance;

    private final BridgeConnection connection = new BridgeConnection(this);

    /** 输入法当前是否显示：由窗口 insets 的 IME 可见性推出来。 */
    private boolean imeVisible;

    /** 我们自己刚请求过显示的时刻（见 {@link #CHURN_MS}）。渲染线程也会写，故 volatile。 */
    private volatile long churnUntilMs;

    /** 这次「收起」是我们自己要的（egui 焦点丢了），不是用户按的。 */
    private volatile boolean hidingByUs;

    /**
     * 用户自己把键盘收了（返回键、「收起」按钮）。
     *
     * <p>在这种状态下不再自动弹回；下次显式 {@link #setActive(boolean)} 时清掉。
     */
    private volatile boolean dismissed;

    private ImeBridge(Context context) {
        super(context);
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    /**
     * 把自己挂到 DecorView 上并抢焦点。
     *
     * <p>Rust 从 NativeActivity 的渲染线程调过来，而 View 树只能在 UI 线程动，
     * 所以统一 post 到 UI 线程执行。
     */
    public static void install(final Activity activity) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (sInstance != null) {
                    return;
                }
                ViewGroup decor = (ViewGroup) activity.getWindow().getDecorView();
                ImeBridge bridge = new ImeBridge(activity);
                // 1×1：既不遮挡内容，也仍然是一个「可见、可获焦」的合法输入目标。
                decor.addView(bridge, new ViewGroup.LayoutParams(1, 1));
                sInstance = bridge;
                bridge.requestFocus();
            }
        });
    }

    /**
     * egui 里有没有输入框拿着焦点：有就抢焦点并弹键盘，没有就收起来。
     *
     * <p>键盘必须由这里弹：winit 的 `set_ime_allowed` 是拿 DecorView 去
     * `showSoftInput` 的，而焦点已经在这个 1×1 的 View 上，系统会直接忽略那次请求
     * （实测 `dumpsys input_method` 里 `mInputShown=false`）。
     */
    public static void setActive(final boolean active) {
        final ImeBridge bridge = sInstance;
        if (bridge == null) {
            return;
        }
        // 这两句必须**同步**执行：焦点一变，winit 那一帧就会先
        // `set_ime_allowed(false)+(true)`，insets 上的「收起」比 post 出去的
        // Runnable 还早 1 毫秒到（实测 54.638 收起 / 54.641 才轮到焦点切换那条链）。
        bridge.dismissed = false;
        if (active) {
            bridge.churnUntilMs = SystemClock.uptimeMillis() + CHURN_MS;
        }
        bridge.post(new Runnable() {
            @Override
            public void run() {
                InputMethodManager imm =
                        (InputMethodManager)
                                bridge.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                bridge.hidingByUs = !active;
                if (!active) {
                    if (imm != null) {
                        imm.hideSoftInputFromWindow(bridge.getWindowToken(), 0);
                    }
                    return;
                }
                if (!bridge.hasFocus() && !bridge.requestFocus()) {
                    return;
                }
                if (imm != null) {
                    imm.restartInput(bridge);
                    imm.showSoftInput(bridge, 0);
                }
            }
        });
    }

    /**
     * 焦点没变、用户又点了一下输入框：只把键盘叫回来。
     *
     * <p>不能复用 {@link #setActive(boolean)}：那里有 {@code restartInput}，
     * 键盘已经显示时重启输入会话会让它闪一下又收起。
     */
    public static void ensureActive() {
        final ImeBridge bridge = sInstance;
        if (bridge == null) {
            return;
        }
        bridge.churnUntilMs = SystemClock.uptimeMillis() + CHURN_MS;
        bridge.post(new Runnable() {
            @Override
            public void run() {
                // 用户刚自己收起的键盘不要弹回来（egui 每次点击都会 request_focus，
                // 这条 show 是给它兜底的，不该盖过用户的意思）。
                if (bridge.dismissed) {
                    return;
                }
                InputMethodManager imm =
                        (InputMethodManager) bridge
                                .getContext()
                                .getSystemService(Context.INPUT_METHOD_SERVICE);
                // 只 show，不 requestFocus：桥 View 已经拿着焦点，重新请求会把
                // InputConnection 换掉，正在连续输入的按键/上屏会被丢掉
                // （实测：`adb input text K1061` 只剩第一个字符）。
                // 也不能用 `imm.isActive()` 当开关：键盘收起时它仍可能是 true。
                if (imm != null) {
                    imm.showSoftInput(bridge, 0);
                }
            }
        });
    }

    /**
     * 盯着输入法的可见性，识别「用户自己把键盘收了」。
     *
     * <p>返回键或键盘上的「收起」只改窗口 insets：输入法不会通知 {@code InputConnection}，
     * egui 也不知道。不处理的话输入框一直保持焦点（光标还在闪），下一次重绘/点击
     * 又会把键盘弹回来——实测按返回收起后 5 秒内自己弹回。
     *
     * <p>API 30 起 {@code WindowInsets.Type.ime()} 直接给了可见性；更老的系统
     * 只能近似：底部 inset 比稳定 inset 高出一块，就认为键盘在。
     */
    @Override
    public WindowInsets onApplyWindowInsets(WindowInsets insets) {
        boolean visible = isImeVisible(insets);
        if (visible != imeVisible) {
            imeVisible = visible;
            Log.i(TAG, "输入法 " + (visible ? "显示" : "收起"));
            if (!visible) {
                if (hidingByUs) {
                    hidingByUs = false;
                } else if (SystemClock.uptimeMillis() < churnUntilMs) {
                    // 刚请求过显示：这是重启输入会话的抖动，不是用户收起了键盘。
                    Log.i(TAG, "收起是我方抖动，忽略");
                } else {
                    dismissed = true;
                    nativeImeHidden();
                }
            }
        }
        return super.onApplyWindowInsets(insets);
    }

    private static boolean isImeVisible(WindowInsets insets) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            return insets.isVisible(WindowInsets.Type.ime());
        }
        // API 30 以前没有 ime() 类型：键盘弹起时 systemWindowInsetBottom 会明显
        // 大于导航栏（stable inset）。
        return insets.getSystemWindowInsetBottom() > insets.getStableInsetBottom();
    }

    /**
     * 输入法按这里填的 {@code inputType} 决定要不要做合成与候选。
     *
     * <p>留 `TYPE_NULL`（默认）时输入法会认定「对面只认按键」，中文输入直接被禁用，
     * 这正是修复前 {@code dumpsys input_method} 里看到的 {@code inputType=0x0}。
     */
    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        out.inputType = InputType.TYPE_CLASS_TEXT;
        out.imeOptions = EditorInfo.IME_ACTION_DONE
                | EditorInfo.IME_FLAG_NO_FULLSCREEN
                | EditorInfo.IME_FLAG_NO_EXTRACT_UI;
        return connection;
    }

    /**
     * 实体键盘 / `adb shell input` 的按键：焦点在这个 View 上，不转发就到不了 Rust。
     *
     * <p>软键盘走 {@link InputConnection}，这两条路不会重复。
     */
    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (keyCode == KeyEvent.KEYCODE_DEL) {
            nativeDelete(1, 0);
            return true;
        }
        if (keyCode == KeyEvent.KEYCODE_ENTER || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER) {
            nativeEnter();
            return true;
        }
        int unicode = event.getUnicodeChar();
        if (unicode >= 0x20 && unicode != 0x7f) {
            nativeCommit(new String(Character.toChars(unicode)));
            return true;
        }
        return super.onKeyDown(keyCode, event);
    }

    // —— Rust 侧注册的原生方法（见 android/src/ime.rs） ——

    /** 最终上屏的文字，中文是整词。 */
    private static native void nativeCommit(String text);

    /** 正在合成、还没上屏的串（拼音 / 候选状态）；空串表示合成结束。 */
    private static native void nativeCompose(String text);

    /** 删除光标前后若干字符。 */
    private static native void nativeDelete(int before, int after);

    /** 回车 /「完成」。 */
    private static native void nativeEnter();

    /** 用户自己收起了软键盘（见 {@link #onApplyWindowInsets}）。 */
    private static native void nativeImeHidden();

    private static final class BridgeConnection extends BaseInputConnection {
        BridgeConnection(View target) {
            // 第二个参数是「文本是否由输入法自己维护」：这里我们自己转发，不需要它维护。
            super(target, false);
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            String value = text == null ? "" : text.toString();
            if (!value.isEmpty()) {
                nativeCommit(value);
            }
            return true;
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            nativeCompose(text == null ? "" : text.toString());
            return true;
        }

        @Override
        public boolean finishComposingText() {
            nativeCompose("");
            return true;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            nativeDelete(beforeLength, afterLength);
            return true;
        }

        @Override
        public boolean performEditorAction(int actionCode) {
            // IME_ACTION_DONE / IME_ACTION_GO 都当回车：界面拿它触发查询。
            nativeEnter();
            return true;
        }

        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            // 有些输入法（尤其国产 ROM）退格与回车仍走按键，而不是 edit 指令。
            if (event.getAction() == KeyEvent.ACTION_DOWN) {
                int keyCode = event.getKeyCode();
                if (keyCode == KeyEvent.KEYCODE_DEL) {
                    nativeDelete(1, 0);
                    return true;
                }
                if (keyCode == KeyEvent.KEYCODE_ENTER
                        || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER) {
                    nativeEnter();
                    return true;
                }
            }
            return super.sendKeyEvent(event);
        }
    }
}
