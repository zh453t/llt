//! `llt build`：下载 / 解包 / 解码 `an.db`，导出 SQLite 或 CSV。
//!
//! 真正的实现在 `lltdata::build`（界面也调它）；这里只做参数翻译与打印。
//! （这是命令实现模块，不是 Cargo 的 build script。）

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lltdata::{Options, Progress, Report};

use crate::cli::BuildArgs;

pub fn run(args: &BuildArgs) -> Result<i32> {
    let opts = options(args)?;
    let progress = CliProgress { quiet: args.quiet };
    let report = lltdata::build(&opts, &progress)?;
    print_report(&report);
    Ok(crate::check::EXIT_OK)
}

/// 命令行参数 → 管线参数。
fn options(args: &BuildArgs) -> Result<Options> {
    Ok(Options {
        input: args.input.clone(),
        out_dir: match &args.output {
            Some(dir) => dir.clone(),
            None => default_output_dir()?,
        },
        url: args.url.clone(),
        ver_url: args.ver_url.clone(),
        force: args.force,
        csv: args.csv,
        check_version: !args.no_version_check,
        // 命令行的临时目录用系统默认（桌面与 CLI 一直是这样）。
        temp_dir: None,
        keep_temp: args.keep_temp,
    })
}

/// 把管线的结果打成现在这套输出：一行摘要 + 每类信息的计数。
fn print_report(report: &Report) {
    if let Some(skipped) = &report.skipped {
        println!("{skipped}");
        println!("{}", report.output.display());
        return;
    }
    let stats = &report.stats;
    println!(
        "车站 {}   车次 {}   停站 {}",
        stats.stations, stats.trains, stats.stops
    );
    if stats.vias > 0 || stats.duties > 0 {
        println!(
            "经由 {} 趟 / {} 点   交路 {} 条   餐车 {} 条",
            stats.vias, stats.via_points, stats.duties, stats.dining
        );
    }
    if stats.same_city_groups > 0 {
        println!(
            "同城 {} 组 · 覆盖 {} 站   电报码别名 {} 条",
            stats.same_city_groups, stats.same_city_stations, stats.tele_aliases
        );
    }
    if stats.platform_stops > 0 || stats.train_changes > 0 {
        println!(
            "站台 {} 站   车次变化 {} 趟",
            stats.platform_stops, stats.train_changes
        );
    }
    println!("sqlite  {}", describe(&report.output));
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

/// 命令行进度：提示走 stderr，下载进度用 `\r` 刷新同一行。
struct CliProgress {
    quiet: bool,
}

impl Progress for CliProgress {
    fn note(&self, text: &str) {
        if !self.quiet {
            eprintln!("{text}");
        }
    }

    fn download(&self, done: u64, total: Option<u64>) {
        if self.quiet {
            return;
        }
        match total.filter(|total| *total > 0) {
            Some(total) => eprint!(
                "\r  已下载 {:.1} / {:.1} MB（{:.0}%）",
                done as f64 / 1e6,
                total as f64 / 1e6,
                (done as f64 / total as f64 * 100.0).min(100.0)
            ),
            None => eprint!("\r  已下载 {:.1} MB", done as f64 / 1e6),
        }
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
