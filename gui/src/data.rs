//! 「数据」页：当前库状态、检查更新、下载重建、备份的导入导出。
//!
//! 这一页在桌面和 Android 上是同一份代码，差别只有目录（见 [`DataDirs`]）：
//! 桌面是「可执行文件旁」（延续绿色版的习惯），Android 是应用私有目录 +
//! 应用专属外部目录（用户能通过 USB 存取）。
//!
//! 耗时动作（联网检查、下载重建、复制 42 MB 备份）都在后台线程里跑，界面只读
//! 一份快照；任务结束时返回 `true` 表示「库变了，调用方该重开数据库」。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, Ui};

use lltdata::local::LocalState;
use lltdata::{Decision, Options, Progress, RemoteVersion};

use crate::{config, theme};

/// 数据页用到的目录。
///
/// 由平台给：桌面全是可执行文件旁；Android 的输出在私有目录、备份在应用专属
/// 外部目录（`Android/data/<包名>/files`），临时目录在 cache（没有 `/tmp`）。
#[derive(Debug, Clone, Default)]
pub struct DataDirs {
    /// 备份导入时的搜索目录。
    pub import: PathBuf,
    /// 导出备份写到这里。
    pub export: PathBuf,
    /// 构建用的临时目录父目录。
    pub temp: PathBuf,
}

impl DataDirs {
    /// 桌面：全在可执行文件旁，临时目录用系统默认。
    pub fn beside_exe() -> Self {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            import: dir.clone(),
            export: dir,
            temp: std::env::temp_dir(),
        }
    }
}

/// 投票结果：这一帧界面要不要重开数据库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataOutcome {
    /// 库被替换过（下载重建 / 导入备份）。
    pub reload_db: bool,
}

/// 后台任务类型，只用来把文案说清楚。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Check,
    Build,
    Import,
    Export,
}

impl TaskKind {
    fn label(self) -> &'static str {
        match self {
            Self::Check => "检查更新",
            Self::Build => "下载并重建",
            Self::Import => "导入备份",
            Self::Export => "导出备份",
        }
    }
}

/// 后台线程与界面之间传递进度的小黑板。
#[derive(Default)]
struct TaskProgress {
    /// 最近几条提示（只留尾部，界面显示最后几行）。
    notes: Mutex<Vec<String>>,
    /// 下载字节数与总量。
    download: Mutex<Option<(u64, Option<u64>)>>,
}

impl TaskProgress {
    fn notes(&self) -> Vec<String> {
        self.notes
            .lock()
            .map(|notes| notes.clone())
            .unwrap_or_default()
    }

    fn download(&self) -> Option<(u64, Option<u64>)> {
        self.download.lock().ok().and_then(|value| *value)
    }
}

impl Progress for TaskProgress {
    fn note(&self, text: &str) {
        if let Ok(mut notes) = self.notes.lock() {
            notes.push(text.to_owned());
            // 只留最后 6 条：日志是给人扫一眼的，不是日志文件。
            if notes.len() > 6 {
                notes.remove(0);
            }
        }
    }

    fn download(&self, done: u64, total: Option<u64>) {
        if let Ok(mut value) = self.download.lock() {
            *value = Some((done, total));
        }
    }
}

/// 正在跑的任务。
struct Task {
    kind: TaskKind,
    progress: Arc<TaskProgress>,
    /// 任务结束时由线程 side 发回结果；用 channel 是为了避免再锁一层。
    result: Receiver<Result<TaskDone, String>>,
}

/// 任务成功后要交给界面的东西。
enum TaskDone {
    /// 检查更新拿到线上版本。
    Remote(RemoteVersion),
    /// 重建完成 / 导入完成（是否需要重开数据库）。
    Rebuilt(String),
    /// 导出完成，附落盘路径。
    Exported(PathBuf),
}

/// 上一次任务的结果（成功/失败的横幅）。
#[derive(Debug, Clone)]
struct Notice {
    ok: bool,
    text: String,
}

/// 备份目录里的一个文件。
#[derive(Debug, Clone)]
struct BackupEntry {
    path: PathBuf,
    bytes: u64,
    /// 库内版本；读不出来时为 `None`（通常意味着不是本工具的库）。
    data_version: Option<String>,
    /// 能否作为备份导入。
    importable: bool,
    /// 不能导入的原因。
    problem: Option<String>,
}

