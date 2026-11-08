# llt-gui 配置文件方案（草稿）

目标：把界面里「下次打开还想保留」的少量状态落盘，避免每次启动重新输入。

> **实现状态**：已按本方案落地（新增 `gui/src/config.rs`，改 `gui/src/main.rs`、`gui/src/app.rs`）。
> 实际验证结果见 §7 末尾。

本期范围（用户提出）：

| # | 状态 | 现状字段 | 现状默认值 |
|---|---|---|---|
| 0 | 当前标签页 | `tab` | 站站（第一位） |
| 1 | 站站「模糊」 | `od_fuzzy` | `false` |
| 2 | 三种查询方式的查询词 | `train_query` / `station_query` / `od_from` + `od_to` | 空 |
| 3 | 车次详情经由「只看车站」 | `via_stations_only` | `true` |

明确不做（记录在案，避免以后当成 bug）：

- 查询日期、选中的车次、同城车站勾选集、站站结果的车站筛选——属于「会话现场」，重启后回到干净状态更符合预期。
- 窗口大小 / 位置 / 主题：与业务无关，见 §10 备选。
- 历史记录列表（多次查询）：已确认只存「上一次」，实现见 §10 的扩展建议。

---

## 1 现状梳理

`gui/src/app.rs` 中相关字段与写入点：

| 字段 | 写入点 |
|---|---|
| `od_fuzzy` | `od_ui` 的复选框 → `refresh_od()` |
| `train_query` | `train_ui` 输入框 / 清空按钮 |
| `station_query` | `station_ui` 输入框 / 清空按钮 / 候选点选 / `follow_detail_link` / `go_back` |
| `station_fuzzy` | `station_ui` 复选框 / `follow_detail_link`（强制 `false`）/ `go_back`（还原 `ReturnPoint`） |
| `od_from` / `od_to` | `od_ui` 输入框 / 交换按钮 / 候选点选 |
| `via_stations_only` | `detail_ui` → `via_section_ui` / `via_page_ui`，收尾写回 `self.via_stations_only` |

两个关键约束：

1. **查询依赖数据库**。`refresh_*` 全部读 `self.db`，所以「恢复查询词」必须排在 `open_db()` 之后；`open_db()` 还兼作「重试」按钮的入口，恢复逻辑要能被重试路径再次触发。
2. **`station_query` 的一条特殊路径**：详情里点车站会把 `station_fuzzy` 置 `false` 再查询。车站页的模糊也一并持久化（已确认），落盘的就是「用户当下看到的状态」；副作用见 §5 末尾的已知行为。

`eframe::App` 在本仓库的实现方式是 `fn ui(&mut self, ui, frame)`；`on_exit(&mut self)`（未启 `glow` feature）与 `save(&mut self, _storage)`（仅 `persistence` feature 下会被调用）都可用。

---

## 2 方案选型

### A. 自建 JSON 配置文件（推荐）

新增 `gui/src/config.rs`，`serde` + `serde_json`（`serde_json` 已在 workspace 依赖里，`serde` 需补一行；`serde_derive` 已在 `Cargo.lock`，几乎无新增编译量）。

- 优点：是「配置文件」而不是二进制/框架私有格式，可手改、可删除、可 git 化；schema 与 egui 版本无关；纯函数易测（路径解析、读写、坏文件）；与 CLI 后续共享也容易。
- 缺点：要自己管路径、写盘时机、错误提示；约 200 行新代码。

### B. eframe 的 `persistence` feature

`eframe = { features = ["persistence"] }` + `impl App::save` + `eframe::get_value/set_value`。

- 优点：代码最少（约 20 行），自动 30 秒一存、退出必存，顺带保存窗口几何与主题偏好。
- 缺点：
  - 落盘位置是 `%APPDATA%\<app_id>\data\app.ron`，默认 `app_id` 就是窗口标题「路路通时刻表」，得补 `.with_app_id("llt-gui")` 才能拿到干净路径；
  - `app.ron` 里混着 egui 的内存镜像（面板尺寸、折叠状态、`TableBuilder` 状态……），体积大、人不可读、跨 egui 版本可能失配；
  - 业务状态和 UI 框架状态绑死：以后想换 UI 框架或想让 CLI 读同一份配置，都要再迁一次；
  - 无法只重写「业务那几条」，每次都是整块 `app.ron`；
  - `save` 只在退出与 30 秒间隔调用，且**只在启用了 feature 时**——与 A 的持久性同档，没有质变。

### C. 存进 SQLite（`meta` 表或新表）

