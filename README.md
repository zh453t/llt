# llt — 路路通离线时刻表

Rust workspace，把路路通（`com.lltskb.lltskb`）的离线数据包 `an.db` 变成可查询的 SQLite / CSV，
并提供桌面查询界面与命令行工具。

| 路径 | 包名 | 作用 |
|---|---|---|
| `cli/` | `llt` | `build`：下载/解码 `an.db` → SQLite/CSV；`query`：查询数据库 |
| `gui/` | `llt-gui` | egui 桌面查询界面（车次 / 车站 / 站站），只读 SQLite；另提供 lib `llt_gui` 给 Android 入口复用 |
| `crates/lltdb/` | `lltdb` | 两个应用共用的只读查询层（含经由 / 交路 / 餐车，只依赖 `rusqlite` 与 `jiff`） |
| `android/` | `llt-gui-android` | Android 入口（cdylib）与打包配置，独立包、不参与 workspace，见 [`docs/android.md`](docs/android.md) |
| `tools/` | — | Python 参考实现 `tools/decode_llt.py` |
| `out/` | — | 已解码的数据；`out/timetable/README.md` 是格式、表结构与数据版本说明 |
| `docs/` | — | 逆向报告与二进制格式说明 |

依赖方向是 `llt-gui → lltdb`、`llt → lltdb`，`lltdb` 不依赖 GUI 或网络栈；
GUI 与 CLI 都是单文件可执行程序，静态链接，没有运行时依赖或进程间调用。
`android/` 复用 `gui/` 里的 `llt_gui` 库，把同一套界面装进 APK。

## 构建与测试

```bash
cargo build --workspace --release   # -> target/release/llt.exe、target/release/llt-gui.exe
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Android 的 APK 单独构建（不进 workspace，需要 NDK，见 [`docs/android.md`](docs/android.md)）：

```powershell
pwsh -File android/build.ps1 -Release   # -> android/target/release/apk/llt-gui.apk
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

## 界面配置

`llt-gui` 把「下次打开还想保留」的少量状态写在**可执行文件旁**的 `llt-gui.json`
（绿色版：整个目录复制走，配置跟数据一起过去）：

| 配置项 | 界面位置 | 默认 |
|---|---|---|
| `ui.tab` | 当前标签页（`od` / `train` / `station`） | `od`（站站查询） |
| `train.query` | 按车次：车次号 | 空 |
| `station.query` / `station.fuzzy` | 按车站：车站名、「模糊」 | 空 / 关 |
| `od.from` / `od.to` / `od.fuzzy` | 站站查询：出发、到达、「模糊」 | 空 / 关 |
| `detail.via_stations_only` | 车次详情·经由：「只看车站」 | 开 |

- 写入时机：停止输入 0.5 秒后（先写 `.tmp` 再改名覆盖），关窗时再兜底一次。
- 读取失败（JSON 坏了）回退默认值，并把原文件改名 `llt-gui.json.bad`，界面顶部给一句提示。
- 只读目录（`Program Files`、只读共享盘）写不进去时不阻断使用，可用 `--config <path>`
  或环境变量 `LLT_CONFIG` 指到可写位置。
- 查询日期、选中车次、同城车站勾选等会话现场不落盘。

完整方案与取舍见 [`docs/gui-config.md`](docs/gui-config.md)。
