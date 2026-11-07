package org.llt.timetable;

import android.app.Activity;
import android.content.Context;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
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
    /** UI 线程上创建的唯一实例；没装上时为 null。 */
    private static ImeBridge sInstance;

    private final BridgeConnection connection = new BridgeConnection(this);

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
        bridge.post(new Runnable() {
            @Override
            public void run() {
                InputMethodManager imm =
                        (InputMethodManager)
                                bridge.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
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
