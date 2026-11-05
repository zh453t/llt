# llt — 路路通离线时刻表

Rust workspace，把路路通（`com.lltskb.lltskb`）的离线数据包 `an.db` 变成可查询的 SQLite / CSV，
并提供桌面查询界面与命令行工具。

| 路径 | 包名 | 作用 |
|---|---|---|
| `cli/` | `llt` | `build`：下载/解码 `an.db` → SQLite/CSV；`query`：查询数据库 |
| `gui/` | `llt-gui` | egui 桌面查询界面（车次 / 车站 / 站站），只读 SQLite |
| `crates/lltdb/` | `lltdb` | 两个应用共用的只读查询层（含经由 / 交路 / 餐车，只依赖 `rusqlite` 与 `jiff`） |
| `tools/` | — | Python 参考实现 `tools/decode_llt.py` |
| `out/` | — | 已解码的数据；`out/timetable/README.md` 是格式、表结构与数据版本说明 |
| `docs/` | — | 逆向报告与二进制格式说明 |

依赖方向是 `llt-gui → lltdb`、`llt → lltdb`，`lltdb` 不依赖 GUI 或网络栈；
GUI 与 CLI 都是单文件可执行程序，静态链接，没有运行时依赖或进程间调用。

## 构建与测试

```bash
cargo build --workspace --release   # -> target/release/llt.exe、target/release/llt-gui.exe
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## 数据

```bash
llt check                      # 比对本地库与线上版本（android.ver），只看不动
llt build                      # 已是最新则跳过；否则下载 an.db、解包解码，生成 llt-timetable.sqlite
llt build --csv -o out         # 只导出 stations/trains/stops.csv
llt query train G1             # 查询（默认今天开行，附交路与经由）
llt query od 北京 上海 --json  # 站到站直达，JSON 输出
```

不带子命令时打印帮助。用法、参数、数据版本写入方式见 [`cli/README.md`](cli/README.md)，
SQLite 表结构见 [`out/timetable/README.md`](out/timetable/README.md)。
