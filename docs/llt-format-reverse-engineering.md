# 路路通离线时刻表数据格式逆向报告

对象：`com.lltskb.lltskb` 5.3.5 / 数据 20261105。目的：学习私有二进制格式的还原方法与强度评估。
文中行数、字节数等具体数字是 20261105 快照的测量值；后继数据包（如 20261108）只有条目数量变化，
格式本身未变。

## 1 结论

- **不是加密**：无密钥、无 IV、无 MAC，站名/车次明文 UTF-8；匿名 HTTP 即可下载。
- **难读的四条原因**（都不是密码学）：无文档、无魔数、无版本号；`base255`/`base128` 非标准编码；代码混淆 + 加固壳；字段含义依赖业务语义。
- **强度**：混淆级（security through obscurity）。完整规格还原成本约数小时；数据层无校验，可被篡改而客户端不报错。

## 2 结构

| 层 | 内容 | 关键事实 |
|---|---|---|
| 分发 | `http://down.lltskb.com/an.db`（`android.ver` 的 `dataUrl`） | 实为 ZIP，64 条目，3,171,176 B → 8,045,313 B（2.54×） |
| 索引 | `s.i` / `t.i` | `u16be count` + `count×(u16be len + UTF-8)`，即 Java `writeUTF` |
| 分片 | `T0..T19.dat`、`S0..S9.dat` | 按「编号 % N」路由，片内 key 升序、步长恒定 |
| 记录 | `[key 2B][head 15B][stop×N 7B]` | 变长但可精确切分，扫完恰好等于文件长度 |

设备侧也可 `run-as` 导出（需改包 debuggable），与分发包同名文件 sha256 一致，但只有 40 个（App 被加固壳 `SIGKILL` 打断）。

## 3 编码原语

| 原语 | 定义 | 用途 | 实测 |
|---|---|---|---|
| `base255` | `b0*255 + b1`，字节 ≤ 0xFE | 车次号、站号、停站数、里程 | `T0.dat` 中 0xFF 出现 **0** 次，0x00 占 16.3% |
| `base128` | `b0 + b1<<7 + b2<<14 + b3<<21`（定长 LEB128） | `YYYYMMDD` | 20260408 / 20991231 |
| 字符串 | `u16be len + UTF-8` | 站名、车次名 | `s.i` 32,031 B / 3,326 站；`t.i` 116,646 B / 13,745 车 |

`base255` 用 255 而非 256 的取舍：牺牲 1 bit 表达力，换取 **0xFF 恒不出现**（可作哨兵，便于 C 侧 `memchr` 扫描），而 65,024 上限对站号/里程/停站数绰绰有余。

## 4 规格

**`.i`**：`u16be count` + `count×{u16be len; UTF-8}` → 编号↔名称。

**`T{N}.dat`**（车次 `i` 在 `T[(i+1)%20]`）：

```
key    2B  base255  → 车次编号
head  15B  [0]车型id(高4位 新/空调, 低4位 0普快 1快/特快 2普客 3动车 4,6高速 7市郊)
           [1][2]保留   [3..6]开行起 YYYYMMDD   [7..10]开行止   [11..12]未知v3   [13..14]停站数N
stop  N×7B [0..1]base255 站号 | [2]时 | [3]分 | [4]停留分钟 | [5..6]base255 累计里程km
```

时刻语义：首站 `[2][3]` 为发车；中间站为到达，发车 = 到达 + `[4]`；末站为到达。时间 00:00–23:59 循环，跨天无字段，需自行进位。

**`S{N}.dat`**（车站 `j` 在 `S[(j+1)%10]`）：`key 2B 站号 | count 2B | count×2B 车次号`。

**`jlb.dat`**（`JlbDataMgr`，594,796 B）：交路/编组/担当/餐车

```
S1 int32 n;  n×{i32 起 YYYYMMDD, i32 止, i32 v1, i32 v2, b255 企业id, 1B len+UTF8 编组, 1B len+UTF8 交路}
S2 int32 n;  n×{i32 车次编号, i32 起, i32 止, 1B len+UTF8 餐车}
```