`out/timetable/llt-timetable.sqlite` 是 `llt build` 的产物，重建即覆盖；把用户偏好写进去会被一次更新抹掉，且 GUI 现在是**只读**连库（`Db` 只做查询）。直接排除。

### 对比

| 维度 | A 自建 JSON | B eframe persistence | C 存库 |
|---|---|---|---|
| 新增依赖 | `serde`（已传递编译） | `serde` + `ron` + feature | 无 |
| 代码量 | ~200 行（含测试） | ~20 行 | ~60 行 |
| 可读 / 可手改 | ✅ | ⚠️ RON + 大块 UI 状态 | ⚠️ |
| 与 egui 版本解耦 | ✅ | ❌ | ✅ |
| 可单元测试 | ✅ 纯函数 | ❌ 依赖 `Storage`/`Frame` | ✅ |
| 顺带存窗口/主题 | ❌ | ✅ | ❌ |
| 被 `llt build` 覆盖的风险 | 无 | 无 | **高** |
| 出错影响面 | 只回退偏好 | 可能丢整个 UI 状态 | 污染数据包 |

**结论：选 A。** B 的唯一优势（窗口几何）不值得让业务配置依赖 UI 框架的序列化格式；若以后确实想要窗口记忆，可单独再开 `persistence`，与 A 并存，互不影响。

---

## 3 数据模型与文件格式

`llt-gui.json`，按标签页分组（便于以后每个页面继续加字段）：

```json
{
  "version": 1,
  "ui":      { "tab": "od" },
  "train":   { "query": "G1" },
  "station": { "query": "北京", "fuzzy": false },
  "od":      { "from": "北京", "to": "上海", "fuzzy": true },
  "detail":  { "via_stations_only": true }
}
```

`ui.tab` 的取值就是三个标签页的 `snake_case` 名字：`od` / `train` / `station`，
缺省或第一次启动是 `od`（站站，界面第一页）。

兼容规则：

- 所有结构体加 `#[serde(default)]`，字段缺失就取默认值 → 老文件缺字段、新版本加字段都不会解析失败；
- 未知字段忽略（**不要**加 `deny_unknown_fields`），降级运行也安全；
- `version` 目前只作标记与将来迁移的判断依据，读到更高版本不报错，直接按已知字段尽力解析；
- 字段**值**非法（例如 `"tab": "diagram"`）算解析失败：整份回退默认值 + 顶部提示 + 原名改 `.bad`，
  这是刻意的——宁可一次性回到默认，也不要带着半个状态运行；
- 写盘用 `serde_json::to_vec_pretty`，UTF-8 无 BOM、LF、缩进 2、字段顺序固定（`serde` 按声明顺序），保证 diff 友好。

默认值必须与现在硬编码的一致：`train.query=""`、`station.fuzzy=false`、`od.fuzzy=false`、`detail.via_stations_only=true`。这样「没有配置文件」时行为和今天完全一样。

---

## 4 文件位置与优先级

```
1. --config <path> / --config=<path>
2. 环境变量 LLT_CONFIG
3. <exe 目录>/llt-gui.json        （默认；绿色版语义）
4. <当前目录>/llt-gui.json        （取不到 exe 路径时的兜底）
```

理由（已确认按「就近 exe 旁」）：

- 绿色版语义：`llt-gui.exe`、`llt-timetable.sqlite`、`llt-gui.json` 放同一个目录，整个目录复制走，配置跟数据一起过去，不往宿主机写任何东西。
- 代价：exe 在 `Program Files`、只读共享盘或 `target/debug` 里时可能写不进去（`cargo clean` 也会一起删掉）。这种情况下界面提示一次「保存配置失败」，并指向 `--config` / `LLT_CONFIG`——这两个开关同时也是测试与多套配置的入口，术语与 `--db` / `LLT_DB` 保持一致。
- 不做「系统配置目录」分支：多一条隐式路径就多一处「我的配置到底在哪」的困惑，需要时用 `--config` 显式指定。

解析逻辑是纯函数 `resolve(explicit, env, exe_dir, cwd) -> PathBuf`，便于单测（参考 `lltdb::path::collect`）。

---

## 5 读写时机

### 启动

```
fonts::install / theme::install
load(config_path)                      ← 失败只降级，不阻断
LltApp { 字段 = 配置值, error = 提示 }
open_db()
  ├─ 成功 → refresh_train_search(); refresh_station(); refresh_od()
  └─ 失败 → 显示 db_error_ui（保留「重试」）
```