/// 「数据」页的全部状态。
pub struct DataPage {
    dirs: DataDirs,
    /// 当前库的状态快照（每次刷新/任务结束重读）。
    local: Option<LocalState>,
    /// 线上版本（检查过才有）。
    remote: Option<Result<RemoteVersion, String>>,
    /// 正在跑的任务。
    task: Option<Task>,
    /// 上一次任务的结果。
    notice: Option<Notice>,
    /// 备份目录里的文件。
    backups: Vec<BackupEntry>,
    /// 备份目录上次扫描的时间（避免每帧 stat）。
    scanned_at: Option<Instant>,
    /// 等待确认导入的备份。
    pending_import: Option<PathBuf>,
    /// 是否强制重建（跳过“已是最新”的判断）。
    force: bool,
}

/// 备份目录的扫描间隔：目录是用户自己往里放的，不用每帧看。
const SCAN_INTERVAL: Duration = Duration::from_secs(3);

impl DataPage {
    pub fn new(dirs: DataDirs) -> Self {
        Self {
            dirs,
            local: None,
            remote: None,
            task: None,
            notice: None,
            backups: Vec::new(),
            scanned_at: None,
            pending_import: None,
            force: false,
        }
    }

    /// 重读当前库与备份目录（启动、任务结束、切换标签页时调用）。
    pub fn refresh(&mut self, db_path: &Path) {
        self.local = if db_path.is_file() {
            Some(LocalState::read(db_path.to_path_buf()))
        } else {
            None
        };
        self.scan_backups();
    }