交路串形如 `DJ5904#G549#G748#G1049`（`#` 分段＝一昼夜内的套跑链，`/` 为同车次方向变体）；
命中规则是把每段拆开后用「段首车次号 == 查询车次」匹配，并用 `起<=查询日<=止` 过滤。
企业 id 指向 `extra.dat` 首段的企业/担当段表（373 条）。

**`sn.dat`**（`FuzzyStationMgr`，9,128 B）：同城车站 / 站名别名 / 城市名节点

```
S1  short n1;  n1 × writeUTF（u16 长度 + UTF-8）   城市/地区名，虚拟节点 id 从 10000 起（73 条）
S2  short n2;  n2 × { short gid; 1B k; k × short 车站索引 }        同城组（658 组）
S3  short n3;  n3 × (1B len + UTF-8 "站名-电报码")                  电报码别名（364 条）
```

- `gid` 是组的规范节点：车站索引（< `s.i` 条数）或 S1 的虚拟 id（≥10000，代表"城市名"）
- **`gid` 自身不出现在自己的成员向量里**，`OooO0Oo()` 查询时把它插到首位
- `ResMgr.getCity(name) = getStationName(OooO0O0(getStationIndex(name)))`，`OooO0O0()` 就是"找包含该站的组的 gid"
- 「模糊站名」勾选后按站搜索，候选即该组的成员
- 覆盖：1,406 / 3,331 站（42%）落在某个组里；有 108 对互为镜像的组（如天津的 `192` 与 `2310`，成员仅差自身），因此同一城市可能出现两个 gid

**`routes.dat`**（`RoutesMgr`，1,289,137 B）：线路经由 + 车次经由

```
S1 int32 nLines;  nLines × { int32 lineCode, 1B len+UTF8 lineName, int32 nItems,
                             nItems × {1B len+UTF8 车站名, 1B len+UTF8 电报码,
                                       int32 routeFlag, int32 里程} }
S2 int32 nRoutes; nRoutes × { 1B len+UTF8 车次内部编号, 1B len+UTF8 终到站,
                             int32 nPoints,
                             nPoints × {1B len+UTF8 站名, int32 lineCode} }
```

- 线路 796 条（code 0 是占位模板行，需跳过），最长 `沪昆线` 136 站
- 车次经由 14,766 条，key 是 **`sp.dat` 的内部车次编号**（`D6420/D6421 → 5c000D642005`），不是车次号
- 车次经由只存「换线点」序列；完整站点由 `getRoutes()` 在线路表上按区间展开

**`sp.dat`**（`SpDataMgr`，354,250 B，六段顺序排列）：

```
S1 int32 n;  n×16B {i32 a, i32 b, i32 c, b255 d, b255 e}   多方案开行表(TSDD)，12 条
S2 int32 n;  n×(1B len + UTF-8)                            车次内部编号 ↑ 索引=车次编号，14,889 条
S3 b255 n;   n×(b255 id + 1B len + UTF-8)                  省 id→省名，33 条
S4 int32 n;  n×(i32 站号 + b255 市id + b255 省id)           车站→行政区，3,329 条
S5 int32 n;  n×(b255 站号 + 1B len + UTF-8)                 车站→"省 市 区"，3,312 条
S6 int32 n;  n×(1B len + UTF-8 "站名,城市")                 站名→城市别名，677 条
```

## 5 逐字节示例

`T0.dat` 首记录 = 车次 `4081`（齐齐哈尔→北安，9 站）：

```
00 13 | 00 00 00 38 4c 54 09 04 52 54 09 00 00 00 09 | 07 3f 0f 14 00 00 00 | 02 c1 0f 1d 02 00 0a
```

| 片段 | 计算 | 结果 |
|---|---|---|
| `00 13` | 0×255+19 | 车次编号 19 → `t.i[19]="4081"` |
| `00` | 车型 | 新空调普快 |
| `38 4c 54 09` | 56+76·128+84·128²+9·128³ | 开行起 20260408 |
| `04 52 54 09` | 4+82·128+84·128²+9·128³ | 开行止 20261124 |
| `00 09` | 停站数 | 9 |
| `07 3f` | 7×255+63 | 站号 1848 齐齐哈尔，`0f 14 00` = 15:20 停留 0，`00 00` = 0 km |
| `02 c1` | 2×255+193 | 站号 703 高头，15:29 停留 2，10 km |