- 标签页直接恢复成上次那一页；焦点按该页的规则落位（站站落在还空着的那一侧）。
- **恢复后主动查询一次**，否则界面上会出现「有查询词、有清空按钮、却没有结果」的空转状态（输入框只在 `changed()` 时查询，切标签页不会重查）。
- 三次刷新都在 `open_db()` 成功后执行，因此「重试」按钮也要在成功分支里补一次恢复查询（现在的 `open_db()` 是 `db_error_ui` 里唯一的入口，改一处即可）。
- 三种查询的启动开销都有限：车次是前缀/`LIKE`；站站有 `OD_RESULT_LIMIT = 500` 封顶；车站的 `station_board_multi` 没有 `LIMIT`，同城组最多几千行，实测量级仍在毫秒级。若以后出现启动卡顿，再改成「首次切到该标签页时再刷新」的懒恢复（多一个 `pending: [bool; 3]`，代码约 +10 行），不影响文件格式。

### 运行期

不在每个修改点散落 `save()`（`refresh_*` 是逐键触发的，逐键写盘既浪费又会产生半截输入）。改为**帧首快照对比 + 去抖**：

```rust
fn persist(&mut self, ctx: &egui::Context) {
    let snapshot = self.settings();                     // 7 个字段，Clone + Eq
    if snapshot == self.persisted {
        self.dirty_since = None;
        return;
    }
    let now = Instant::now();
    let since = *self.dirty_since.get_or_insert(now);
    if now.duration_since(since) < SAVE_DEBOUNCE {
        ctx.request_repaint_after(SAVE_DEBOUNCE);       // 没有输入时补一次唤醒
        return;
    }
    self.store(&snapshot);                              // 写盘 + 记录快照
    self.dirty_since = None;
}

fn store(&mut self, snapshot: &config::Settings) {
    if let Err(err) = config::save(&self.config_path, snapshot) {
        if !self.save_failed {                          // 同一条错误只报一次
            self.save_failed = true;
            self.error = Some(format!("保存配置失败（{}）：{err}", self.config_path.display()));
        }
    } else {
        self.save_failed = false;
    }
    self.persisted = snapshot.clone();                  // 失败也记「已处理」，避免 500 ms 一次重试
}
```

- 逐帧只做一次 7 字段比较，成本可忽略；打字过程中最多每 500 ms 落盘一次，停手 0.5 s 内必定落盘。
- 复选框这类离散操作也走同一条路径，不需要单独的 save 调用点。

### 退出

`fn on_exit(&mut self)` 里做一次无条件 `save`（`save_failed` 为真时跳过，避免关窗时再弹错误）。

- 该签名依赖 eframe 的 `glow` feature：本仓库 `eframe` 走默认 feature（wgpu），且 `NativeOptions.renderer = Wgpu`，所以是 `fn on_exit(&mut self)`；如果哪天 feature 统一把 `glow` 打开，这里会编译失败而不是静默出错，注意同步改了即可（在 doc 注释里写明）。
- 强制结束进程（任务管理器 / 断电）会丢最后 0.5 s 内的改动，属于可接受范围。

### 写盘安全

```rust
fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() { fs::create_dir_all(dir)?; }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    fs::rename(&tmp, path)          // Windows 上 std::fs::rename 覆盖已存在文件
}
```

先写临时文件再 `rename`，避免写一半掉电留下截断的 JSON（Windows 的覆盖式 rename 是原子的）。

### 读取异常处理

| 情况 | 处理 |
|---|---|
| 文件不存在 | 用默认值，**不**提示（首次启动的常态，标签页即第一页「站站」） |
| JSON 语法错 / 字段类型错 | 用默认值 + 顶部错误条「配置文件解析失败，已使用默认设置」，并把坏文件改名 `llt-gui.json.bad`（避免下一帧被覆盖后无处查证） |
| 目录不可写 | 用内存值运行 + 一次错误提示；后续改动不再重试 |
| 与另一实例同时运行 | 后写覆盖，不做文件锁（配置里全是 UI 偏好，冲突无害）；在文档里写明 |
| 值越界（如超长查询词） | 加载时截断到上限（车次 32、站名 32 字符）即可，不做严格校验 |

---

## 6 改动清单

