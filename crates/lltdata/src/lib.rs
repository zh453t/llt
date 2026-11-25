//! 路路通数据管线：下载数据包 → 解包 → 解码 → 写 SQLite/CSV。
//!
//! 这一层与平台无关（纯计算 + 文件 IO），`llt`（CLI）、`llt-gui`（桌面与 Android
//! 的「数据」页）都调它，避免同一套解码逻辑出现两份实现。
//!
//! 入口是[`build`]：给它输出目录、可选的本地输入和进度回调，它负责其余的判断
//! （要不要重建、下载哪个镜像、版本是否一致）。`inspect_db` / [`check::decide`] /
//! [`update::fetch`] 单独暴露给界面，用来显示「当前数据 / 线上数据 / 该不该更新」。
//!
//! 两条约定：
//!
//! - **不打印**。所有进度通过 [`Progress`] 回调出去，CLI 打到 stderr、界面打进状态栏。
//! - **不猜路径**。输出目录、临时目录都由调用方给：CLI 用可执行文件旁，Android 用
//!   应用私有目录（Android 没有 `/tmp`，`tempfile` 的默认目录在那边不可写）。

pub mod archive;
pub mod build;
pub mod check;
pub mod decode;
pub mod download;
pub mod duty;
pub mod export;
pub mod extras;
pub mod format;
pub mod local;
pub mod platform;
pub mod progress;
pub mod routes;
pub mod samecity;
pub mod spdata;
pub mod update;
pub mod xw;

pub use build::{Options, Report, build};
pub use check::{Decision, decide};
pub use local::LocalState;
pub use progress::{Progress, Silent};
pub use update::RemoteVersion;

/// 官方数据包地址（版本接口没给镜像时的兜底）。
pub const DEFAULT_DATA_URL: &str = "http://down.lltskb.com/an.db";

/// 版本接口地址。
pub const DEFAULT_VERSION_URL: &str = "http://down.lltskb.com/android.ver";