## 6 分片路由

实测：`T0` 记录 687 条、key 19,39,59…（`%20` 恒为 19）；`S0` 332 条、key 9,19,29…（`%10` 恒为 9）。片内 key 严格升序、步长恒定，因此可预建 offset 表得 O(1)；App 选择线性扫描 + 命中即返回（O(687)，几十微秒），代价是每片整体读入 `byte[]`，启动常驻约 1.8 MB。

体积账：核心 `T+S+i` 1,964,982 B，均摊 143 B/车，最大分片 `T7.dat` 57,172 B。

## 7 语义锚点

布局能从 `DataMgr` 读出，语义必须靠 UI 锚定：

| 锚点 | 推出 |
|---|---|
| `R.string.fmt_result_stop_time` + `getIndex(5)` | 第 4 字节 = 停留分钟 |
| `getIndex(3)`→列3 / `getIndex(4)`→列2 | 一到一发两列 |
| `"已到站\r\n%d 公里"` | `[5..6]` 是里程，不是时间 |
| `iArr`/`iArr2`、`bArr2[0]==-1` | 首/末站特例；正常数据走 `else` 分支 |

两个典型误判：把里程当累计分钟（同样单调递增，靠"公里"文案才纠正）；只读 `if` 的第一个分支（有临客表时是另一套字段）。

## 8 验证

| 级别 | 方法 | 结果 |
|---|---|---|
| 结构 | 20 分片全部恰好消费完；车次号 0…13,744 无重复无缺失 | 通过 |
| 外部 | 与公开时刻/里程比对 | G1 1318 km、1461 1463 km、Z263 4974 km，全部命中 |
| 统计 | 里程单调、跨天合理 | 停站 2–49（中位 7）；跨天 1,426 趟；仅 2 条里程非单调（环线，合理） |

`S*.dat` 与 `T*.dat` 交叉：2,862/3,326 站完全一致，其余差异来自同名站归并（`sn.dat` + `FuzzyStationMgr`）。

## 9 安全评估

| 检查项 | 本数据 |
|---|---|
| 密钥 / IV / AEAD | 无 |
| 完整性（CRC/MAC/签名） | 无，篡改后静默错位不报错 |
| 机密性 | 无，明文 UTF-8 可直接 grep |
| 压缩 | 仅分发层 ZIP deflate |
| 混淆 | `OooO0O0` 命名、`libinno`/`libsec` 加固壳、反调试 `SIGKILL`（保护实现，不保护数据） |

判定：违反 Kerckhoffs 原则，格式即唯一秘密。真正值得担心的不是"被抄数据"，而是**无完整性 → 可注入伪造时刻表**，客户端逐字节信任。

## 10 若要真保护

| 方案 | 机密性 | 防篡改 | 1.8 MB 成本 |
|---|---|---|---|
| 记录级 HMAC（8B/记录，约 +110 KB） | — | ✅ | 最小改动，优先做 |
| 分片 AEAD（AES-GCM/nonce per shard） | ✅ | ✅ | 解密 <5 ms（AES-NI） |
| SQLCipher（换 SQLite） | ✅ | ✅ | 体积 +30–50% |
| 整包 AES + 固定密钥 | 弱（密钥在 APK） | ❌ | 无意义 |

另需：文件头加魔数与版本号（让解析失败显式报错）、密钥服务端下发并绑定设备/轮换。
作者当年不加密的合理约束：2013 年起家，客户端横跨 Java ME/S60/PPC/Android/iOS，无可信密钥存储，且追求 "320 KB 装下全国数据、老机秒开"。

## 11 复现