| 文件 | 改动 | 规模 |
|---|---|---|
| `Cargo.toml` | `[workspace.dependencies]` 增 `serde = { version = "1", features = ["derive"] }` | +1 |
| `gui/Cargo.toml` | 增 `serde`、`serde_json` | +2 |
| `gui/src/config.rs`（新） | `Settings` / `Tab`（serde）、`resolve` / `resolve_path` / `load` / `save` + 10 个单测 | 385（其中测试约 175） |
| `gui/src/main.rs` | `Args::parse_from`（`--db` / `--config` 一次解析）+ 2 个单测；`LltApp::new` 多传一个配置路径 | +92 / −29 |
| `gui/src/app.rs` | 新增 `config_path` / `persisted` / `dirty_since` / `save_failed` 字段；`new()` 先加载再 `open_db`、恢复标签页与焦点；`open_db()` 成功分支追加 `refresh_restored_queries()`；`ui()` 帧首调用 `persist()`；`on_exit()`；`settings()` / `store()` / `focus_field()` | +130 / −20 |
| `README.md` | 一段「配置文件」：路径、字段、删了会怎样 | +10 |
| `docs/gui-config.md` | 本文 | — |

`Settings` 与界面的映射只有两个函数，借 `&mut self` 直接读写既有字段，不引入中间状态层：

```rust
impl Settings {
    fn apply(&self, app: &mut LltApp) { app.train_query = self.train.query.clone(); /* … */ }
}
impl LltApp {
    fn settings(&self) -> Settings { Settings { train: Train { query: self.train_query.clone() }, /* … */ } }
}
```

`station_fuzzy` 已纳入（`gui/src/config.rs` 的 `station.fuzzy`），`via_stations_only` 同理——它已经是 `detail_ui` 里「读出来、改在局部、再写回」的字段，不涉及额外管道。

---

## 7 测试计划

单元测试（`gui/src/config.rs`，纯函数、不碰 GUI）：

1. `save` → `load` 往返一致（含中文站名、空字符串、`via_stations_only=false`）；
2. 文件不存在 → 全默认，且不产生告警；
3. 坏 JSON / 字段类型错 → 全默认 + 有告警 + 生成 `.bad`；
4. 老文件缺 `detail` 段 → 该段取默认（`true`），其余保留；
5. 未知字段 / 更高 `version` → 正常解析，不报错；
6. `resolve` 优先级：`--config` > `LLT_CONFIG` > exe 目录 > 当前目录（纯函数传参，参考 `lltdb::path` 的测试写法）；
7. 写盘失败（目录只读 / 父路径是文件）→ 返回 `Err` 且不留 `.tmp`。

手工验收（`cargo run -p llt-gui`）：

| 场景 | 期望 |
|---|---|
| 改动三项 → 等 1 秒 → 直接结束进程（非正常退出）→ 重启 | 三项都恢复 |
| 切换模糊、填三组查询词 → 正常关窗 → 重启 | 三项恢复，且三个标签页各自已有结果（不是空白） |
| 删掉配置文件 → 重启 | 回到今天的默认行为（无模糊、空查询、只看车站勾选） |
| 写入 `{` 坏文件 → 重启 | 能启动、顶部有提示、`.bad` 文件存在 |
| 把 `llt-gui.json` 放在 exe 旁 → 重启 | 读写的就是这一份 |
| 目录改为只读 → 改设置 | 界面不崩，提示一次「保存配置失败」 |
| `--db` 指向错路径 → 启动 → 改成对的 → 点「重试」 | 重试成功后恢复的查询词直接出结果 |

