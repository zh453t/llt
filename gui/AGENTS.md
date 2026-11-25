# AGENTS.md

## 项目概述

`llt-gui` 是基于 Rust、egui 0.37 和 SQLite 的轻量级列车时刻表查询桌面应用。

主要功能：
- 站站（`od`），查询直达列车（默认标签页）。
- 车次（`train`），支持前缀匹配。
- 车站（`station`），支持站名候选。
- 按日期判断列车开行状态。
- 查看列车详情、交路、车型、担当、经由及停站信息。

详细功能、交互规则和现有限制以 `README.md` 为准。

## UI 设计原则

- 采用现代、简洁、专业的桌面应用风格。
- 优先保证信息层级、视觉一致性和操作效率。
- 统一字体、字号、字重、行高、间距和颜色。
- 保持文本、图标、控件及表格的对齐。
- 合理利用可用空间，避免不必要的留白和拥挤。
- 避免过度装饰、过多边框及不必要的视觉元素。
- 保证中文显示正常，正确处理字体回退和粗体字族。

## 布局与交互

- 遵循 egui Immediate Mode UI 的设计方式。
- 优先使用原生 egui 控件和布局 API。
- 统一各区域的控件尺寸、间距和视觉状态。
- 保证窗口缩放时布局合理，不出现溢出或错位。
- 保留现有宽窄窗口自适应逻辑。
- 保证查询结果和详情表格的可读性。
- 合理处理 Hover、Focus、Active、Disabled 等状态。
- 不随意改变已有交互习惯和键盘操作。

## 实现规范

- 修改前先阅读相关代码，理解现有布局和状态管理。
- 检查实际使用的 egui / eframe 版本及 API。
- 优先复用现有组件，避免重复实现。
- 统一管理主题、颜色、字号和间距。
- 避免硬编码重复的样式参数。
- 避免不必要的内存分配和数据复制。
- 不在 UI 绘制过程中执行耗时数据库操作。
- 不引入没有明确必要性的依赖。

## 平台差异（Android）

- 平台差异只有**一个入口**：`gui/src/lib.rs` 的 `Platform`（`insets` / `ime` / `pointer` /
  `bars` / `dirs` / `files` / `control` / `font_dirs`）。桌面用 `Platform::desktop()`，
  Android 在 `android/src/lib.rs` 里组装。以后再加平台能力（剪贴板、返回键）往这个结构体
  加字段，不要往 `run_with` / `LltApp::new` 加参数。
- 具体差异三件：**系统栏安全区**（`ScreenInsets`，根 `Ui` 已经缩进，新建模态/弹窗时要用
  `safe_screen()` 而不是 `ctx.content_rect()`）、**软键盘**（高度算进 bottom 缩进）、
  **没有鼠标悬停**（悬停才出现的信息不能是唯一入口，现有悬停提示都是补充说明）。
  另外 **系统栏图标颜色**跟着主题走（`Platform::bars`，见 `sync_system_bars`）：
  浅色主题下不改就是白图标白底，状态栏的时间/信号全看不见。
- **不要用 `cfg(target_os = "android")` 在界面里分叉**；需要平台信息就走 `Platform`。
- **尺寸随指针类型取值**：`theme::control_height()` / `list_row_height()` /
  `detail_row_height()` / `link_hot_pad()` / `scroll_bar_width()`。鼠标 30/28/26、
  触摸 44/42/38；新增控件高度时用这些函数，不要写死数字。
- **触摸屏的详情是整页**（`detail_page_ui` 直接复用竖排分支，不要改成 `Modal`：模态按内容
  自动长高，会和「按行取整」的停站表互相喂，长按页面会一路往上跑）。触摸屏同时关掉
  `interaction.selectable_labels`，否则表格里每个 Label 都带 click sense，把点行手势吃掉。
- **手机整页详情只有一个两向 `ScrollArea`**（`detail_page_scroll`）：纵向滚整页，横向滚
  停站表溢出的列。别再给表格套一层横向 `ScrollArea`——内层会把手指拖动整段吃掉，
  按在表格上页面就不动了。杂项（用时 / 交路 / 日历）画在一个摆回视口左沿的子 `Ui` 里，
  宽度固定为视口宽，所以表格右溢出既不撑宽杂项、也不会带着它们横滚。
- **交路明细不在绘制里查库**：每段车次的「首站/末站 + 时刻」是选中车次时一次查完的
  （`duty_runs` → `TrainExtras::runs`，键是 `(交路 id, 段序号)`），展开只查表。
  查不到那趟车就不放进 map，框里那一行只写车次号。折叠状态按车次记（`duty_fold_id`）。
  框内行要自己把 `interact_size.y` 压到一行字高，否则触摸屏每一行都会被抬到 44 点。
- **详情里的滚动区 id 带 `detail_gen`**（`id_salt(("detail_page_scroll", gen))`、
  `("detail_side_scroll", gen)` 等）：`select_train` 每开一次车次就加一，新页面从页首开始，
  不继承上一趟车翻到的位置。新加详情内滚动区/表格时也要把这个世代拼进去。
- **详情的入口（经由 / 开行计划）走同一条 `entry_bar_ui`**：一条上细线 + 一行格子 +
  一条下细线，格子自己只管标题/摘要/尾部「查看 ▸」。不要再手写行（以前三份手写：
  `via_header_ui` / `via_page_trigger` / `run_plan_entry`，行高、箭头、细线各不同，
  叠在一起就是双线加错位）。宽屏侧栏把两个入口并排一行（经由分剩下的宽度，开行计划
  按 `entry_natural_width` 定宽）；窄屏和手机整页仍是一行一个。格子内部间距显式 `add_space`
  给了，子 `Ui` 的 `item_spacing.x` 要置 0，否则会叠成两倍、定宽算不准就会挤成叠字。