```bash
curl -o /tmp/an.db http://down.lltskb.com/an.db
unzip -o /tmp/an.db -d out/data/an          # 包内是大写 T0.dat
python tools/decode_llt.py -d out/data/an -o out/timetable
python tools/decode_sp.py                      # sp.dat：车次内部编号 + 车站行政区
python tools/decode_sp.py --dump out/timetable/sp
python tools/decode_jlb.py                     # jlb.dat：交路/编组/担当/餐车
python tools/decode_jlb.py --dump out/timetable/jlb
python tools/decode_routes.py                 # routes.dat：线路经由 + 车次经由
python tools/decode_routes.py D6420/D6421     # 打印某车次逐点经由与里程
python tools/decode_routes.py --dump out/timetable/routes
python tools/decode_sn.py 北京               # sn.dat：同城车站
python tools/decode_sn.py --dump out/timetable/sn
python tools/check_update.py                   # 比对线上 android.ver 与本地 ver.txt
```

Rust 版（`cli/`）把上述全部内容一次写入同一个 SQLite：

```bash
cargo run -p llt --release -- build -i out/data/an -o out/timetable
cargo run -p llt --release -- query train G1        # 车次 + 交路 + 经由
cargo run -p llt --release -- query stops G1        # 停站表 + 交路 + 逐点经由
cargo run -p llt --release -- query station 北京    # 车站时刻表 + 同城车站（sn.dat）
```

对应的附加表为 `lines` / `line_stations`（线路）、`train_routes`（车次经由展开）、
`duties` / `duty_segments`（交路）、`dining_cars`（餐车）、
`city_nodes` / `same_city_groups` / `same_city_members`（同城车站）、`tele_aliases`（电报码），
解析实现见 `cli/src/routes.rs`、`cli/src/duty.rs`、`cli/src/spdata.rs`、`cli/src/samecity.rs`。

```python
b255 = lambda b, o: (b[o] & 0xFF) * 255 + (b[o+1] & 0xFF)
b128 = lambda b, o: sum((b[o+i] & 0xFF) << (7*i) for i in range(4))

def parse_shard(d, stations, trains):
    o, rows = 0, []
    while o + 17 <= len(d):
        key, hdr, n = b255(d, o), d[o+2:o+17], b255(d, o+15)
        o += 17
        stops = [(b255(d, o+7*k), d[o+7*k+2], d[o+7*k+3], d[o+7*k+4], b255(d, o+7*k+5))
                 for k in range(n)]
        o += 7 * n
        rows.append((trains[key], stops))
    return rows
```

## 12 附：「开行计划」的数据来源

车次详情页那条月度日历（绿=开行 / 灰=停开）**完全由本地数据算出**，不需要网络：

```
T{N}.dat 车次头 [3..6]开行起 [7..10]开行止 [11..12]ruleIndex   ← 已确认即 t.rule 行号
t.rule   131 行 "周期 掩码"，第 k 行对应 ruleIndex=k
sch.dat  临时加开/停运覆盖表（本数据集 0 字节 → 恒无覆盖）
         ↓
Rule.outOfDate(minDate, maxDate, date, ruleIndex, auxDay)      engine/Rule.java
  1) date+offset < 开行起 → 未开行；> 开行止 → 已过期
  2) n=(date+offset-开行起).days；(j,mask)=t.rule[k]；
     j!=1 且 ((1<<(n%j)) & mask)==0 → 该日不开行
         ↓
RunChartTopLayout / RunChartView（5×7 日历渲染，同时用于列表「当日不开行」着色/隐藏）
```

`offset`(auxDay) 是「以始发站日期计算」的天数补偿。

**在线覆盖（仅特定条件下）**：`POST https://mobile.12306.cn/wxxcx/wechat/bigScreen/queryTrainDiagram`，
表单 `trainCode` + `queryDate`(yyyy-MM-dd，取查询日-15)，返回 `data.running_list[].{date,flag}`（flag=="1" 表示开行），
由 `StTrainInfo.queryTrainDiagram()` 转成 `Map<date,flag>`，再经 `RunChartView.setTrainDiagram()` 覆盖本地判定。
是否采用在线结果由 `ResultMgr.isUseLocalRunStatus()` 决定：只有 `QueryCC` 发现「12306 返回的车次号 ≠ 本地车次号」时才关闭本地模式
（正是 V/N 改号场景）。

**实测验证**（20261108 数据）：

