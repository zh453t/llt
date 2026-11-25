# llt — 路路通离线时刻表 CLI

用 Rust 写的命令行工具：

- `llt build`：把路路通（`com.lltskb.lltskb`）的离线数据包 `an.db` 变成
  `llt-timetable.sqlite` 或三个 CSV；
- `llt query`：直接查询已生成的 SQLite（车次 / 停站 / 车站 / 站到站），
  `query train` / `query stops` 会同时显示交路、编组、担当与经由，
  `query stops` 另带逐站「显示车次」（`xw.dat` 车次变化）与站台 / 出站口（`plat.dat`）；
- 不带子命令时打印帮助。

`an.db` 的名字有误导：它其实是个 **zip 容器**（未加密、deflate 压缩），里面是
`s.i`、`t.i`、`T0..T19.dat`、`S0..S9.dat` 等自定义二进制分片。格式说明见
[`../docs/llt-format-reverse-engineering.md`](../docs/llt-format-reverse-engineering.md)，
产物表结构见 [`../out/timetable/README.md`](../out/timetable/README.md)。
`build` 是 [`../tools/decode_llt.py`](../tools/decode_llt.py) 的 Rust 版本，输出与之一致；
`query` 与 GUI 共用 [`../crates/lltdb`](../crates/lltdb) 查询层，语义完全相同。

## 用法

```bash
# 先看是否需要更新（只读，退出码：0 最新 / 3 有更新 / 4 本地库不可用 / 5 接口不可达）
llt check

# 生成数据库
llt build                                    # 已是最新则跳过；否则下载最新数据并生成 sqlite
llt build --csv -o out                       # 只导出 stations/trains/stops.csv
llt build -i an.db -o out                    # 用本地 an.db（zip）解码
llt build -i out/data/an -o out              # 已解包的目录也能直接解码（大小写不敏感）

# 查询（默认查“今天开行”）
llt query train G1                           # 精确车次优先：只有 G1
llt query train G1 --prefix --limit 0        # 前缀匹配：G1/G10/G101…
llt query train 54 --all                     # 车次号 54 的全部前缀：G54/K54/Z54
llt query stops G1                           # 完整停站表
llt query stops --index 5884                 # 用 train_index 指定（同号多区间时）
llt query station 北京 --limit 0             # 北京当天经停列车
llt query station 北京 --date 2026-11-08     # 指定日期
llt query od 北京 上海                       # 站到站直达
llt query od 北京 上海 --json                # JSON 输出，便于脚本处理
```

### `build` 参数

| 参数 | 说明 |
|---|---|
| `-i`, `--input <FILE>` | 输入 `an.db`（zip）或已解包的目录；省略时从网络下载 |
| `-o`, `--output <DIR>` | 输出目录；默认是**可执行文件所在目录**，不存在会自动创建 |
| `--csv` | 只输出三个 CSV，不写 sqlite |
| `--url <URL>` | 数据包地址；省略时用版本接口给出的镜像列表逐个尝试 |
| `--ver-url <URL>` | 版本接口，默认 `http://down.lltskb.com/android.ver` |
| `--force` | 忽略“已是最新”的判断，强制重新下载构建 |
| `--no-version-check` | 不做线上版本检查，直接下载构建（脚本 / 镜像场景） |
| `--keep-temp` | 保留临时目录并打印路径（排查问题用） |
| `-q`, `--quiet` | 不输出下载进度与提示 |

### `check` 与“已是最新”短路

版本号来自官方「检查更新」接口 `android.ver`（`tools/check_update.py` 的复刻）：
一次 2 KB 请求就能拿到 `version.data`（数据版本，与数据包里的 `ver.txt` 同义）、
`version.app`、`minSdk` 与 `dataUrl` 镜像列表。响应不是严格 JSON（实测有 `"name": 应用宝"`
这种缺引号的写法），解析器会先做行内修补，**解析失败一律当作“未知”，绝不当作“最新”**。

`llt build` 在未给 `-i` 时会先查版本，然后按下面的矩阵决定：