    /// 有任务在跑时，别的会改库的按钮都要禁用。
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }

    /// 画这一页。返回是否需要重开数据库。
    pub fn ui(&mut self, ui: &mut Ui, db_path: &Path) -> DataOutcome {
        let mut outcome = DataOutcome::default();
        self.poll_task(db_path, &mut outcome);
        if self
            .scanned_at
            .is_none_or(|at| at.elapsed() >= SCAN_INTERVAL)
        {
            self.scan_backups();
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.current_ui(ui, db_path);
                ui.add_space(theme::GAP_M);
                self.task_ui(ui, db_path);
                ui.add_space(theme::GAP_M);
                self.backup_ui(ui, db_path, &mut outcome);
                if !self.backups.is_empty() {
                    ui.add_space(theme::GAP_M);
                    self.backup_list_ui(ui, db_path, &mut outcome);
                }
                ui.add_space(theme::GAP_M);
                self.notes_ui(ui);
            });

        outcome
    }

    // ------------------------------------------------------------ 分区

    fn current_ui(&mut self, ui: &mut Ui, db_path: &Path) {
        heading(ui, "当前数据");
        match &self.local {
            Some(state) if state.usable() => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = theme::GAP_S;
                    ui.label(body(format!(
                        "数据 {}",
                        state.data_version.as_deref().unwrap_or("未知")
                    )));
                    ui.label(meta(format!(
                        "· 结构 v{}",
                        state
                            .schema_version
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "?".to_owned())
                    )));
                });
                ui.label(meta(format!(
                    "构建于 {}",
                    state.built_at.as_deref().unwrap_or("未知")
                )));
                let bytes = file_size(db_path);
                ui.label(meta(format!(
                    "{} · {}",
                    size_text(bytes),
                    db_path.display()
                )));
            }
            Some(state) => {
                ui.label(body("数据不完整：").color(ui.visuals().error_fg_color));
                for problem in &state.problems {
                    ui.label(meta(format!("· {problem}")));
                }
            }
            None => {
                ui.label(body(format!("还没有数据（{}）", db_path.display())));
                ui.label(meta("下载一份，或从备份里导入。"));
                ui.label(meta(
                    "也可以指定路径：llt-gui --db <path>，或环境变量 LLT_DB。",
                ));
            }
        }

        if let Some(remote) = &self.remote {
            ui.add_space(theme::GAP_XS);
            let text = match remote {
                Ok(remote) => {
                    let version = remote.data_version.as_deref().unwrap_or("未知");
                    let decision = lltdata::decide(self.local.as_ref(), Some(remote), self.force);
                    match decision {
                        Decision::UpToDate => format!("线上数据 {version} · 已是最新"),
                        Decision::Rebuild(reason) => format!("线上数据 {version} · {reason}"),
                        Decision::LocalNewer => format!("线上数据 {version} · 本地更新"),
                    }
                }
                Err(err) => format!("线上版本获取失败：{err}"),
            };
            ui.label(meta(text));
        }
    }

    fn task_ui(&mut self, ui: &mut Ui, db_path: &Path) {
        heading(ui, "更新");
        let busy = self.busy();

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            if button(ui, "检查更新").clicked() && !busy {
                self.start_check(ui.ctx());
            }
            let download_label = if self.local.is_some() {
                "下载并重建"
            } else {
                "下载数据"
            };
            if button(ui, download_label).clicked() && !busy {
                self.start_build(ui.ctx(), db_path, true);
            }
            ui.checkbox(&mut self.force, "强制重建");
        });

        if let Some(task) = &self.task {
            ui.add_space(theme::GAP_S);
            ui.label(body(task.kind.label()));
            if let Some((done, total)) = task.progress.download() {
                let ratio = total
                    .filter(|total| *total > 0)
                    .map(|total| done as f32 / total as f32)
                    .unwrap_or(0.0);
                let text = match total.filter(|total| *total > 0) {
                    Some(total) => format!(
                        "下载 {:.1} / {:.1} MB",
                        done as f64 / 1e6,
                        total as f64 / 1e6
                    ),
                    None => format!("下载 {:.1} MB", done as f64 / 1e6),
                };
                ui.add(egui::ProgressBar::new(ratio).text(text));
            }
            let notes = task.progress.notes();
            for note in notes.iter().rev().take(3).rev() {
                ui.label(meta(note.as_str()));
            }
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }

        if let Some(notice) = &self.notice {
            ui.add_space(theme::GAP_XS);
            let color = if notice.ok {
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().error_fg_color
            };
            ui.label(meta(notice.text.as_str()).color(color));
        }
    }

    fn backup_ui(&mut self, ui: &mut Ui, db_path: &Path, outcome: &mut DataOutcome) {
        heading(ui, "备份");
        let busy = self.busy();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            if button(ui, "导出备份").clicked() && !busy {
                self.start_export(ui.ctx(), db_path);
            }
            if button(ui, "导入备份").clicked() && !busy {
                self.scan_backups();
                if self.backups.is_empty() {
                    self.notice = Some(Notice {
                        ok: false,
                        text: format!("{} 下没有可导入的 .sqlite 备份", self.dirs.import.display()),
                    });
                }
            }
        });
        ui.label(meta(format!("导入目录：{}", self.dirs.import.display())));

        if let Some(path) = self.pending_import.clone() {
            ui.add_space(theme::GAP_S);
            ui.label(body(format!(
                "确认导入 {}？当前库会先备份一份。",
                file_name(&path)
            )));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_S;
                if button(ui, "确认导入").clicked() && !busy {
                    self.start_import(ui.ctx(), db_path, path.clone());
                    self.pending_import = None;
                }
                if button(ui, "取消").clicked() {
                    self.pending_import = None;
                }
            });
        }

        // 让调用方知道库变了（导入成功时由 poll_task 设置，这里转发一次）。
        let _ = outcome;
    }

    fn backup_list_ui(&mut self, ui: &mut Ui, _db_path: &Path, _outcome: &mut DataOutcome) {
        let busy = self.busy();
        ui.add_space(theme::GAP_XS);
        let mut pick: Option<PathBuf> = None;
        for entry in &self.backups {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_S;
                let version = entry
                    .data_version
                    .as_deref()
                    .map(|version| format!("数据 {version}"))
                    .unwrap_or_else(|| "版本未知".to_owned());
                ui.label(body(format!(
                    "{} · {} · {version}",
                    file_name(&entry.path),
                    size_text(entry.bytes)
                )));
                if entry.importable {
                    if button(ui, "导入").clicked() && !busy {
                        pick = Some(entry.path.clone());
                    }
                } else if let Some(problem) = &entry.problem {
                    ui.label(meta(problem.as_str()).weak());
                }
            });
        }
        if let Some(path) = pick {
            self.pending_import = Some(path);
        }
    }

    fn notes_ui(&mut self, ui: &mut Ui) {
        if self.backups.is_empty() && self.notice.is_none() && self.task.is_none() {
            return;
        }
        heading(ui, "说明");
        ui.label(meta(format!(
            "导出的备份写在 {}；把它拷回这里就能导入。",
            self.dirs.export.display()
        )));
        ui.label(meta(
            "下载会占用约 90 MB 临时空间（2.3 MB 数据包 + 解包 + 40 MB 新库）。",
        ));
    }

    // ------------------------------------------------------------ 动作

    fn start_check(&mut self, ctx: &egui::Context) {
        let progress = Arc::new(TaskProgress::default());
        let ver_url = lltdata::DEFAULT_VERSION_URL.to_owned();
        let progress_for_thread = Arc::clone(&progress);
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            progress_for_thread.note("正在查询线上版本…");
            let result = lltdata::update::fetch(&ver_url, Duration::from_secs(20))
                .map(TaskDone::Remote)
                .map_err(|err| format!("{err:#}"));
            let _ = ctx; // 线程不需要催帧：界面轮询
            let _ = tx.send(result);
        });
        self.task = Some(Task {
            kind: TaskKind::Check,
            progress,
            result: rx,
        });
    }

    fn start_build(&mut self, ctx: &egui::Context, db_path: &Path, check_version: bool) {
        let Some(dir) = db_path.parent().map(Path::to_path_buf) else {
            self.notice = Some(Notice {
                ok: false,
                text: format!("数据库路径没有父目录：{}", db_path.display()),
            });
            return;
        };
        let progress = Arc::new(TaskProgress::default());
        let opts = Options {
            input: None,
            out_dir: dir,
            url: None,
            ver_url: lltdata::DEFAULT_VERSION_URL.to_owned(),
            force: self.force,
            csv: false,
            check_version,
            temp_dir: Some(self.dirs.temp.clone()),
            keep_temp: false,
        };
        let progress_for_thread = Arc::clone(&progress);
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = lltdata::build(&opts, progress_for_thread.as_ref())
                .map(|report| {
                    TaskDone::Rebuilt(match report.skipped {
                        Some(reason) => reason,
                        None => format!(
                            "重建完成：数据 {} · 车站 {} · 车次 {}",
                            report.data_version.as_deref().unwrap_or("未知"),
                            report.stats.stations,
                            report.stats.trains
                        ),
                    })
                })
                .map_err(|err| format!("{err:#}"));
            let _ = ctx;
            let _ = tx.send(result);
        });
        self.task = Some(Task {
            kind: TaskKind::Build,
            progress,
            result: rx,
        });
    }

    fn start_export(&mut self, ctx: &egui::Context, db_path: &Path) {
        let source = db_path.to_path_buf();
        let target_dir = self.dirs.export.clone();
        let version = self
            .local
            .as_ref()
            .and_then(|state| state.data_version.clone());
        let progress = Arc::new(TaskProgress::default());
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = export_backup(&source, &target_dir, version.as_deref())
                .map(TaskDone::Exported)
                .map_err(|err| format!("{err:#}"));
            let _ = ctx;
            let _ = tx.send(result);
        });
        self.task = Some(Task {
            kind: TaskKind::Export,
            progress,
            result: rx,
        });
    }

    fn start_import(&mut self, ctx: &egui::Context, db_path: &Path, source: PathBuf) {
        let target = db_path.to_path_buf();
        let backup_dir = self.dirs.export.clone();
        let progress = Arc::new(TaskProgress::default());
        let ctx = ctx.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = import_backup(&source, &target, &backup_dir)
                .map(TaskDone::Rebuilt)
                .map_err(|err| format!("{err:#}"));
            let _ = ctx;
            let _ = tx.send(result);
        });
        self.task = Some(Task {
            kind: TaskKind::Import,
            progress,
            result: rx,
        });
    }

    /// 取走后台任务的结果。
    fn poll_task(&mut self, db_path: &Path, outcome: &mut DataOutcome) {
        let Some(task) = &self.task else {
            return;
        };
        match task.result.try_recv() {
            Ok(result) => {
                let kind = task.kind;
                self.task = None;
                match result {
                    Ok(done) => {
                        self.notice = Some(Notice {
                            ok: true,
                            text: match &done {
                                TaskDone::Remote(remote) => format!(
                                    "线上数据 {}",
                                    remote.data_version.as_deref().unwrap_or("未知")
                                ),
                                TaskDone::Rebuilt(text) => text.clone(),
                                TaskDone::Exported(path) => {
                                    format!("已导出到 {}", path.display())
                                }
                            },
                        });
                        match done {
                            TaskDone::Remote(remote) => self.remote = Some(Ok(remote)),
                            TaskDone::Rebuilt(text) => {
                                // 「已是最新」不会改库，但重开一次也无害且状态更简单。
                                if kind == TaskKind::Import || !text.starts_with("已是最新") {
                                    outcome.reload_db = true;
                                }
                            }
                            TaskDone::Exported(_) => {}
                        }
                    }
                    Err(err) => {
                        self.notice = Some(Notice {
                            ok: false,
                            text: err,
                        });
                    }
                }
                self.refresh(db_path);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.task = None;
                self.notice = Some(Notice {
                    ok: false,
                    text: "后台任务意外结束".to_owned(),
                });
                self.refresh(db_path);
            }
        }
    }

    fn scan_backups(&mut self) {
        self.scanned_at = Some(Instant::now());
        let mut entries = Vec::new();
        let Ok(dir) = std::fs::read_dir(&self.dirs.import) else {
            self.backups = entries;
            return;
        };
        for item in dir.flatten() {
            let path = item.path();
            if !path.is_file() || !is_sqlite(&path) {
                continue;
            }
            let bytes = item.metadata().map(|meta| meta.len()).unwrap_or(0);
            let state = LocalState::read(path.clone());
            let mut problem = None;
            if !state.usable() {
                problem = Some("不是本工具的库".to_owned());
            } else if !state.schema_current(lltdata::export::SCHEMA_VERSION) {
                problem = Some(format!(
                    "结构 v{}，需要 v{}",
                    state
                        .schema_version
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "?".to_owned()),
                    lltdata::export::SCHEMA_VERSION
                ));
            }
            entries.push(BackupEntry {
                path,
                bytes,
                data_version: state.data_version.clone(),
                importable: problem.is_none(),
                problem,
            });
        }
        // 新的在前：用户刚导出的那份就在顶上。
        entries.sort_by(|a, b| b.path.file_name().cmp(&a.path.file_name()));
        self.backups = entries;
    }
}