| 证据 | 结果 |
|---|---|
| 断网（`svc wifi/data disable`）后重开车次详情页 | 开行计划日历照常全绿；底部「车次状态」报网络异常、列车信息块消失 → 日历是本地 |
| 规则预测 vs 列表 | `G531` ruleIndex=89 → `t.rule=(7,15)`，2026-11-03 应停开、11-02 应开行；App 中 11/3 列表确无 G531（首行 G547 06:18），切到 11/2 后 G531（06:08）出现在首行 |
| 车次头日期 | `K54` 开行止 20261124 → 11/25 起判定「已过期」，与改革日吻合 |

复刻工具：`python tools/runchart.py G531 20261101 20261120`（打印逐日开/停）。

落到 SQLite 里是两处：`trains.rule_index`（车次头 [11..12] 的 base255）与
`train_rules(rule_index, period, mask)`（`t.rule` 的 131 行）。判定实现在
`lltdb::date::run_state`：GUI 的月历、列表的「当日开行」圆点、CLI 的 `--date` 过滤
都调它，口径只有一份。

### sp.dat：为什么需要「车次内部编号」

`T{N}.dat` 里只有车次号（`K54`），而 12306 的接口用内部编号 `train_no`（如 `K54 → 1200000K540D`）。
`sp.dat` 的 S2 就是这张映射：**索引 = 车次编号**，值 = 12306 `train_no`，与 `t.i` 一一对应（14,889 条）。

格式为 `[2 位局码][0 填充][车次号][2 位方案后缀]`，总长固定 12；后缀可区分同车次的多套方案
（`D932/D933`→`700000D9330B` vs `D932/D933B`→`700000D9330C`）。

实测校验（`POST https://mobile.12306.cn/wxxcx/wechat/bigScreen/queryTrainDiagram` 返回 `subbureau_code`）：

| 车次 | sp.dat 前缀 | 12306 `subbureau_code` |
|---|---|---|
| G1 / 1461 / K411 | `24` | 24（北京） |
| K54 | `12` | 12（沈阳） |
| Z263 | `88` | 88（青藏公司） |
| D54 | `62` | 62（长沙公司） |
| D933 | `70` | 70（贵广公司） |
| V1009 | `1g` | 1g（哈大沈段） |

用途即 `QueryCC` 里的那句比较：`12306 返回 trainNo == sp[trainIndex]` 为真才用本地开行计划，
否则（如 K54 被改成 V54）切到在线 `running_list`。注意该接口**未登录时 `running_list` 为空**
（`noLogin:"Y"`），所以实际呈现仍以本地规则为主。


### t.rule：ruleIndex 是「周期模板」，不是「日历模式」

```
t.rule 每行 "<周期 j> <位掩码 mask>"，第 k 行即 ruleIndex=k（131 行，两两不同）
判定：outOfDate(minDate, maxDate, date, ruleIndex, auxDay)
  d   = date - auxDay                # auxDay：按始发站日期补偿的天数
  d < minDate → 1 未开行；d > maxDate → 2 已过期
  n   = (d - minDate).days
  if j != 1 and (1 << (n % j)) & mask == 0 → 3 规则停开    # LLTUtils.pow(x) == 1<<x
  否则 → 0 开行
```

**相位起点是每条车次自己的 `minDate`（开行起始日），不是固定的日历锚点**。所以：

- `ruleIndex → (j, mask)` 是**确定且唯一**的（131 组参数互不重复，128 组被使用，57/58/59 空闲）
- 但 `ruleIndex → 真实开行日` **不确定**：起始星期不同的车次复用同一 ruleIndex 时，真实星期集合会平移
  - 实测：周期>1 的 127 条规则中，**33 条**在数据里出现 ≥2 种真实模式
  - 例：`ruleIndex=76`（`7 1`）被 45 趟车共用，起日是周日/周五/周一/周六的车次分别在**日/五/一/六**开行
  - 例：`ruleIndex=114`（`7 7`）134 趟车 → `五六日`102、`六日一`20、`三四五`11、`四五六`1
- 决定性的是三元组 `(ruleIndex, minDate, auxDay)`；对单条车次而言结果唯一

规则分三类（`out/timetable/rules.csv` 有全表）：

