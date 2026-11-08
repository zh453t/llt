//! 系统文件选择器（Android 的 SAF）与界面之间的接口。
//!
//! 桌面没有这一层：备份的导入导出直接读写出可执行文件旁的目录（见 [`crate::data`]）。
//! Android 上普通应用读不到别人的文件，必须走系统选择器；而选择器的结果只回得到
//! `Activity` 子类里（`onActivityResult`），所以由平台侧实现这个 trait，界面只收发事件。
//!
//! 分工：**平台负责把字节搬到本地路径**（Android 在 Java 里用 `ContentResolver` 的流，
//! 逐块过 JNI 太慢），界面只负责校验、替换和提示。

use std::path::{Path, PathBuf};

/// 选择器回来的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEvent {
    /// 用户选了一份备份，平台已把它复制到这个本地路径。
    Imported(PathBuf),
    /// 导出完成，`target` 是用户选的位置（给提示文案用，可能是 `content://` URI）。
    Exported(String),
    /// 用户取消。
    Cancelled,
    /// 失败（文案可以直接显示）。
    Failed(String),
}

/// 平台文件选择器。实现必须是线程安全的：回调来自 UI 线程。
pub trait FilePicker: Send + Sync {
    /// 弹「打开」选择器；平台负责把选中的文件复制到 `dest_dir`。
    fn request_import(&self, dest_dir: &Path);
    /// 弹「保存到」选择器，把 `source` 另存为 `suggested_name`。
    fn request_export(&self, source: &Path, suggested_name: &str);
    /// 取走这一帧攒下的事件。
    fn drain(&self) -> Vec<FileEvent>;
    /// 回调发生在别的线程，egui 不会自己醒，需要句柄催一帧。
    fn attach_context(&self, ctx: &crate::egui::Context);
}
