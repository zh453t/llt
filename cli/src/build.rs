//! `llt build`：下载 / 解包 / 解码 `an.db`，导出 SQLite 或 CSV。
//!
//! （这是命令实现模块，不是 Cargo 的 build script。）

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::archive;
use crate::check::{self, Decision};
use crate::cli::BuildArgs;
use crate::dbpath::LocalState;
use crate::decode;
use crate::download;
use crate::export;
use crate::extras::Extras;
use crate::update::{self, RemoteVersion};

pub fn run(args: &BuildArgs) -> Result<i32> {
    // 先做参数校验，尽早失败，避免白白建临时目录。
    let input = match &args.input {
        Some(path) if !path.exists() => bail!("输入路径不存在：{}", path.display()),
        Some(path) => Some(path.clone()),
        None => None,
    };

    let out_dir = match &args.output {
        Some(dir) => dir.clone(),
        None => default_output_dir()?,
    };

    // 未显式给输入时：先问线上版本，已是最新就直接跳过（省掉 2.3 MB 下载与重建）。
    let mut remote: Option<RemoteVersion> = None;
    if input.is_none() && !args.no_version_check {
        remote = match update::fetch(&args.ver_url, std::time::Duration::from_secs(20)) {
            Ok(version) => {
                if !args.quiet {
                    eprintln!(
                        "线上数据版本：{}",
                        check::describe_version(version.data_version.as_deref())
                    );
                }
                Some(version)
            }
            Err(err) => {
                // 接口坏了不能当成“最新”，只提示后照常下载。
                eprintln!("提示：无法获取线上版本（{err}），继续下载构建");
                None
            }
        };

        // sqlite 模式看库本身；--csv 模式看目录里的 meta.json。
        let local = if args.csv {
            LocalState::inspect_csv(&out_dir)
        } else {
            LocalState::inspect(Some(&out_dir.join(crate::export::SQLITE_FILE)), true)
        };
        match check::decide(local.as_ref(), remote.as_ref(), args.force) {
            Decision::UpToDate => {
                if let Some(state) = &local {
                    println!(
                        "已是最新：数据 {} · 构建于 {}",
                        check::describe_version(state.data_version.as_deref()),
                        state.built_at.as_deref().unwrap_or("未知"),
                    );
                    println!("{}", state.path.display());
                }
                if !args.quiet {
                    eprintln!("提示：需要重建请加 --force");
                }
                return Ok(check::EXIT_OK);
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
                println!(
                    "本地数据 {local_version} 比线上 {remote_version} 新，已保留现有库；需要覆盖请加 --force"
                );
                if let Some(state) = &local {
                    println!("{}", state.path.display());
                }
                return Ok(check::EXIT_OK);
            }
            Decision::Rebuild(reason) => {
                if !args.quiet {
                    eprintln!("将重新构建：{reason}");
                }
            }
        }
    }

    fs::create_dir_all(&out_dir)
        .with_context(|| format!("无法创建输出目录 {}", out_dir.display()))?;

    // 所有下载 / 解包操作都在临时目录里完成，结束时（含出错返回）自动清理。
    let tmp = tempfile::Builder::new()
        .prefix("llt-timetable-")
        .tempdir()
        .context("无法创建临时目录")?;
    if !args.quiet {
        eprintln!("临时目录：{}", tmp.path().display());
    }

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
            // 显式 --url 优先；否则用版本接口给出的镜像列表依次尝试。
            let mirrors = remote
                .as_ref()
                .map(|version| version.mirrors(args.url.as_deref()))
                .unwrap_or_else(|| {
                    vec![
                        args.url
                            .clone()
                            .unwrap_or_else(|| crate::cli::DEFAULT_DATA_URL.to_owned()),
                    ]
                });
            let (used, info) = download::download_first(&mirrors, &dest, args.quiet)?;
            meta.source = used;
            meta.archive_bytes = Some(info.bytes);
            meta.source_last_modified = info.last_modified;
            meta.etag = info.etag;
            dest
        }
    };

    let shards = archive::unpack(&input, tmp.path(), args.quiet)?;
    meta.data_version = shards.read_text("ver.txt");
    if let (Some(actual), Some(declared)) = (
        meta.data_version.as_deref(),
        meta.remote_data_version.as_deref(),
    ) && actual != declared
    {
        eprintln!(
            "提示：数据包实际版本 {actual} 与接口声明 {declared} 不一致（镜像可能尚未同步），以实际值为准"
        );
    }
    if !args.quiet {
        match (&meta.data_version, meta.data_date()) {
            (Some(version), Some(date)) => eprintln!("数据版本：{version}（{date}）"),
            (Some(version), None) => eprintln!("数据版本：{version}"),
            _ => {}
        }
    }

    let data = decode::load(&shards)?;

    // 经由 / 交路属于增强信息，缺失只提示，不影响主表。
    let extras = Extras::load(&shards, &data.train_codes, &data.stations);
    for note in data.notes.iter().chain(extras.notes.iter()) {
        eprintln!("提示：{note}");
    }
    println!(
        "车站 {}   车次 {}   停站 {}",
        data.stations.len(),
        data.trains.len(),
        data.stop_count()
    );
    if !extras.vias.is_empty() || extras.duty_count() > 0 {
        println!(
            "经由 {} 趟 / {} 点   交路 {} 条   餐车 {} 条",
            extras.vias.len(),
            extras.route_point_count(),
            extras.duty_count(),
            extras.dining_count(),
        );
    }
    if extras.same_city_group_count() > 0 {
        println!(
            "同城 {} 组 · 覆盖 {} 站   电报码别名 {} 条",
            extras.same_city_group_count(),
            extras.same_city_covered(),
            extras.tele_alias_count(),
        );
    }

    if args.csv {
        for path in export::write_csvs(&out_dir, &data, &extras, &meta)? {
            println!("csv     {}", describe(&path));
        }
    } else {
        let db = out_dir.join(export::SQLITE_FILE);
        export::write_sqlite(&db, &data, &extras, &meta)?;
        println!("sqlite  {}", describe(&db));
    }

    if args.keep_temp {
        let kept = tmp.keep();
        eprintln!("临时目录已保留：{}", kept.display());
    }
    Ok(check::EXIT_OK)
}

/// 输出文件的一行摘要，例如 `llt-timetable.sqlite (6.9 MB)`。
fn describe(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(meta) => format!("{} ({:.1} MB)", path.display(), meta.len() as f64 / 1e6),
        Err(_) => path.display().to_string(),
    }
}

/// 未给 `-o` 时的默认输出目录：可执行文件自身所在目录。
fn default_output_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("无法定位可执行文件路径")?;
    Ok(exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".")))
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

    #[test]
    fn default_output_dir_is_absolute() {
        let dir = default_output_dir().unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
    }
}