| 本地状态 | 线上 `version.data` | 行为 |
|---|---|---|
| 没有库 / 没有 `meta`（Python 建的库） | 任意 | 重建 |
| `schema_version` 低于当前工具 | 任意 | 重建（提示工具升级） |
| 缺表、行数与 `meta.*_count` 不符 | 任意 | 重建（上次构建被占用或中断） |
| 结构最新、版本相同、库完整 | 相同 | **跳过**，打印版本与构建时间 |
| 版本不同 | 较新 | 下载镜像 → 重建 |
| 本地比线上新 | 较旧 | 不覆盖，提示加 `--force` |
| 接口不可达 / 解析失败 | — | 提示后照常下载重建，**不跳过** |

下载会按 `dataUrl` 顺序逐个尝试（显式 `--url` 时只用该地址）；解包后还会用实际的
`ver.txt` 与接口声明比对，镜像尚未同步时提示“以实际值为准”。

`--csv` 模式没有 `meta` 表，因此会在输出目录写 `meta.json`（数据版本 / schema /
构建时间 / 来源 / 行数），让 CSV 也能判断是否最新。

### `query` 子命令

| 子命令 | 说明 |
|---|---|
| `train <CODE>` | 按车次查：完整车次 `G1`、车次号 `54`、斜杠车次 `G97`；`--prefix` 改为前缀匹配 |
| `stops <CODE>` | 完整停站表；`--index <N>` 直接按 `train_index` 指定 |
| `station <NAME>` | 某车站的经停列车，按本站发车时间排序 |
| `od <FROM> <TO>` | 站到站直达车次，含历时、里程、跨天 |

公共参数：

| 参数 | 说明 |
|---|---|
| `--db <FILE>` | 数据库路径；省略时按下面的顺序查找 |
| `--strict` | 只认 `--db` 与 `$LLT_DB`，不做隐式搜索（脚本用） |
| `--date <DATE>` | 查询日期，`YYYY-MM-DD` 或 `YYYYMMDD`，默认今天 |
| `--all` | 忽略日期，显示全部开行区间（与 `--date` 互斥） |
| `--limit <N>` | 最多显示多少条，默认 50，`0` 表示不限制 |
| `--json` | 输出 JSON（`query station` 额外含 `same_city`） |

数据库定位（`query` / `check` / GUI 共用 `lltdb::path`）：

| 顺序 | 位置 | 说明 |
|---|---|---|
| 1 | `--db <path>` | 给了就必须存在，不存在直接报错，不再回退 |
| 2 | `$LLT_DB` | 同上 |
| 3 | 可执行文件旁 | `llt build` 的默认输出位置 |
| 4 | 当前目录 | `./llt-timetable.sqlite`、`./out/timetable/llt-timetable.sqlite` |
| 5 | 向上找仓库 `out/timetable` | **仅开发环境**：自动逐级向上（最多 6 层），任意深度都能命中 |

“开发环境”按**运行时**判断，而不是 `debug/release` 编译模式：可执行文件路径里含 `target` 段，
或当前目录的某个祖先有带 `[workspace]` 的 `Cargo.toml`。所以仓库里跑
`cargo build --release` 的产物、或把 release 二进制复制到仓库根运行，都能找到 `out/timetable`；
真正发布（二进制放在任意目录）时只会查前四项。

`query` 会先在 stderr 打印实际使用的库和来源（`-q` 可关闭），
找不到时错误信息会列出尝试过的全部路径。

日期语义（与 GUI 一致）：

- 车次：按 `trains.date_from` ~ `date_to` 判断查询日是否开行；
- 车站 / 站到站：按停站「第几天」倒推**始发日期**再判断，所以夜间车、跨天车按
  “当日在本站有车/能乘车”归类；
- `--all` 不做日期过滤，用于查历史区间或停运车次。

输出示例：

```
$ llt query od 北京 上海
北京 → 上海 · 2026-10-31 周六 开行 · 5 趟直达
车次  车型        出发   到达       历时        里程     起 → 终
----  ----------  -----  ---------  ----------  -------  -----------
G5    高速动车    07:40  12:32      4小时52分   1334 km  北京 → 上海
1461  新空调普快  11:59  06:45+1天  18小时46分  1463 km  北京 → 上海
...
```

`--json` 的结构固定：`train` / `station` / `od` 为
`{query, date, total, shown, truncated, items: [...]}`，`stops` 为
`{query, train, train_index, count, stops: [...]}`（另有车型、起终站、里程、开行区间）；
条目字段与表结构同名
（如 `train`、`first_station`、`arrive_min`、`duration_min`）；`stops` 的条目另有
`display_train`（该站显示的车次）与 `platform`（站台 / 出站口，没有数据时为空字符串）。
`query train` / `query stops` 的条目还带三组附加数据：

