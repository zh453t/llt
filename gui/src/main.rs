//! 桌面入口。界面与启动逻辑在 `lib.rs`（与 Android 共用），这里只负责
//! 在 Windows 的发布构建里隐藏控制台窗口。

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

fn main() -> eframe::Result {
    llt_gui::run_desktop()
}
