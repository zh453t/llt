//! 路路通（com.lltskb.lltskb）离线时刻表工具。
//!
//! - `llt check`：比对本地库与线上版本（`android.ver`）；
//! - `llt build`：下载 / 解包 / 解码 `an.db`，生成 sqlite 或 csv；
//! - `llt query`：查询已生成的 sqlite（车次 / 停站 / 车站 / 站到站）；
//! - 不带子命令时打印帮助。
//!
//! 格式说明见 `docs/llt-format-reverse-engineering.md` 与 `out/timetable/README.md`。

mod archive;
mod build;
mod check;
mod cli;
mod dbpath;
mod decode;
mod download;
mod duty;
mod export;
mod extras;
mod format;
mod query;
mod routes;
mod samecity;
mod spdata;
mod table;
mod update;

use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        // 不带子命令时打印帮助，而不是报错。
        let mut cmd = Cli::command();
        let _ = cmd.print_help();
        println!();
        return;
    };

    let result = match command {
        Command::Build(args) => build::run(&args),
        Command::Query(args) => query::run(&args).map(|()| check::EXIT_OK),
        Command::Check(args) => check::run(&args),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // `{:#}` 展开 anyhow 的上下文链，便于定位是哪一步失败。
            eprintln!("错误：{err:#}");
            std::process::exit(check::EXIT_ERROR);
        }
    }
}
