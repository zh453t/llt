//! 构建：下载（或读本地 `an.db`）→ 解包 → 解码 → 写 SQLite / CSV。
//!
//! 命令行与界面走的是同一条路：`llt build` 只是把它包了一层参数解析与打印。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::archive;
use crate::check::{self, Decision};
use crate::decode;
use crate::download;
use crate::export;
use crate::extras::Extras;
use crate::local::LocalState;
use crate::progress::Progress;
use crate::update::{self, RemoteVersion};
use crate::{DEFAULT_DATA_URL, DEFAULT_VERSION_URL};

/// 一次构建的输入与开关。
#[derive(Debug, Clone)]
pub struct Options {
    /// 本地输入（`an.db` 或已解包目录）。`None` 表示联网下载。
    pub input: Option<PathBuf>,
    /// 输出目录：SQLite 写到这里，`csv` 模式下 CSV 也写到这里。
    pub out_dir: PathBuf,
    /// 显式指定的下载地址；`None` 用版本接口给的镜像列表。
    pub url: Option<String>,
    /// 版本接口地址。
    pub ver_url: String,
    /// 跳过版本比对，强制重建。
    pub force: bool,
    /// 写 CSV 而不是 SQLite。
    pub csv: bool,
    /// 是否先问线上版本（本地输入时无意义，自动忽略）。
    pub check_version: bool,
    /// 临时目录的父目录。Android 上必须给（没有可写的 `/tmp`）。
    pub temp_dir: Option<PathBuf>,
    /// 出错也保留临时目录，方便排查。
    pub keep_temp: bool,
}

impl Options {
    /// 联网构建到 `out_dir` 的最小配置。
    pub fn remote(out_dir: impl Into<PathBuf>) -> Self {
        Self {
            input: None,
            out_dir: out_dir.into(),
            url: None,
            ver_url: DEFAULT_VERSION_URL.to_owned(),
            force: false,
            csv: false,
            check_version: true,
            temp_dir: None,
            keep_temp: false,
        }
    }
}

/// 主表与附加信息的规模，界面「当前数据」栏用它，比读数据库快得多。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub stations: usize,
    pub trains: usize,
    pub stops: usize,
    pub vias: usize,
    pub via_points: usize,
    pub duties: usize,
    pub dining: usize,
    pub same_city_groups: usize,
    pub same_city_stations: usize,
    pub tele_aliases: usize,
}

/// 构建结果。
#[derive(Debug, Clone)]
pub struct Report {
    /// 产出的文件（`report.output` 指向 SQLite 或 CSV 目录）。
    pub output: PathBuf,
    /// 数据包版本（`ver.txt`）。
    pub data_version: Option<String>,
    /// 有值表示**没有重建**：已是最新 / 本地更新，内容是人话。
    pub skipped: Option<String>,
    pub stats: Stats,
    /// 没有致命影响、但值得告诉用户的提示（缺经由、版本与接口声明不一致等）。
    pub notes: Vec<String>,
    /// 线上版本（没查或查询失败时为 `None`）。
    pub remote: Option<RemoteVersion>,
}

impl Report {
    /// 是否真的重建了。
    pub fn rebuilt(&self) -> bool {
        self.skipped.is_none()
    }
}

/// 读输出目录里已有的库（SQLite 或 CSV）的状态，给界面显示用。
///
/// 找不到时返回 `None`；库坏了也会返回（`problems` 里写着原因）。
pub fn inspect_output(out_dir: &Path, csv: bool) -> Option<LocalState> {
    if csv {
        LocalState::inspect_csv(out_dir)
    } else {
        LocalState::inspect(Some(&out_dir.join(export::SQLITE_FILE)), true)
    }
}