| 类型 | 条件 | 条数 | 涉及车次 | 语义 |
|---|---|---|---|---|
| 每天 | `j == 1` | 1（`0 1 1`） | 13,774 | 开行区间内天天开 |
| 星期制 | `j == 7` | 50 | 767 | 位序号相对起日星期平移 |
| 周期制 | 其它（`j` 2…31） | 80 | 348 | 如 `j=10, 开位 0 2 7 9` = 开1停1开1停4开1停1 的 10 日循环 |

验证：`G531` = ruleIndex 89 → `7 15`（位 0..3）；起日 2026-07-03 是**周五**，故开行日 = 五六日一。
App 中 11/2（周一）有 G531、11/3（周二）无 → 与「相对起日」一致；若按「位=星期」解释应为一~四开行，与 App 矛盾。


### 数据库更新推送：只有一条 HTTP 接口

| 项 | 值 |
|---|---|
| URL | `http://down.lltskb.com/android.ver` |
| 方法/编码 | GET，明文 HTTP，**无参数、无鉴权**，JSON（含未转义值，靠 Gson lenient 解析） |
| 触发 | ① 启动后若距上次检查 > 2 天（`172800000 ms`）或本地无版本 → 静默检查；② 设置 → 检查新版本；③ 首页右下角「版本:xxxx」 |
| 解析 | `UpgradeModel.checkUpdate` → Gson → `AppVersion`；`minSdk > Build.VERSION.SDK_INT` 直接丢弃 |

响应字段与判定（`UpgradeModel.OooO0oo / OooOO0o / OooOOO0`）：

```
version { app, desc, data, minSdk }        app/data 都是字符串，直接按字符串比较大小
appUrl  [ ... ]      APK 下载地址（按序回退）
dataUrl [ ... ]      数据包下载地址（同上，当前首项 http://223.107.87.50:8011/an.db）
market  [ { name, id, version{...} } ]    按已安装的应用商店再判一次

判 APK 更新： version.app  > 本地 app 版本        → 弹「发现新数据??」否，走 APK 分支
判数据更新： version.data > 本地 ver.txt 内容     → 弹「发现更新 / 发现新数据：<data>」
            且优先 APK 更新；DebugModel.isForceDataUpdate() 可强制
```

弹窗（`UpgradeHintDialog`）：标题 `update_found`「发现更新」，正文
`new_data_found`「发现新数据，现在更新吗？」+ `

` + `version.data`，按钮「更新 / 取消」；
若是 APK 分支则正文为 `fmt_app_upgrade_hint`「发现程序新版本: %1$s
%2$s」（app 版本 + desc）。

点「更新」后（`UpgradeModel.OooOOoo`）：按 `dataUrl` 顺序下载到 `<files>/lltskb.db` →
`LLTUtils.upZipFile(...,ResMgr.mPath)` 就地解包（即前面分析的 64 个 `.dat`）→ 删除压缩包 → 刷新 `ver.txt`。

复刻脚本：`python tools/check_update.py [--local-app 5.3.5] [--json]`，输出与 App 同口径的结论与弹窗文案。

实测：当前线上 `version.data = 20261108`，本机 `ver.txt = 20261108` → 手点「检查新版本」返回
`no_update_found`「没有发现更新。」；同一响应里 `version.app = 5.3.7` > 已装 5.3.5，故 APK 分支本应提示，
只因 `market` 里匹配到本机应用商店（VIVO）且该渠道条目版本不高于本地而未触发。
脚本用 `--local-app 5.3.5` 复现时给出「有 APK 更新（5.3.5 → 5.3.7）」。
注：响应不是严格 JSON（`"name": 应用宝",` 缺左引号且多一右引号），脚本按行做了最小修补。

### sn.dat：同城车站实测

北京（车站索引 165）→ 组 `gid=165`，成员 10 + 自身 = 11 站：

```
北京 · 北京北 · 北京朝阳 · 北京大兴 · 北京东 · 北京丰台 · 北京南 · 北京西 · 昌平北 · 清河 · 亦庄
```