| 字段 | 内容 |
|---|---|
| `duties` | 交路：`company`（担当）、`formation`（编组）、`chain`、`segments`（`label` / `code` / `current`） |
| `via` | 经由：`seq`、`station`、`line`（线路名，跨线 `A/B`）、`km`（累计里程）、`is_station`（是否车站）、`station_index`（车站下标，非车站为 `null`） |
| `dining_cars` | 餐车：`coach` 与生效区间 |

数据缺失（例如用旧版工具建的库）时这三个字段是空数组，其余查询不受影响。

`query station` 的 JSON 另外带 `same_city`：

```json
{"station_index": 165, "stations": ["北京", "北京北", "…"],
 "groups": [{"group_id": 165, "group_name": "北京", "is_city_node": false, "members": [...]}]}
```

`stations` 是合并镜像组后的同城候选（自身排第一），`groups` 是原始组，保留 gid 与
`is_group_head`，便于需要精确复刻 App `OooO0Oo()` 行为的调用方。

`is_station` 与 `station_index` 是同一事实的两种表示，互为充要条件：
`is_station = 1` 当且仅当 `station_index IS NOT NULL`（DDL 里用
`CHECK ((is_station = 1) = (station_index IS NOT NULL))` 固化，写库时不会出现不一致）。
保留布尔是为了过滤/展示直观，保留下标是为了能 join `stations`。

### 同城车站（「模糊站名」）

`sn.dat` 把「同一城市的一组车站」挂在同一个 gid 下；App 勾选「模糊站名」后按站搜索
（`llt-gui` 的「模糊」勾选走的是同一批数据，见 `gui/README.md`），
候选就是该组成员。`llt query station 北京` 会在表格后面补一行：

```
同城车站（11 站）：北京、北京北、北京朝阳、北京大兴、北京东、北京丰台、北京南、北京西、昌平北、清河、亦庄
```

要点：

- gid 自身**不在**成员向量里，App 查询时插到首位；本工具在 `same_city_groups`
  里用 `head_station_index` 表达 gid（城市名节点的 head 为 NULL）。
- 有 108 对互为镜像的组（天津的 `192(滨海)` 与 `2310(天津)`，成员仅差自身），
  因此一个站可能命中多个组，输出时取并集；`groups` 字段保留原始分组。
- 658 组覆盖 1,406 / 3,331 站（42%）；组大小最大 12（天津、兰州），中位 2。
- `s.i` 里没有「成都」（站改中），但 S1 有城市名节点 `10004(成都)`，下属
  成都东 / 成都南 / 成都西 / 犀浦。用 `query station 成都` 会直接提示这些车站。

经由点是否「车站」的判据是站名能否在 `s.i` 车站表里找到，也就是能否直接用于
`llt query station <站名>`。实测 1,514 个不同经由点中 1,111 个是车站，
其余 403 个多为线路所 / 信号所（名字以「所」结尾，共 175 个）或未收录的货运站；
按点计 431,364 个经由点里 311,909 个是车站。所谓“非车站”就是搜不到的那种。

## `build` 流程

1. 参数校验：`-i` 路径必须存在，`-o` 目录会被创建。
2. 在系统临时目录建 `llt-timetable-*`（`tempfile`，出错也会清理）。
3. 没有 `-i` 时用 `ureq` 下载 `an.db` 到该临时目录（流式写入，带进度、超时和长度校验）。
4. 校验 zip 魔数后解出需要的分片到 `临时目录/an`；`ver.txt` 有内容时打印数据版本，并写进 sqlite 的 `meta` 表 / `PRAGMA user_version`。
5. 解码：`s.i`/`t.i` 索引 + 20 个 `tN.dat` 车次分片（必需）+ 10 个 `sN.dat` 车站分片（可选）。
6. 解码附加数据：`routes.dat`（线路 + 车次经由，key 是 `sp.dat` 的内部编号）、
   `jlb.dat`（交路 / 编组 / 餐车，企业名来自 `extra.dat`）、`sn.dat`（同城车站）、
   `xw.dat`（途中改车次号，写进 `stops.display_train`）、
   `plat.dat`（站台 / 出站口，写进 `stops.platform`）。缺失或损坏只提示、不中断。
