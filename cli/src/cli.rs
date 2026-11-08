//! 命令行参数。
//!
//! 顶层只有两个子命令：`build`（生成数据库）与 `query`（查询数据库）。
//! 不带子命令时不报错，直接打印帮助（见 `main.rs`）。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// 官方版本接口（`version.data` 即数据版本，见 `tools/check_update.py`）。
///
/// 数据包地址在 `lltdata` 里（`DEFAULT_DATA_URL`），CLI 只在帮助文本里提到它。
pub const DEFAULT_VER_URL: &str = lltdata::DEFAULT_VERSION_URL;

const EXAMPLES: &str = "\
示例：
  llt check                                     比对本地库与线上版本（退出码见 README）
  llt build                                     下载最新数据，在程序目录生成 sqlite
  llt build --csv -o out                        下载并只导出 CSV
  llt build --force                             忽略“已是最新”，强制重建
  llt build -i an.db -o out                     用本地 an.db（zip）解码
  llt query train G1                            按车次查询（默认今天开行）
  llt query train 54 --all                      车次号 54 的全部前缀
  llt query stops G1                            完整停站表
  llt query station 北京 --date 2026-11-08      某站当日经停列车
  llt query od 北京 上海 --limit 0              站到站直达，不限条数
  llt query train G1 --json                     输出 JSON
";

#[derive(Debug, Parser)]
#[command(
    name = "llt",
    version,
    about = "路路通离线时刻表工具",
    long_about = "把路路通（com.lltskb.lltskb）的离线数据包 an.db 解码成 SQLite / CSV，并直接查询。\n\
                  an.db 实际是 zip 容器，内含 s.i、t.i、T0..T19.dat、S0..S9.dat 等自定义二进制分片。\n\
                  不带子命令时打印本帮助。",
    after_help = EXAMPLES
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 下载 / 解包 / 解码 an.db，生成 SQLite 或 CSV
    Build(BuildArgs),
    /// 查询已生成的 llt-timetable.sqlite
    Query(QueryArgs),
    /// 比对本地库与线上版本，判断是否需要更新
    Check(CheckArgs),
}

// ---------------------------------------------------------------- build

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// 输入数据包 an.db（zip 容器）或已解包的目录；省略时从网络下载
    #[arg(short, long, value_name = "FILE")]
    pub input: Option<PathBuf>,

    /// 输出目录，sqlite 或 csv 写入这里；默认是可执行文件所在目录
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// 只输出 CSV（主表 + 经由 / 交路 / 餐车），不生成 sqlite
    #[arg(long)]
    pub csv: bool,

    /// 数据包地址；省略时优先用版本接口给出的镜像
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,

    /// 版本接口地址（`version.data` 即数据版本）
    #[arg(long, value_name = "URL", default_value = DEFAULT_VER_URL)]
    pub ver_url: String,

    /// 忽略“已是最新”的判断，强制重新下载构建
    #[arg(long)]
    pub force: bool,

    /// 不做线上版本检查，直接下载构建（脚本 / 离线镜像场景）
    #[arg(long)]
    pub no_version_check: bool,

    /// 保留临时目录（便于排查问题），默认成功后删除
    #[arg(long)]
    pub keep_temp: bool,

    /// 不输出进度与提示信息
    #[arg(short, long)]
    pub quiet: bool,
}

// ---------------------------------------------------------------- check

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// 数据库路径（默认：$LLT_DB、可执行文件旁、当前目录、仓库 out/timetable）
    #[arg(long, value_name = "FILE")]
    pub db: Option<PathBuf>,

    /// 只认 --db 与 $LLT_DB，不做隐式搜索
    #[arg(long)]
    pub strict: bool,

    /// 版本接口地址
    #[arg(long, value_name = "URL", default_value = DEFAULT_VER_URL)]
    pub ver_url: String,

    /// 输出 JSON
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------- query

#[derive(Debug, Args)]
pub struct QueryArgs {
    #[command(subcommand)]
    pub command: QueryCommand,
}

#[derive(Debug, Subcommand)]
pub enum QueryCommand {
    /// 按车次查询（完整车次 G1、前缀 G1、车次号 54、斜杠车次 G97）
    Train(TrainArgs),

    /// 完整停站表
    Stops(StopsArgs),

    /// 某车站的经停列车
    Station(StationArgs),

    /// 站到站直达查询
    Od(OdArgs),
}

#[derive(Debug, Args)]
pub struct TrainArgs {
    /// 车次或车次号
    pub code: String,

    /// 按前缀列出全部匹配（默认：有精确车次/车次号时只显示精确匹配）
    #[arg(long)]
    pub prefix: bool,

    #[command(flatten)]
    pub opts: QueryOpts,
}

#[derive(Debug, Args)]
pub struct StopsArgs {
    /// 车次（重名时用 --index 指定）
    pub code: Option<String>,

    /// 直接按 train_index 指定，用于同车次号多区间的情况
    #[arg(long, value_name = "N")]
    pub index: Option<i64>,

    #[command(flatten)]
    pub base: DbOpts,
}

#[derive(Debug, Args)]
pub struct StationArgs {
    /// 站名，需与数据一致（如「北京」「北京南」）
    pub name: String,

    #[command(flatten)]
    pub opts: QueryOpts,
}

#[derive(Debug, Args)]
pub struct OdArgs {
    /// 出发站
    pub from: String,

    /// 到达站
    pub to: String,

    #[command(flatten)]
    pub opts: QueryOpts,
}

#[derive(Debug, Args)]
pub struct DbOpts {
    /// 数据库路径（默认：$LLT_DB、可执行文件旁、当前目录、仓库 out/timetable）
    #[arg(long, value_name = "FILE")]
    pub db: Option<PathBuf>,

    /// 只认 --db 与 $LLT_DB，不做隐式搜索（脚本用）
    #[arg(long)]
    pub strict: bool,

    /// 输出 JSON，便于脚本处理
    #[arg(long)]
    pub json: bool,

    /// 不打印“用的是哪个库”这类提示
    #[arg(short, long)]
    pub quiet: bool,
}

#[derive(Debug, Args)]
pub struct QueryOpts {
    #[command(flatten)]
    pub base: DbOpts,

    /// 查询日期（YYYY-MM-DD 或 YYYYMMDD），默认今天
    #[arg(long, value_name = "DATE")]
    pub date: Option<String>,

    /// 忽略日期，显示全部开行区间
    #[arg(long, conflicts_with = "date")]
    pub all: bool,

    /// 最多显示多少条，0 表示不限制
    #[arg(long, value_name = "N", default_value_t = 50)]
    pub limit: usize,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }
}
