#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fonts;
mod theme;

use std::path::{Path, PathBuf};

use eframe::egui;

/// 默认窗口大小（逻辑点）。
pub const DEFAULT_WINDOW_SIZE: [f32; 2] = [1600.0, 1000.0];

fn main() -> eframe::Result {
    let db_path = resolve_db_path();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size([480.0, 280.0])
            .with_clamp_size_to_monitor_size(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "路路通时刻表",
        options,
        Box::new(move |cc| Ok(Box::new(app::LltApp::new(cc, db_path)))),
    )
}

/// 解析数据库路径：`--db` → `$LLT_DB` → 可执行文件旁 → 当前目录 → 仓库 `out/timetable`。
///
/// 查找逻辑在 `lltdb::path`（与 CLI 共用一份实现）。
fn resolve_db_path() -> PathBuf {
    if let Some(explicit) = explicit_db_arg() {
        return explicit;
    }
    match lltdb::path::find(None, false) {
        Ok(located) => located.path,
        // 找不到时仍返回一个“预期的位置”，让界面上显示的路径有参考价值。
        Err(_) => default_db_path(),
    }
}

/// 命令行里的 `--db <path>` / `--db=<path>`。
fn explicit_db_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--db" {
            if let Some(path) = args.next()
                && !path.trim().is_empty()
            {
                return Some(PathBuf::from(path));
            }
        } else if let Some(path) = arg.strip_prefix("--db=")
            && !path.trim().is_empty()
        {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// 界面上展示的默认位置：可执行文件旁。
fn default_db_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(lltdb::path::DB_FILE)
}