7. 导出：默认写 `llt-timetable.sqlite`，`--csv` 时写 CSV（3 张主表 + 经由 / 交路 / 餐车 / 同城）。
8. 删除临时目录（`--keep-temp` 除外），打印统计与产物路径。

正常结束（包括任何错误返回）都会清理临时目录；只有 Ctrl-C、关窗口等强制中断可能留下
`%TEMP%\llt-timetable-*`，手动删除即可（每个目录只有下载的 `an.db` 和解包的分片，几 MB）。

## 产物

三张数据表与 `tools/decode_llt.py` 相同（`stops` 再多一列 `platform` 写站台），
另外多一张 `meta` 表（版本信息）：

| 文件 | 内容 |
|---|---|
| `llt-timetable.sqlite` | 主表 + 附加表 + `meta` 与版本 pragma，`gui/` 可直接读取 |
| `trains.csv` | 车次、车次号、车型、开行日期区间、起终站、里程、天数 |
| `stops.csv` | 停站：车次、显示车次、站序、站名、里程、到/发、停留、第几天、站台 |
| `stations.csv` | 车站及 `sN.dat` 里的车次数 |
| `train_routes.csv` | 车次经由：车次、点序、站名、线路、累计里程 |
| `duties.csv` | 交路：日期区间、担当企业、编组、交路串、拆段 |
| `dining_cars.csv` | 餐车车厢 |
| `same_city.csv` | 同城组：组 id / 组名 / 节点下标 / 站名 / 是否组头 |
| `city_nodes.csv` | 城市名虚拟节点及其下属车站 |
| `tele_aliases.csv` | 站名-电报码别名 |
| `meta.json` | `--csv` 的版本标记：数据版本 / schema / 构建时间 / 来源 / 行数 |

SQLite 里的附加表（`llt build` 写入，`tools/decode_llt.py` 不写）：

| 表 | 内容 |
|---|---|
| `lines` / `line_stations` | `routes.dat` 的 796 条线路及其有序节点（站名、电报码、里程） |
| `train_routes` | 车次经由展开结果，每行一个点（`train_index, seq, station, line, km, is_station, station_index`） |
| `duties` / `duty_segments` | 交路（`jlb.dat`）：日期区间、担当企业、编组、交路串与拆段 |
| `duty_trains` | 「交路段 ↔ 车次」映射（构建时解析），`duties_for(train_index)` 直接走索引 |
| `dining_cars` | 餐车车厢与生效区间 |
| `city_nodes` | `sn.dat` S1：城市名虚拟节点（73 条，id ≥ 10000） |
| `same_city_groups` / `same_city_members` | `sn.dat` S2：同城组（658 组）与成员（1,026 条） |
| `tele_aliases` | `sn.dat` S3：站名-电报码别名（364 条） |

查询示例：

```sql
-- 某趟车的经由（is_station = 0 的是线路所 / 信号所等搜不到的站）
SELECT seq, station, line, km, is_station, station_index
FROM train_routes WHERE train_index = 6012 ORDER BY seq;
-- 只取真正可以按车站搜索的点
SELECT seq, station, km FROM train_routes
WHERE train_index = 6012 AND is_station = 1 ORDER BY seq;
-- 某趟车所属交路（已按 train_index 解析好；Z158/5 与 Z155/Z158 这类写法也能命中）
SELECT d.company, d.formation, d.chain, t.seq AS chain_position
FROM duties d JOIN duty_trains t ON t.duty_id = d.duty_id
WHERE t.train_index = 6012 ORDER BY d.start_date;
-- 线路上的车站
SELECT seq, station, km FROM line_stations
WHERE line_code = (SELECT line_code FROM lines WHERE line_name = '京沪高速线') ORDER BY seq;
-- 某站的同城组（「模糊站名」候选）：组头 + 成员
SELECT g.group_id, g.group_name, g.is_city_node,
       COALESCE(g.head_station_index, g.group_id) AS node, g.group_name AS station
FROM same_city_groups g WHERE g.group_id = 165
UNION ALL
SELECT m.group_id, g.group_name, g.is_city_node, m.station_index, m.station
FROM same_city_members m JOIN same_city_groups g ON g.group_id = m.group_id
WHERE g.group_id = 165 ORDER BY node;
-- 城市名节点（s.i 里没有「成都」）下属车站
SELECT m.station_index, m.station FROM same_city_members m
JOIN same_city_groups g ON g.group_id = m.group_id
WHERE g.is_city_node = 1 AND g.group_name = '成都' ORDER BY m.seq;
```