/// 执行一次构建。
///
/// 已是最新（或本地比线上新）时**不报错**，返回 `skipped`；真正失败才返回 `Err`。
pub fn build(opts: &Options, progress: &dyn Progress) -> Result<Report> {
    // 参数校验放最前面，避免白建临时目录。
    let input = match &opts.input {
        Some(path) if !path.exists() => bail!("输入路径不存在：{}", path.display()),
        Some(path) => Some(path.clone()),
        None => None,
    };

    // 未显式给输入时：先问线上版本，已是最新就直接跳过（省掉 2.3 MB 下载与重建）。
    let mut remote: Option<RemoteVersion> = None;
    if input.is_none() && opts.check_version {
        remote = match update::fetch(&opts.ver_url, Duration::from_secs(20)) {
            Ok(version) => {
                progress.note(&format!(
                    "线上数据版本：{}",
                    check::describe_version(version.data_version.as_deref())
                ));
                Some(version)
            }
            Err(err) => {
                // 接口坏了不能当成“最新”，只提示后照常下载。
                progress.note(&format!("无法获取线上版本（{err}），继续下载构建"));
                None
            }
        };

        let local = inspect_output(&opts.out_dir, opts.csv);
        match check::decide(local.as_ref(), remote.as_ref(), opts.force) {
            Decision::UpToDate => {
                let detail = local
                    .as_ref()
                    .map(|state| {
                        format!(
                            "已是最新：数据 {} · 构建于 {}",
                            check::describe_version(state.data_version.as_deref()),
                            state.built_at.as_deref().unwrap_or("未知"),
                        )
                    })
                    .unwrap_or_else(|| "已是最新".to_owned());
                return Ok(Report {
                    output: local.map(|state| state.path).unwrap_or_default(),
                    data_version: remote
                        .as_ref()
                        .and_then(|version| version.data_version.clone()),
                    skipped: Some(detail),
                    stats: Stats::default(),
                    notes: Vec::new(),
                    remote,
                });
            }
            Decision::LocalNewer => {
                let local_version = local
                    .as_ref()
                    .and_then(|state| state.data_version.clone())
                    .unwrap_or_else(|| "?".to_owned());
                let remote_version = remote
                    .as_ref()
                    .and_then(|version| version.data_version.clone())
                    .unwrap_or_else(|| "?".to_owned());
                return Ok(Report {
                    output: local.map(|state| state.path).unwrap_or_default(),
                    data_version: Some(local_version.clone()),
                    skipped: Some(format!(
                        "本地数据 {local_version} 比线上 {remote_version} 新，已保留现有库"
                    )),
                    stats: Stats::default(),
                    notes: Vec::new(),
                    remote,
                });
            }
            Decision::Rebuild(reason) => progress.note(&format!("将重新构建：{reason}")),
        }
    }

    fs::create_dir_all(&opts.out_dir)
        .with_context(|| format!("无法创建输出目录 {}", opts.out_dir.display()))?;

    // 所有下载 / 解包操作都在临时目录里完成，结束时（含出错返回）自动清理。
    let tmp = temp_dir(opts)?;
    progress.note(&format!("临时目录：{}", tmp.path().display()));

    let mut meta = export::Meta {
        built_at: now_rfc3339(),
        remote_data_version: remote
            .as_ref()
            .and_then(|version| version.data_version.clone()),
        remote_app_version: remote
            .as_ref()
            .and_then(|version| version.app_version.clone()),
        version_source: remote.as_ref().map(|version| version.source_url.clone()),
        ..Default::default()
    };

    let mut notes = Vec::new();
    let input = match input {
        Some(path) => {
            meta.source = path.display().to_string();
            // 目录输入（已解包）没有“包大小”这个概念，只在文件输入时记录。
            meta.archive_bytes = fs::metadata(&path)
                .ok()
                .filter(|info| info.is_file())
                .map(|info| info.len());
            path
        }
        None => {
            let dest = tmp.path().join("an.db");
            // 显式 url 优先；否则用版本接口给出的镜像列表依次尝试。
            let mirrors = remote
                .as_ref()
                .map(|version| version.mirrors(opts.url.as_deref()))
                .unwrap_or_else(|| {
                    vec![
                        opts.url
                            .clone()
                            .unwrap_or_else(|| DEFAULT_DATA_URL.to_owned()),
                    ]
                });
            let (used, info) = download::download_first(&mirrors, &dest, progress)?;
            meta.source = used;
            meta.archive_bytes = Some(info.bytes);
            meta.source_last_modified = info.last_modified;
            meta.etag = info.etag;
            dest
        }
    };

    let shards = archive::unpack(&input, tmp.path(), progress)?;
    meta.data_version = shards.read_text("ver.txt");
    if let (Some(actual), Some(declared)) = (
        meta.data_version.as_deref(),
        meta.remote_data_version.as_deref(),
    ) && actual != declared
    {
        notes.push(format!(
            "数据包实际版本 {actual} 与接口声明 {declared} 不一致（镜像可能尚未同步），以实际值为准"
        ));
    }
    if let Some(date) = meta.data_date() {
        progress.note(&format!(
            "数据版本：{}（{date}）",
            check::describe_version(meta.data_version.as_deref())
        ));
    } else {
        progress.note(&format!(
            "数据版本：{}",
            check::describe_version(meta.data_version.as_deref())
        ));
    }

    progress.note("解码…");
    let data = decode::load(&shards)?;

    // 经由 / 交路属于增强信息，缺失只提示，不影响主表。
    let extras = Extras::load(&shards, &data.train_codes, &data.stations);
    notes.extend(data.notes.iter().cloned());
    notes.extend(extras.notes.iter().cloned());

    let stats = Stats {
        stations: data.stations.len(),
        trains: data.trains.len(),
        stops: data.stop_count(),
        vias: extras.vias.len(),
        via_points: extras.route_point_count(),
        duties: extras.duty_count(),
        dining: extras.dining_count(),
        same_city_groups: extras.same_city_group_count(),
        same_city_stations: extras.same_city_covered(),
        tele_aliases: extras.tele_alias_count(),
    };

    let output = if opts.csv {
        progress.note("写出 CSV…");
        let _ = export::write_csvs(&opts.out_dir, &data, &extras, &meta)?;
        opts.out_dir.clone()
    } else {
        progress.note("写出 SQLite…");
        let db = opts.out_dir.join(export::SQLITE_FILE);
        // `write_sqlite` 自己写临时文件再改名：正在读旧库的进程不会读到半个库。
        export::write_sqlite(&db, &data, &extras, &meta)?;
        db
    };

    if opts.keep_temp {
        let kept = tmp.keep();
        progress.note(&format!("临时目录已保留：{}", kept.display()));
    }

    Ok(Report {
        output,
        data_version: meta.data_version.clone(),
        skipped: None,
        stats,
        notes,
        remote,
    })
}

