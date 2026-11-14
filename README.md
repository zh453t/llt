# 路路通离线时刻表（llt）

把「路路通」发布的离线数据包解码成本地 SQLite，**离线**查询全国铁路时刻表：
按站到站查直达车、按车次查、按车站查当天经停，以及车次详情（停站表、经由、交路、
车型担当、开行计划月历）。日常查询不联网，只有「更新数据」时才访问网络。

GUI 项目基于 [`egui`](https://github.com/emilk/egui), Android 与桌面共用一套界面。另外还有一个命令行工具。

| 入口 | 形态 | 说明 |
|---|---|---|
| **Android** | APK（`org.llt.timetable`） | 触摸界面，整页滚动的车次详情；要求 Android 8.0 以上 |
| **桌面** | `llt-gui.exe` | 单文件程序，Windows 桌面（Linux / macOS 理论可编译，未验证） |
| **命令行** | `llt` | 更新数据、查询、导出 CSV / JSON，见 [`cli/README.md`](cli/README.md) |

## 三种查询

三种查询都先选一个**查询日期**（默认今天，切换日期后旁边会出现「今天」按钮）：

| 标签页 | 输入 | 结果 |
|---|---|---|
| **站站** | 出发站 + 到达站 | 可直达的车次、出发/到达时刻、历时、里程、跨天；可按实际出发站 / 到达站筛选 |
| **车次** | `G1`、`K54`、`1461`、`G100/G97`，或只写号（`54` → G54/K54/Z54） | 匹配车次及其始发终到、类型、里程、开行区间 |
| **车站** | 站名或城市名（实时候选） | 该站当日经停的全部列车，按到达/发车时刻排序 |

- 输入即查询：没有单独的「查询」按钮；触摸屏上点一下输入框会清掉原来的文字，直接重输。
- 结果列表把**当日开行**排在前、不开行排在后，前面标 `●` / `○`；底栏给出两组趟数。
- 「模糊」按同城组匹配（城市名与车站名都能匹配），旧数据没有同城表时自动退化为单站。

## 车次详情

点列表里的车次进入详情（Android 是整页，桌面是右侧边栏 / 底部面板）：

- **停站表**：到达、发车、停留、里程、天数、均速；列可自选、可拖宽、可拖表头换序，
  宽度和顺序都会记住。
- **经由**：点数、里程与线路表，可展开或放大看。
- **交路**：车底周转的那几趟车，点开逐段列出「首站 时刻 → 末站 时刻」，车次号可直接跳转。
- **车型 / 担当 / 餐车**：编组、担当企业与餐车位置。
- **开行计划**：按查询日期算那天开不开行的月历（`●` 开行、斜杠 按周期停开、灰 区间外）。
- 停站表里的站名也能点，直接跳到该车站的当日经停；标题旁的「← G105」是回头路。

## 安装与使用

### Android

1. 从 Release 页下载相应 APK（Android 8.0 / API 26 以上）并安装。
2. **APK 不带数据库**，第一次启动应用时应在「设置 → 数据」，
   点「下载并重建」下载数据库并解码；也可以「导入备份」导入外部数据文件。
3. 之后更新只需「检查更新 → 下载并重建」。

### 桌面

1. 运行 `llt-gui.exe`。
2. 没有库时界面「数据」页，可以在应用里下载或导入数据库。
3. 导入本地数据库：将数据库 `llt-timetable.sqlite` 放在 exe 同目录；
   亦可用 `--db <path>` 或环境变量 `LLT_DB` 指定。
3. 配置写在 exe 旁的 `llt-gui.json`（当前标签页、查询词、主题、列设置与列宽）。
   绿色版：整个目录复制走，配置跟数据一起过去。只读目录写不进去时可用 `--config <path>`
   或 `LLT_CONFIG` 指到可写位置。

### 命令行

```bash
llt check                      # 比对本地库与线上版本（0 最新 / 3 有更新 / 4 本地库不可用）
llt build                      # 已是最新则跳过；否则下载数据包、解码、生成 llt-timetable.sqlite
llt build --csv -o out         # 只导出 stations / trains / stops.csv
llt query train G1             # 查询车次（默认今天开行，附交路与经由）
llt query od 北京 上海 --json  # 站到站直达，JSON 输出
```

不带子命令时打印帮助；完整用法见 [`cli/README.md`](cli/README.md)。

## 数据

- 数据来自路路通官方发布的离线数据包（`an.db`，实际是个 zip 容器），由本项目解码成
  SQLite；版本号形如 `20261120`，界面标题栏的「数据 2026-11-20」就是它。
- 本仓库只做解码与查询，**不提供数据包本身**，也不改动数据内容。
- 数据库里读的主要是 `trains` / `stops` / `stations` 三张主表；交路、餐车、经由来自
  附带的几张表，旧库缺这些表时对应信息自动隐藏，不影响三种查询。

## 常见问题

- **默认打开了「数据」页、显示「还没有时刻表数据」**：库文件不在 exe 旁边，或路径不对。
  用 `--db` 指过去，或在应用里下载 / 导入一份。
- **配置没保存**：程序所在目录不可写（如 `Program Files`、只读共享盘）。用 `--config` 或
  `LLT_CONFIG` 指到可写位置。
- **数据来源**：时刻、里程、开行规律全部来自数据库，本项目只做换算与展示；相关内容另见
  [`docs/llt-format-reverse-engineering.md`](docs/llt-format-reverse-engineering.md)。

## 从源码构建

```bash
cargo build --workspace --release   # -> target/release/llt.exe、target/release/llt-gui.exe
cargo test --workspace
```

Android 的 APK 单独构建（需要 NDK 与 Gradle，见 [`docs/android.md`](docs/android.md)）：

```powershell
pwsh -File android/build.ps1 -Release   # -> android/app/build/outputs/apk/release/app-release.apk
```

参与开发请先看 [`CONTRIBUTING.md`](CONTRIBUTING.md)（提交规范）、
[`AGENTS.md`](AGENTS.md)（项目结构、命令与验证流程）；
界面行为与实现细节在 [`gui/README.md`](gui/README.md)，数据格式在
[`docs/llt-format-reverse-engineering.md`](docs/llt-format-reverse-engineering.md)。

## 说明

非官方工具，与路路通官方无关。数据版权归原提供方，本项目仅解码其公开发布的离线数据包，
仅供个人查询使用；请自行遵守数据来源的服务条款。