SQLite 用 bundled SQLite 写入：先写同目录的临时文件，成功后再改名，避免留下半个库；
若目标文件被占用（例如 `llt-gui` 正开着）会给出明确提示。

CSV 与 Python 版的 BOM / 行尾 / 引号规则相同；`stops.csv` 比 Python 版多一列 `platform`。

## 数据版本与时间

数据包里唯一的时间信息是 8 字节的 `ver.txt`（`YYYYMMDD`，无换行，例如 `20261108`），
官方用它作为数据版本；各分片的 zip 时间戳只是打包时文件的 mtime（`ime.dat` 甚至还是 2009 年），
不可靠。服务器另外会返回 HTTP `Last-Modified` / `ETag`，那是数据包的**发布**时间。

生成的 sqlite 因此在三处标明版本：

| 位置 | 内容 |
|---|---|
| `PRAGMA user_version` | 数据版本整数，如 `20261108`（`ver.txt` 缺失时为 0） |
| `PRAGMA application_id` | `0x4C4C5401`（`LLT` + 格式版本），用于识别本工具生成的库 |
| `meta` 表 | 见下表 |

`meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)` 的键：

| key | 含义 |
|---|---|
| `schema_version` | 表结构版本（当前 5） |
| `generator` | 生成工具，如 `llt 0.1.0` |
| `data_version` / `data_date` | `ver.txt` 原文 / `2026-11-08` |
| `built_at` | 本库生成时间（UTC，RFC3339，精确到秒） |
| `source` | 下载地址，或 `-i` 的输入路径 |
| `source_last_modified` / `etag` | 仅下载时存在，来自 HTTP 响应头 |
| `archive_bytes` | 数据包字节数 |
| `station_count` / `train_count` / `stop_count` | 行数，便于快速校验 |
| `platform_record_count` | `plat.dat` 的记录数（97,394） |
| `train_change_count` | 有分段的车次数（`xw.dat`，4,562） |

```sql
PRAGMA user_version;                               -- 20261108
SELECT value FROM meta WHERE key = 'data_version';  -- 20261108
SELECT value FROM meta WHERE key = 'built_at';      -- 2026-10-31T13:15:07Z
```

注意 `trains.date_from` / `date_to` 是车次的**开行区间**，不是数据更新日期。
Python 版 `decode_llt.py` 不写 `meta` 表，对比两个库的内容时忽略它即可。

## 构建与测试

在仓库根目录（workspace）执行：

```bash
cargo build -p llt --release
cargo test -p llt
cargo clippy --workspace --all-targets -- -D warnings
```

生成的可执行文件在 `target/release/llt.exe`；也可以在 `cli/` 目录里直接 `cargo build`。

测试覆盖 base255/base128 编码、索引截断、车次号/车型标签、跨天时间换算、分片截断容错、
CSV 的 BOM/CRLF/引号规则、表格的中文宽度对齐、`query` 的车次/车站解析与歧义处理，
以及附加数据解析：`routes.dat` 经由展开（对照 App/Python 参考值：G1 27 点 1318 km、
K54 67 点 728 km、1461 61 点 1463 km）、`jlb.dat` 交路拆段与餐车去重、`sp.dat` 内部编号。
测试跑在仓库自带的 `out/data/an*` 与 `out/timetable/llt-timetable.sqlite` 上，
数据缺失时自动跳过。

## 退出码

- `0` 成功 / 已是最新 / 本地比线上新（不覆盖）；不带子命令打印帮助也是 `0`
- `1` 运行错误（参数路径不存在、下载失败、数据包缺文件/损坏、写盘失败、查不到车次等），
  错误信息按 `错误：外层原因: 内层原因` 的形式打印到 stderr
- `2` 命令行用法错误（`clap` 默认行为，例如 `--date` 与 `--all` 同时使用）
- `3` `check`：线上有更新的数据（或库结构过旧需要重建）
- `4` `check`：本地库缺失、结构过旧或不完整
- `5` `check`：无法访问版本接口（未联网 / 服务器故障）