/// 临时目录：优先用调用方给的父目录（Android 没有可写 `/tmp`）。
fn temp_dir(opts: &Options) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("llt-timetable-");
    match &opts.temp_dir {
        Some(base) => {
            fs::create_dir_all(base)
                .with_context(|| format!("无法创建临时目录 {}", base.display()))?;
            builder
                .tempdir_in(base)
                .with_context(|| format!("无法在 {} 下创建临时目录", base.display()))
        }
        None => builder.tempdir().context("无法创建临时目录"),
    }
}

/// 当前时间（RFC3339，UTC，精确到秒），写进 `meta.built_at`。
fn now_rfc3339() -> String {
    let now = jiff::Timestamp::now();
    match now.round(jiff::Unit::Second) {
        Ok(rounded) => rounded.to_string(),
        Err(_) => now.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::Silent;

    #[test]
    fn missing_input_fails_before_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("never-created");
        let opts = Options {
            input: Some(dir.path().join("nope.db")),
            out_dir: out.clone(),
            ..Options::remote(&out)
        };
        let err = build(&opts, &Silent).unwrap_err().to_string();
        assert!(err.contains("输入路径不存在"), "{err}");
        assert!(!out.exists(), "不该提前建输出目录");
    }

    /// 临时目录必须落在调用方给的位置：Android 上 `std::env::temp_dir()` 不可写。
    #[test]
    fn temp_dir_honors_parent() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let opts = Options {
            temp_dir: Some(base.clone()),
            ..Options::remote(dir.path())
        };
        let tmp = temp_dir(&opts).unwrap();
        assert_eq!(tmp.path().parent(), Some(base.as_path()));
    }
}