构建门槛：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`。

### 实施时已跑过的验证（Windows 11 / 250% 缩放 / 数据 20261108）

| 验证 | 结果 |
|---|---|
| 写配置 + 启动，`ui.tab=station` | 直接停在「车站」页，焦点在该页输入框 |
| 写配置 + 启动，`train.query=G1` | 输入框直接是 `G1`，且已带结果「694 条匹配」（启动即恢复查询） |
| `station.query=北京` + `fuzzy=true` | 车站页：模糊勾选、同城组/候选站/车站筛选就位、「11 站 · 2172 趟经停」 |
| `od.from/to=北京/上海` + `fuzzy=true` | 站站页：模糊勾选、两侧输入就位、「58 趟直达」结果已在 |
| `detail.via_stations_only=false` | 改一次别的设置触发写盘后，文件里仍是 `false`（说明读入内存的是 false，写回的是内存快照） |
| 手输字符 → 自然关窗 | 0.5 s 后 `llt-gui.json` 出现在 exe 旁；未改动时不会产生任何写入 |
| `--config <temp>\llt-gui.json` 指向一份坏 JSON | 正常启动、原文件改名 `.bad`、新文件写在指定位置，exe 旁不产生配置 |
| `cargo test --workspace` / clippy / fmt | 13 个 GUI 单测通过（含 6 个参数解析与 8 个读写路径），无告警 |

一处实施细节：`--config --db x` 这类「选项后面紧跟另一个选项」的写法，不再把 `--db` 当成路径吃掉
（`Args::parse_from` 用 `peekable` 判断），单测覆盖。

---

## 8 风险评估

| 风险 | 可能性 | 影响 | 缓解 |
|---|---|---|---|
| 启动时同步查询拖慢首帧 | 低 | 低 | 有上限的查询已在毫秒级；预留「首次进页再刷新」开关 |
| 恢复查询词但库版本变了（车次/车站已不存在） | 中 | 低 | 现有 UI 已有「没有匹配 / 没有找到」空状态；不额外处理 |
| 配置与数据包不配套（比如车站改名） | 低 | 低 | 同上，空状态兜底 |
| 逐帧比较 + 去抖逻辑写错导致频繁写盘 | 低 | 中 | 单测覆盖 `persist` 的状态机（把去抖抽成纯函数 `should_write(now, dirty_since, changed)`） |
| `on_exit` 签名随 eframe feature 变化 | 低 | 编译期暴露 | docs 注明；不启用 `glow` |
| 坏配置阻断启动 | 低 | 高 | 解析失败一律回退默认值，绝不 `unwrap`/`?` 向上冒泡 |
| 多实例互相覆盖 | 中 | 低 | 只存 UI 偏好；文档说明 |
| 用户找不到配置文件 | 中 | 低 | 错误提示里带完整路径；README 写明位置 |

行为回归面很小：不碰 SQL、不碰 `lltdb`、不新增网络；无配置文件时所有字段取今日默认值。

---

## 9 工作量

| 项 | 估时 |
|---|---|
| `config.rs` + 单测 | 1.5–2 h |
| `main.rs` / `app.rs` 接入（含去抖与错误提示） | 1 h |
| 手工验收 + README | 0.5 h |
| 合计 | 约 3–4 h；实际增改约 600 行（含测试与文档），其中业务代码约 250 行 |

与现有代码风格一致（无 `unwrap`、错误走 `self.error`、纯函数抽出来测），review 成本低。

---

## 10 备选与将来

- **历史记录列表**（若「三种查询方式记录」指的是多条历史而非上一次）：再加 `recent: { train: [String], station: [String], od: [{from,to}] }`，各限 10 条、去重（车次按大写、站名按 trim 后全等）、最近优先；UI 在每个输入框下方加一行 chip（复用现有 `候选站` 的渲染）。估时 +3–5 h，主要成本在 UI 与「点击历史 = 立即查询」的交互细节。
- **窗口几何 / 主题**：单开 eframe `persistence`（互不干扰），或存进 JSON 的 `window: {size, pos}` 并在首帧应用；后者更可控，但与 `fit_window()` 的收敛逻辑要合并。
- **查询日期**：标签页已经落盘（`ui.tab`）；日期若也要「接着上次继续看」，加 `ui.date` 即可，代价是启动不再固定是「今天」。
- **配置迁移**：`version` 已经占好位；真正需要不兼容变更时按 version 分支改字段名。

---

## 11 已确认的决策

0. **标签页落盘**（`ui.tab`）：重启回到上次那一页；第一次启动是第一位「站站」。
1. **「三种查询方式记录」= 上一次的查询词**（不存历史列表）。所以每个输入框只恢复最后一次的值，不新增任何 UI。
2. **车站页的「模糊」一起存**（`station.fuzzy`），与站站查询的模糊各自独立。
3. **默认落盘位置 = 可执行文件旁**（绿色版），只读目录靠 `--config` / `LLT_CONFIG` 兜底。

### 已知行为（不算 bug，记录在案）

- 从车次详情点车站会切到「车站」页并把模糊关掉（现有交互），这个状态同样会被持久化。若此时直接退出、没点「← 上一个车站」，下次启动看到的就是跳转后的车站而不是原来的同城查询。想避免只能放弃「所见即所存」，取舍上选了后者。
- 多个实例同时开启时后写覆盖先写，不做文件锁：配置里全是 UI 偏好，冲突无害。
- 强制结束进程会丢掉最后 0.5 秒内的改动（去抖窗口）；正常关窗由 `on_exit` 兜底。

## 12 Android

Android 上没有「可执行文件旁」这个语义，所以隐式查找不参与：`android/src/lib.rs`
直接指定路径，配置落在应用私有目录（与随 APK 下发的数据库同一目录）：

```
/data/user/0/org.llt.timetable/files/llt-gui.json
```

字段、去抖写盘、坏文件回退、查询词截断全部与桌面一致；`--config` / `LLT_CONFIG`
在 Android 上不生效（进程没有命令行入口）。详见 [`android.md`](android.md)。