与界面观察到的 9 站相比，多出 **北京东**、**亦庄**（均为北京市郊铁路城市副中心线车站）。
天津 → 镜像组 `192(滨海)` / `2310(天津)`，两者成员仅差自身，并集 12 站
（天津、天津北、天津南、天津西、滨海、滨海北、滨海西、塘沽、军粮城北、北辰、武清、杨柳青）。

另注：`s.i` 里没有「成都」这一站（站改中），但 S1 有城市名节点 `10004(成都)`，
其成员为 成都东 / 成都南 / 成都西 / 犀浦 —— 即"城市名搜站"走的是 S1+S2 这条路径。

### routes.dat：经由页怎么拼出来

「经由」页的每一行 = 线路表里的一个节点。展开算法（`RoutesMgr.getRoutes`）：

```
对换线序列的每对相邻点 (a→b, 线路 L)：
  在 L 的车站数组里定位 a、b 的下标 → 切片（逆序则翻转）
  里程 = 上一节点累计里程 + |本节点在 L 内的里程 - 上一节点在 L 内的里程|
  区间首节点里程 = 上一区间末节点的累计里程
相邻同名站合并，线路名拼成 "A/B"（换线点会同时属于两条线）
```

实测 `D6420/D6421`（key `5c000D642005`）：22 点、龙岩→平潭、456 km，与 App 页面逐行一致

| 站点 | 线路 | 里程 | App 显示 |
|---|---|---|---|
| 龙岩 | 南龙线 | 0 | 龙岩 / 南龙线 / 0公里 |
| 湖一所 | 南龙线 | 5 | 5公里 |
| 南石所 | 南龙线 | 16 | 16公里 |
| 雁石南 | 南龙线 | 29 | 29公里 |
| 漳平西 | 南龙线 | 52 | 52公里 |
| … | | | |
| 延平 | 南龙线/合福高速线 | 247 | （换线点，双线名） |
| 平潭 | 福平铁路 | 456 | 456公里 |

全量自检（14,766 条）：里程 100% 单调、100% 起于 0、无空结果；
末点里程与 `t*.dat` 末站里程交叉一致（G1 1318、G549 1318、K54 728、1461 1463、Z16 1249、D6420/D6421 456）。

### jlb.dat 实测对照

| 项 | 本地解码 | App「交路」弹窗 |
|---|---|---|
| 担当企业 | 济南局济南段（qiye_id→extra.dat 企业表） | 担当企业：济南局济南段 |
| 编组/车型 | `CR400AF-A` | 车型信息：CR400AF-A |
| 交路链 | `DJ5904#G549#G748#G1049` | DJ5904 → G549 → G748 → G1049（逐段补齐起终站与时刻） |
| 餐车 | `9车无餐座`(20260615~20991231) | 餐车车厢：9车无餐座 |

顺带解出：S3 省表含「老挝」；S4/S5 提供车站行政区（3,312 站有"省 市 区"，19 站缺）；
S6 677 条站名→城市别名；S1 的 TSDD 是 12 条"同车次多方案"记录（D932/D933 系列、K411/K412）。

## 13 附录

**未解字段**：`head[1..2]`（恒 0，疑预留）；`head[11..12]` v3（多 0，疑票价/编组模块 id）；`sch.dat`/`ts.dat` 为 0 字节占位（临客/停运覆盖表，非空时才走另一分支）。

**踩坑**：① `uninstall -k` 后换签名安装被拒（`INSTALL_FAILED_UPDATE_INCOMPATIBLE`），Android 上无法"保留数据改包"；② AXML 插 `debuggable` 必须按 resId 升序且补 namespace/resmap，否则系统无视；③ 加固壳检测 debuggable 后 `SIGKILL`（`reason=SIGNALED status=9`），与数据格式无关；④ 包内 `T0.dat` 大写、代码用小写，Windows 大小写不敏感掩盖了 bug；⑤ 单调字段最易误判（里程 vs 累计分钟）；⑥ 单样本验证不可靠，须"全文件消费完 + 全量统计"双重校验。

**可迁移结论**：判"加密"只看密钥与认证，不看是否看得懂；解析顺序固定为 framing → 原语 → 路由 → 语义；唯一可信验收是"能预测未见事实"（G1 里程对上那一刻）。