- **宽屏的经由是右侧分栏不是就地展开**：点入口条上的「经由」由 `body` 从窗口右侧分出
  `via_right` 面板（先 show 的右面板占最右，详情栏左移），内容画在 `LltApp::via_side_ui`；
  窗口分不出空间时转成 `via_page_ui` 弹窗。不要再把它塞回详情侧栏的滚动流里。
- **开行计划那格不写灰字摘要**：抬头下面已经写过开行区间，再挂一串「区间 · 月历」只是噪声。
- **拖列宽要跟手**：位移按「按下那一刻的宽度 + 指针的绝对位移」算（`ResizeAnchor`），
  不能用每帧的 `pointer.delta()` 累加——触摸屏上按住的头 0.4 秒、以及松手那一帧的位移
  都会丢，列宽比手指走过的距离少一截。并且在**画表之前**先调 `apply_resize_drag`
  把宽度落进去，否则列宽晚一帧生效，快速滑动时分隔线会明显落后于手指。
- **结果表骨架只有一份**：车次 / 车站 / 站站三张结果表都走 `result_table`（`ResultRows` 给出行数、
  开行分界、选中、行→车次下标，闭包只画一格），列集合用 `table_specs`。新增结果表直接调这两个，
  不要再把「`scrollable_table` + 列宽 + 表头 + 拖动」抄一遍（以前三份各抄一遍，改一处漏两处）。
  停站表在详情里，行高/表头高都不同，仍走自己的 `stops_table_ui`。
- **自适应列宽按「布局宽度」算**（`table_layout_width` = 当前可用宽度 + 右侧临时面板占的宽度，
  `body` 铺完 `detail_right` / `via_right` 后记进 `ui.data`）：打开、关闭详情、拖详情分隔线
  都只让结果表多出横向滚动，列集合与列宽都不变，三张表表现一致；只有窗口尺寸真变了
  才重排（`VIEWPORT_SLACK`）。停站表在详情里，按自己的可用宽度走 `column_viewport`。
- **列宽记忆**：四张表（`od_results` / `train_results` / `station_board` / `train_stops`）的
  宽度工作副本在 egui 界面状态里（`(salt, "widths")`），`persist` 时读出来取整写进
  `columns.widths.<表>`，启动时 `seed_column_widths` 塞回去。列序变了按列名把宽度重排
  （`(salt, "widths_keys")` 记住上一帧的列序），**不要**整份丢掉——用户刚调好的宽度
  不能因为换个位置就回到默认；只有列集合真的换了、或可视宽度变化超过 `VIEWPORT_SLACK`
  才重算。新增可拖列宽的表时，往 `WidthTable` 里加一项，别另起一套。
- **表头拖动换序**：`table_header_drag` 只把新顺序写进 `(salt, "column_order")`，
  由 `LltApp::apply_column_order` 下一帧落回 `*_columns`；拖动 id 用列名而不是下标
  （换完序这列会挪到别的格子，用下标拖动会断）。触摸屏要和调宽一样先长按。
  拖动中的列靠 `table_active_column`（读 `resize_active` 与 `head_drag`）加粗变强调色，
  和调宽共用一套提示。
- **宽度阈值集中**在 `theme`：`WIDE_MIN_WIDTH`、`STACKED_DETAIL_MAX_WIDTH`、
  `INLINE_DATE_MIN_WIDTH`、`TRAIN_COLUMNS_MIN_WIDTH`、`STATION_COLUMNS_MIN_WIDTH`、
  `OD_COLUMNS_MIN_WIDTH`。新增响应式列时从这里取，不要写字面量。

## 修改约束

- 优先进行局部、渐进式修改。
- 不重写已经正常工作的功能。
- 不改变 SQL 查询逻辑、日期换算及开行判断。
- 不改变查询结果、详情信息和数据含义。
- 不随意修改数据库结构及数据访问方式。
- 不删除现有功能以简化界面。
- 如确需改变布局或交互，应保证原有能力不丢失。

## 验证要求

每次 UI 修改后：

1. 执行 `cargo fmt`。
2. 执行 `cargo check`。
3. 运行应用，检查实际界面。
4. 截图并检查视觉效果。
5. 重点检查字号、对齐、间距、布局及溢出。
6. 检查宽屏和窄屏下的显示效果。
7. 发现问题后继续修改并重新截图。

不能仅以编译通过作为 UI 修改完成的标准。

需要按坐标点击、读界面树 / 控件矩形、或以代码的固定尺寸改窗口时，不必依赖 Win32 脚本
（远程会话里合成鼠标经常不生效）：用 `cargo run -p llt-gui --features inspection` 配合
`egui-mcp`，用法见 [`../scripts/debug/README.md`](../scripts/debug/README.md)。
注意点表格**行**要落在单元格的空白处（文字标签会把点击吃掉）。

## 完成标准

- 整体视觉风格统一。
- 字体、字号和字重协调一致。
- 各区域排版整齐、对齐准确。
- 查询结果清晰易读。
- 列车详情信息层次明确。
- 不同窗口尺寸下布局合理。
- 原有功能和交互保持正常。