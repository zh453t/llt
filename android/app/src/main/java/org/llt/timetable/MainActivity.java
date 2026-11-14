package org.llt.timetable;

import android.app.Activity;
import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.view.View;
import android.view.Window;
import android.view.WindowInsetsController;
import android.view.WindowManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * 应用入口：{@link NativeActivity} 的子类。
 *
 * <p>界面、查询、下载解码全在 Rust（{@code android/src/}）里，这个类只做两件 Rust 做不到的
 * 事：
 *
 * <ol>
 *   <li>系统文件选择器（SAF）需要 {@code startActivityForResult} / {@code onActivityResult}，
 *       而结果回调只有 Activity 子类才收得到——{@code android.app.NativeActivity} 本身不转发；
 *   <li>选中之后按字节拷贝也放在 Java 侧：{@code ContentResolver} 的流在这里是一等公民，
 *       从 Rust 逐块过 JNI 会慢一个数量级。
 * </ol>
 *
 * <p>Rust 侧通过 JNI 调 {@link #requestImportBackup} / {@link #requestExportBackup}，
 * 结果通过 {@code native*} 方法回到 Rust（注册见 {@code android/src/files.rs}）。
 */
public final class MainActivity extends NativeActivity {
    private static final int REQUEST_IMPORT = 1001;
    private static final int REQUEST_EXPORT = 1002;

    /** 导入：选中的文件拷到这个目录。 */
    private String pendingImportDir;
    /** 导出：要拷出去的源文件与建议文件名。 */
    private String pendingExportSource;
    private String pendingExportName;
    /** 系统栏图标要不要画成深色（浅色主题）；回到前台时按它重新贴一遍。 */
    private boolean lightSystemBars;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // 键盘只「浮」在界面之上：不缩窗口、也不平移窗口。
        // 窗口一 resize，我们的 edge-to-edge 排版会跟着重算，列表/底栏就会跳；
        // 位置稳定更重要，键盘遮挡由界面自己决定怎么处理（见 ScreenInsets）。
        getWindow().setSoftInputMode(
                WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING
                        | WindowManager.LayoutParams.SOFT_INPUT_STATE_UNSPECIFIED);
        // 输入法桥依赖 DecorView 已经建立，放在这里最稳。
        ImeBridge.install(this);
    }

    // —— Rust 调过来的入口（后台线程 → 这里 post 到 UI 线程） ——

    /**
     * 系统栏图标颜色：{@code light=true} 表示界面是浅色，图标要画成深色。
     *
     * <p>窗口是 edge-to-edge 的，状态栏/导航栏直接盖在界面上，那两条栏没有背景色可设，
     * 只剩图标深浅：深色主题下系统默认的白正好，浅色主题下不改就是白字白底。
     * 状态栏、导航栏一起改——底部的手势条／三键同样是画在浅色界面上的。
     *
     * <p>Rust 侧（{@code android/src/bars.rs}）只在主题真的变了时才调，这里再切到
     * UI 线程（系统栏只能在 UI 线程动）。
     */
    public void setLightSystemBars(final boolean light) {
        runOnUiThread(() -> applyLightSystemBars(light));
    }

    private void applyLightSystemBars(boolean light) {
        lightSystemBars = light;
        Window window = getWindow();
        if (window == null) {
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsetsController controller = window.getInsetsController();
            if (controller == null) {
                return;
            }
            int mask = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
            controller.setSystemBarsAppearance(light ? mask : 0, mask);
            return;
        }
        // API 30 以前没有 insetsController：退回到 decorView 上的 systemUiVisibility。
        // minSdk 是 26，两个 LIGHT_* 标志都有（导航栏那个正好从 26 起）。
        View decor = window.getDecorView();
        int flags = decor.getSystemUiVisibility();
        int want = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR
                | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
        decor.setSystemUiVisibility(light ? (flags | want) : (flags & ~want));
    }

    /**
     * 回到前台时按记住的颜色重贴一遍。
     *
     * <p>系统在息屏、任务切换、系统界面重启之后不保证保留外观标志（实测浅色主题下
     * 从后台回来图标又变白），重贴一次很便宜。
     */
    @Override
    protected void onResume() {
        super.onResume();
        applyLightSystemBars(lightSystemBars);
    }

    /** 弹系统选择器挑一份备份，选中后拷进 {@code destDir}。 */
    public void requestImportBackup(final String destDir) {
        runOnUiThread(() -> {
            pendingImportDir = destDir;
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("*/*");
            intent.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{
                    "application/octet-stream", "application/x-sqlite3", "*/*"});
            try {
                startActivityForResult(intent, REQUEST_IMPORT);
            } catch (Exception e) {
                nativeBackupFailed("没有可用的文件选择器：" + e.getMessage());
            }
        });
    }

    /**
     * 「返回」的最后一步：把任务收到后台，而不是 {@code finish()}。
     *
     * <p>{@code finish()} 会销毁 Activity，而进程还活着；再次启动时 winit 的事件循环
     * 已经在同一个进程里建过了，{@code android_main} 再跑一遍也接不上新窗口，
     * 结果就是一片黑（实测）。收到后台既保留了「看起来退出了」的效果，又保住了
     * 事件循环；系统真把它杀了，下次启动会走完整的 {@code onCreate} → {@code android_main}。
     */
    public void moveToBack() {
        runOnUiThread(() -> moveTaskToBack(true));
    }

    /** 弹系统「保存到」选择器，把当前库另存一份。 */
    public void requestExportBackup(final String sourcePath, final String suggestedName) {
        runOnUiThread(() -> {
            pendingExportSource = sourcePath;
            pendingExportName = suggestedName;
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("application/octet-stream");
            intent.putExtra(Intent.EXTRA_TITLE, suggestedName);
            try {
                startActivityForResult(intent, REQUEST_EXPORT);
            } catch (Exception e) {
                nativeBackupFailed("没有可用的文件选择器：" + e.getMessage());
            }
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_IMPORT && requestCode != REQUEST_EXPORT) {
            return;
        }
        Uri uri = data == null ? null : data.getData();
        if (resultCode != RESULT_OK || uri == null) {
            nativeBackupCancelled();
            return;
        }
        if (requestCode == REQUEST_IMPORT) {
            final Uri source = uri;
            new Thread(() -> copyInto(source)).start();
        } else {
            final Uri target = uri;
            new Thread(() -> copyOut(target)).start();
        }
    }

    // —— 拷贝（后台线程） ——

    private void copyInto(Uri source) {
        String dir = pendingImportDir;
        if (dir == null) {
            nativeBackupFailed("没有指定导入目录");
            return;
        }
        File dest = new File(dir, safeName(displayName(source), "backup.sqlite"));
        try (InputStream in = getContentResolver().openInputStream(source);
             OutputStream out = new FileOutputStream(dest)) {
            if (in == null) {
                throw new IllegalStateException("无法打开所选文件");
            }
            copy(in, out);
            nativeImportReady(dest.getAbsolutePath());
        } catch (Exception e) {
            // 失败时别把半个文件留在导入目录里，否则下次扫描会把它列出来。
            //noinspection ResultOfMethodCallIgnored
            dest.delete();
            nativeBackupFailed("导入失败：" + message(e));
        }
    }

    private void copyOut(Uri target) {
        String source = pendingExportSource;
        String name = pendingExportName;
        if (source == null) {
            nativeBackupFailed("没有指定要导出的数据库");
            return;
        }
        try (InputStream in = new java.io.FileInputStream(source);
             OutputStream out = getContentResolver().openOutputStream(target, "wt")) {
            if (out == null) {
                throw new IllegalStateException("无法写入所选位置");
            }
            copy(in, out);
            nativeExportDone(displayName(target) != null ? displayName(target) : name);
        } catch (Exception e) {
            nativeBackupFailed("导出失败：" + message(e));
        }
    }

    private static void copy(InputStream in, OutputStream out) throws Exception {
        byte[] buffer = new byte[64 * 1024];
        int read;
        while ((read = in.read(buffer)) > 0) {
            out.write(buffer, 0, read);
        }
        out.flush();
    }

    // —— SAF 的杂项 ——

    private String displayName(Uri uri) {
        try (Cursor cursor = getContentResolver().query(uri, null, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (index >= 0) {
                    return cursor.getString(index);
                }
            }
        } catch (Exception ignored) {
            // 查询失败不是致命的：下面用兜底名字。
        }
        return null;
    }

    /** 只留文件名本身：有些提供方的 displayName 会带路径分隔符。 */
    private static String safeName(String name, String fallback) {
        if (name == null || name.isEmpty()) {
            return fallback;
        }
        int slash = Math.max(name.lastIndexOf('/'), name.lastIndexOf('\\'));
        String base = slash >= 0 ? name.substring(slash + 1) : name;
        return base.isEmpty() ? fallback : base;
    }

    private static String message(Exception e) {
        String text = e.getMessage();
        return text == null ? e.getClass().getSimpleName() : text;
    }

    /** 选中的备份已拷到 `path`（本地路径）。 */
    private static native void nativeImportReady(String path);

    /** 导出完成，`target` 是用户选的位置（显示用）。 */
    private static native void nativeExportDone(String target);

    /** 用户取消。 */
    private static native void nativeBackupCancelled();

    private static native void nativeBackupFailed(String message);

    /** 供 Rust 侧拿 Activity 自身的辅助：返回应用专属外部目录（没有则 null）。 */
    public String externalFilesPath() {
        File dir = getExternalFilesDir(null);
        return dir == null ? null : dir.getAbsolutePath();
    }

    /** 应用可写的外部目录在部分机型上被禁用，这里给出可用的后备目录。 */
    public static String externalFilesPath(Activity activity) {
        File dir = activity.getExternalFilesDir(null);
        return dir == null ? null : dir.getAbsolutePath();
    }
}