// ---------------------------------------------------------------- 文件动作

/// 把当前库复制到导出目录，文件名带数据版本与时间。
fn export_backup(
    source: &Path,
    target_dir: &Path,
    data_version: Option<&str>,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    if !source.is_file() {
        anyhow::bail!("当前没有可导出的数据库：{}", source.display());
    }
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("无法创建导出目录 {}", target_dir.display()))?;
    let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M").to_string();
    let version = data_version.unwrap_or("noversion");
    let name = format!("llt-timetable-{version}-{stamp}.sqlite");
    let target = target_dir.join(name);
    let tmp = target.with_extension("sqlite.tmp");
    std::fs::copy(source, &tmp).with_context(|| format!("无法写入 {}", tmp.display()))?;
    std::fs::rename(&tmp, &target).with_context(|| format!("无法写入 {}", target.display()))?;
    Ok(target)
}

/// 导入一份备份：先校验，再把当前库另存一份，最后原子替换。
fn import_backup(source: &Path, target: &Path, backup_dir: &Path) -> anyhow::Result<String> {
    use anyhow::Context;
    let state = LocalState::read(source.to_path_buf());
    if !state.usable() {
        anyhow::bail!(
            "{} 不是可用的备份：{}",
            file_name(source),
            state.problems.join("；")
        );
    }
    if !state.schema_current(lltdata::export::SCHEMA_VERSION) {
        anyhow::bail!(
            "备份结构版本是 {}，当前工具需要 v{}，请重新导出一份",
            state
                .schema_version
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".to_owned()),
            lltdata::export::SCHEMA_VERSION
        );
    }

    if target.is_file() {
        export_backup(target, backup_dir, state.data_version.as_deref())
            .context("导入前备份当前库失败")?;
    }

    let tmp = target.with_extension("sqlite.tmp");
    std::fs::copy(source, &tmp).with_context(|| format!("无法写入 {}", tmp.display()))?;
    std::fs::rename(&tmp, target).with_context(|| format!("无法写入 {}", target.display()))?;
    Ok(format!(
        "已导入 {}（数据 {}）",
        file_name(source),
        state.data_version.as_deref().unwrap_or("未知")
    ))
}

fn is_sqlite(path: &Path) -> bool {
    matches!(
        path.extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase()),
        Some(ext) if ext == "sqlite" || ext == "db" || ext == "sqlite3"
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{:.0} KB", bytes as f64 / 1e3)
    }
}

// ---------------------------------------------------------------- 小控件

fn heading(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(theme::FONT_SECTION)
            .family(crate::fonts::bold_family()),
    );
    ui.add_space(theme::GAP_XS);
}

fn body(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(theme::FONT_BODY)
}

fn meta(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(theme::FONT_META)
}

fn button(ui: &mut Ui, text: &str) -> egui::Response {
    let height = theme::control_height();
    ui.add(
        egui::Button::new(body(text))
            .min_size(egui::vec2(0.0, height))
            .corner_radius(6),
    )
}

/// 数据页需要知道配置里有没有 `data` 这个标签页——配置模块是它的上游，
/// 这里只做一次编译期检查，防止有人改了 `Tab` 忘了数据页。
#[allow(dead_code)]
const _: () = {
    let _ = config::Tab::Data;
};
