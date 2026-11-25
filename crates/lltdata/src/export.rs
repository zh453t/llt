//! 导出：SQLite（默认）或 CSV（`--csv`）。
//!
//! `stations` / `trains` / `stops` 三张表与 `tools/decode_llt.py` 一致（`stops` 另多
//! `platform` 列，写站台），另外还会写入附加表：
//!
//! ```sql
//! meta         (key, value)
//! lines        (line_code, line_name)                     线路
//! line_stations(line_code, seq, station, tele_code, flag, km)
//! train_routes (train_index, seq, station, line, km,
//!               is_station, station_index)                车次经由（已展开）
//! duties       (duty_id, start_date, end_date, company, formation, chain)   交路
//! duty_segments(duty_id, seq, label, code)
//! duty_trains  (train_index, duty_id, seq)          交路段 -> 车次（构建时解析）
//! dining_cars  (train_index, start_date, end_date, coach)
//! city_nodes      (node_id, city)                      城市名虚拟节点
//! same_city_groups(group_id, group_name, head_station_index, is_city_node)
//! same_city_members(group_id, station_index, station, seq)
//! tele_aliases (station, tele_code)                    站名-电报码
//! ```

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::decode::Dataset;
use crate::extras::Extras;
use crate::samecity;

pub const SQLITE_FILE: &str = "llt-timetable.sqlite";

/// `meta.schema_version`，表结构变化时 +1
/// （2 = 经由 / 交路 / 餐车；3 = 经由点标注是否为车站；4 = 同城车站；
/// 5 = 停站增加 `display_train` / `platform`）。
pub const SCHEMA_VERSION: i64 = 5;

/// SQLite `application_id`：`"LLT"` + 版本 1，用于识别本工具生成的库。
const APPLICATION_ID: i64 = 0x4C4C_5401;

/// 写进 `meta` 表的数据包来源信息。
#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// 数据包里的 `ver.txt`（YYYYMMDD），即官方数据版本；缺失时为 `None`。
    pub data_version: Option<String>,
    /// 生成时间（RFC3339，UTC）。
    pub built_at: String,
    /// 下载地址，或本地输入路径。
    pub source: String,
    /// HTTP `Last-Modified`（只有下载时有）。
    pub source_last_modified: Option<String>,
    /// HTTP `ETag`（只有下载时有）。
    pub etag: Option<String>,
    /// 数据包字节数。
    pub archive_bytes: Option<u64>,
    /// 线上版本接口声明的数据版本（用于对照 `ver.txt`，可选）。
    pub remote_data_version: Option<String>,
    /// 线上 APK 版本（用于提醒解码器是否需要适配）。
    pub remote_app_version: Option<String>,
    /// 版本接口地址。
    pub version_source: Option<String>,
}

impl Meta {
    /// `20261108` -> `2026-11-08`，格式不对时返回 `None`。
    pub fn data_date(&self) -> Option<String> {
        let version = self.data_version.as_deref()?;
        if version.len() != 8 || !version.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let month: u32 = version[4..6].parse().ok()?;
        let day: u32 = version[6..8].parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        Some(format!(
            "{}-{}-{}",
            &version[0..4],
            &version[4..6],
            &version[6..8]
        ))
    }

    /// 数据版本作为 `PRAGMA user_version` 整数（YYYYMMDD）。
    pub fn data_version_int(&self) -> Option<i64> {
        let version = self.data_version.as_deref()?;
        if version.len() != 8 || !version.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        version.parse().ok()
    }
}

/// Python 的 `csv.writer` 用 utf-8-sig，这里保持一致（Excel 打开不乱码）。
const BOM: &[u8] = b"\xEF\xBB\xBF";

const STATIONS_CSV: &str = "stations.csv";
const TRAINS_CSV: &str = "trains.csv";
const STOPS_CSV: &str = "stops.csv";
const TRAIN_ROUTES_CSV: &str = "train_routes.csv";
const DUTIES_CSV: &str = "duties.csv";
const DINING_CARS_CSV: &str = "dining_cars.csv";
const SAME_CITY_CSV: &str = "same_city.csv";
const CITY_NODES_CSV: &str = "city_nodes.csv";
const TELE_ALIASES_CSV: &str = "tele_aliases.csv";
pub const META_JSON: &str = "meta.json";

const DDL: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE stations (station_index INTEGER PRIMARY KEY, station TEXT NOT NULL);
CREATE TABLE trains (train_index INTEGER PRIMARY KEY, train TEXT NOT NULL,
    train_no TEXT, type TEXT,
    date_from INTEGER, date_to INTEGER, total_km INTEGER, days INTEGER,
    first_station TEXT, last_station TEXT,
    -- t.rule 的行号：配合 date_from 推出每一天开不开行。
    rule_index INTEGER);
-- 开行周期（t.rule）：period 天一轮，mask 的第 k 位为 1 表示第 k 天开行。
CREATE TABLE train_rules (rule_index INTEGER PRIMARY KEY,
    period INTEGER NOT NULL, mask INTEGER NOT NULL);
CREATE TABLE stops (train_index INTEGER, train TEXT, display_train TEXT, seq INTEGER,
    station_index INTEGER, station TEXT, km INTEGER, arrive TEXT, depart TEXT,
    dwell_min INTEGER, day INTEGER, arrive_min INTEGER, depart_min INTEGER,
    platform TEXT,
    PRIMARY KEY (train_index, seq));
CREATE INDEX stops_station ON stops(station_index);
CREATE INDEX stops_train ON stops(train_index, seq);
CREATE INDEX trains_no ON trains(train_no);
CREATE INDEX trains_code ON trains(train);
CREATE TABLE lines (line_code INTEGER PRIMARY KEY, line_name TEXT NOT NULL);
CREATE TABLE line_stations (line_code INTEGER NOT NULL, seq INTEGER NOT NULL,
    station TEXT NOT NULL, tele_code TEXT, flag INTEGER, km INTEGER,
    PRIMARY KEY (line_code, seq)) WITHOUT ROWID;
CREATE TABLE train_routes (train_index INTEGER NOT NULL, seq INTEGER NOT NULL,
    station TEXT NOT NULL, line TEXT NOT NULL, km INTEGER NOT NULL,
    -- is_station 是 station_index 的派生值：有下标就是车站，没有就不是。
    is_station INTEGER NOT NULL CHECK (is_station IN (0, 1)),
    station_index INTEGER,
    CHECK ((is_station = 1) = (station_index IS NOT NULL)),
    PRIMARY KEY (train_index, seq)) WITHOUT ROWID;
CREATE INDEX train_routes_station ON train_routes(station_index);
CREATE TABLE duties (duty_id INTEGER PRIMARY KEY, start_date INTEGER NOT NULL,
    end_date INTEGER NOT NULL, company TEXT NOT NULL, formation TEXT NOT NULL,
    chain TEXT NOT NULL);
CREATE TABLE duty_segments (duty_id INTEGER NOT NULL, seq INTEGER NOT NULL,
    label TEXT NOT NULL, code TEXT NOT NULL, PRIMARY KEY (duty_id, seq)) WITHOUT ROWID;
CREATE INDEX duty_segments_code ON duty_segments(code);
CREATE TABLE duty_trains (train_index INTEGER NOT NULL, duty_id INTEGER NOT NULL,
    seq INTEGER NOT NULL, PRIMARY KEY (train_index, duty_id, seq)) WITHOUT ROWID;
CREATE INDEX duty_trains_duty ON duty_trains(duty_id, seq);
CREATE TABLE dining_cars (train_index INTEGER NOT NULL, start_date INTEGER NOT NULL,
    end_date INTEGER NOT NULL, coach TEXT NOT NULL,
    PRIMARY KEY (train_index, start_date, coach)) WITHOUT ROWID;
CREATE TABLE city_nodes (node_id INTEGER PRIMARY KEY, city TEXT NOT NULL);
CREATE TABLE same_city_groups (group_id INTEGER PRIMARY KEY, group_name TEXT NOT NULL,
    head_station_index INTEGER, is_city_node INTEGER NOT NULL
        CHECK (is_city_node IN (0, 1)),
    CHECK ((is_city_node = 1) = (head_station_index IS NULL)));
CREATE TABLE same_city_members (group_id INTEGER NOT NULL, station_index INTEGER NOT NULL,
    station TEXT NOT NULL, seq INTEGER NOT NULL,
    PRIMARY KEY (group_id, station_index)) WITHOUT ROWID;
CREATE INDEX same_city_members_station ON same_city_members(station_index);
CREATE TABLE tele_aliases (station TEXT NOT NULL, tele_code TEXT NOT NULL,
    PRIMARY KEY (station, tele_code)) WITHOUT ROWID;
";

/// 写 CSV（3 张主表 + 经由 / 交路 / 餐车），返回文件路径。
pub fn write_csvs(
    dir: &Path,
    data: &Dataset,
    extras: &Extras,
    meta: &Meta,
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::with_capacity(9);

    let mut csv = CsvFile::create(&dir.join(STATIONS_CSV))?;
    csv.header(&["station_index", "station", "trains_in_station_shard"])?;
    for (index, name) in data.stations.iter().enumerate() {
        let count = data.station_train_counts.get(index).copied().unwrap_or(0);
        csv.record(vec![index.to_string(), name.clone(), count.to_string()])?;
    }
    written.push(csv.finish()?);

    let mut csv = CsvFile::create(&dir.join(TRAINS_CSV))?;
    csv.header(&[
        "train_index",
        "train",
        "train_no",
        "type",
        "date_from",
        "date_to",
        "stops",
        "first_station",
        "depart",
        "last_station",
        "arrive",
        "total_km",
        "days",
        "rule_index",
    ])?;
    for train in &data.trains {
        let (Some(first), Some(last)) = (train.stops.first(), train.stops.last()) else {
            // 与 Python 一致：没有停站的车次不进 trains.csv（sqlite 里仍然保留）。
            continue;
        };
        csv.record(vec![
            train.index.to_string(),
            train.code.clone(),
            train.train_no.clone(),
            train.kind.clone(),
            train.date_from.to_string(),
            train.date_to.to_string(),
            train.stops.len().to_string(),
            first.station.clone(),
            first.depart.clone(),
            last.station.clone(),
            last.arrive.clone(),
            last.km.to_string(),
            last.day.to_string(),
            train.rule_index.to_string(),
        ])?;
    }
    written.push(csv.finish()?);

    let mut csv = CsvFile::create(&dir.join(STOPS_CSV))?;
    csv.header(&[
        "train",
        "display_train",
        "seq",
        "station",
        "station_index",
        "km",
        "arrive",
        "depart",
        "dwell_min",
        "day",
        "platform",
    ])?;
    for train in &data.trains {
        for (index, stop) in train.stops.iter().enumerate() {
            csv.record(vec![
                train.code.clone(),
                stop.display_train.clone(),
                (index + 1).to_string(),
                stop.station.clone(),
                stop.station_index.to_string(),
                stop.km.to_string(),
                stop.arrive.clone(),
                stop.depart.clone(),
                stop.dwell.to_string(),
                stop.day.to_string(),
                stop.platform.clone(),
            ])?;
        }
    }
    written.push(csv.finish()?);

    // ---- 经由 / 交路 / 餐车 ----
    if !extras.vias.is_empty() {
        let mut csv = CsvFile::create(&dir.join(TRAIN_ROUTES_CSV))?;
        csv.header(&[
            "train_index",
            "seq",
            "station",
            "line",
            "km",
            "is_station",
            "station_index",
        ])?;
        for via in &extras.vias {
            for (index, point) in via.points.iter().enumerate() {
                let station_index = extras.station_of(&point.station);
                csv.record(vec![
                    via.train_index.to_string(),
                    (index + 1).to_string(),
                    point.station.clone(),
                    point.line.clone(),
                    point.km.to_string(),
                    u8::from(station_index.is_some()).to_string(),
                    station_index
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                ])?;
            }
        }
        written.push(csv.finish()?);
    }

    if let Some(duties) = &extras.duties {
        let mut csv = CsvFile::create(&dir.join(DUTIES_CSV))?;
        csv.header(&[
            "duty_id",
            "start_date",
            "end_date",
            "company",
            "formation",
            "chain",
            "segments",
        ])?;
        for (id, duty) in duties.items.iter().enumerate() {
            let labels: Vec<&str> = duty
                .segments
                .iter()
                .map(|segment| segment.label.as_str())
                .collect();
            csv.record(vec![
                id.to_string(),
                duty.start_date.to_string(),
                duty.end_date.to_string(),
                duty.company.clone(),
                duty.formation.clone(),
                duty.chain.clone(),
                labels.join(" -> "),
            ])?;
        }
        written.push(csv.finish()?);

        let mut csv = CsvFile::create(&dir.join(DINING_CARS_CSV))?;
        csv.header(&["train_index", "start_date", "end_date", "coach"])?;
        for car in &duties.dining {
            csv.record(vec![
                car.train_index.to_string(),
                car.start_date.to_string(),
                car.end_date.to_string(),
                car.coach.clone(),
            ])?;
        }
        written.push(csv.finish()?);
    }

    write_meta_json(dir, data, extras, meta)?;

    // ---- 同城车站 / 城市名节点 / 电报码 ----
    if let Some(sn) = &extras.same_city {
        let mut csv = CsvFile::create(&dir.join(SAME_CITY_CSV))?;
        csv.header(&[
            "group_id",
            "group_name",
            "station_index",
            "station",
            "is_group_head",
        ])?;
        for group in &sn.groups {
            let name = extras.group_name(group);
            // gid 自身排首位（App 查询时也是这么插的），成员按原顺序跟随。
            for node in group.nodes() {
                let is_head = node == group.id;
                // 城市名节点的「站名」就是城市名；它的节点 id 也写在 station_index 列里
                // （与 tools/decode_sn.py --dump 的输出保持一致）。
                let station = if is_head {
                    name.clone()
                } else {
                    extras.station_name(node as usize).unwrap_or("?").to_owned()
                };
                csv.record(vec![
                    group.id.to_string(),
                    name.clone(),
                    node.to_string(),
                    station,
                    u8::from(is_head).to_string(),
                ])?;
            }
        }
        written.push(csv.finish()?);

        let mut csv = CsvFile::create(&dir.join(CITY_NODES_CSV))?;
        csv.header(&["node_id", "node_name", "members"])?;
        for group in sn.groups.iter().filter(|group| group.is_city_node()) {
            let members: Vec<&str> = group
                .members
                .iter()
                .map(|member| extras.station_name(*member as usize).unwrap_or("?"))
                .collect();
            csv.record(vec![
                group.id.to_string(),
                extras.group_name(group),
                members.join(" "),
            ])?;
        }
        written.push(csv.finish()?);

        let mut csv = CsvFile::create(&dir.join(TELE_ALIASES_CSV))?;
        csv.header(&["station", "tele_code"])?;
        for (station, code) in &sn.aliases {
            csv.record(vec![station.clone(), code.clone()])?;
        }
        written.push(csv.finish()?);
    }

    Ok(written)
}

/// CSV 目录没有 sqlite 的 `meta` 表，另写 `meta.json` 记录版本。
fn write_meta_json(dir: &Path, data: &Dataset, extras: &Extras, meta: &Meta) -> Result<()> {
    let path = dir.join(META_JSON);
    let value = serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "generator": format!("llt {}", env!("CARGO_PKG_VERSION")),
        "data_version": meta.data_version,
        "data_date": meta.data_date(),
        "built_at": meta.built_at,
        "source": meta.source,
        "archive_bytes": meta.archive_bytes,
        "remote_data_version": meta.remote_data_version,
        "remote_app_version": meta.remote_app_version,
        "version_source": meta.version_source,
        "station_count": data.stations.len(),
        "train_count": data.trains.len(),
        "stop_count": data.stop_count(),
        "route_point_count": extras.route_point_count(),
        "duty_count": extras.duty_count(),
        "same_city_group_count": extras.same_city_group_count(),
        "platform_record_count": extras.platform_record_count(),
        "train_change_count": extras.train_change_count(),
    });
    let text = serde_json::to_string_pretty(&value)?;
    std::fs::write(&path, text).with_context(|| format!("无法写入 {}", path.display()))?;
    Ok(())
}

/// 写 SQLite。先写同目录的临时文件，成功后改名，避免留下半个库。
pub fn write_sqlite(path: &Path, data: &Dataset, extras: &Extras, meta: &Meta) -> Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| SQLITE_FILE.to_owned());
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);

    match write_sqlite_at(&tmp, data, extras, meta) {
        Ok(()) => {}
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
    }

    if let Err(err) = replace_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err).with_context(|| {
            format!(
                "无法写入 {}（文件可能正被其他程序占用，例如 llt-gui）",
                path.display()
            )
        });
    }
    Ok(())
}

/// 原子替换文件。
///
/// Windows 上目标库刚被 SQLite 关闭时，句柄释放可能比 UI 那一帧慢一点点；
/// 这里对「拒绝访问 / 共享冲突」重试约 1 秒，避免下载完成后仍报 os error 5。
pub fn replace_file(tmp: &Path, target: &Path) -> std::io::Result<()> {
    let mut last = None;
    for _ in 0..20 {
        match fs::rename(tmp, target) {
            Ok(()) => return Ok(()),
            Err(err) => {
                let retry = matches!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Other
                );
                if !retry {
                    return Err(err);
                }
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other(format!("无法替换 {}", target.display()))))
}

fn write_sqlite_at(path: &Path, data: &Dataset, extras: &Extras, meta: &Meta) -> Result<()> {
    let conn = Connection::open(path).with_context(|| format!("无法创建 {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF;\nPRAGMA synchronous = OFF;\nPRAGMA temp_store = MEMORY;",
    )
    .context("初始化 SQLite 连接失败")?;
    // 文件级标识：application_id 表明是本工具的库，user_version 直接存数据版本。
    conn.pragma_update(None, "application_id", APPLICATION_ID)?;
    conn.pragma_update(
        None,
        "user_version",
        meta.data_version_int().unwrap_or_default(),
    )?;
    conn.execute_batch(DDL)
        .with_context(|| format!("建表失败：{}", path.display()))?;

    let tx = conn
        .unchecked_transaction()
        .context("开启 SQLite 事务失败")?;

    {
        let mut insert = tx.prepare("INSERT INTO meta (key, value) VALUES (?1, ?2)")?;
        for (key, value) in meta_rows(data, extras, meta) {
            insert.execute(params![key, value])?;
        }
    }

    {
        let mut insert = tx.prepare("INSERT INTO stations VALUES (?1, ?2)")?;
        for (index, name) in data.stations.iter().enumerate() {
            insert.execute(params![index as i64, name])?;
        }
    }

    {
        let mut insert =
            tx.prepare("INSERT INTO trains VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)")?;
        for train in &data.trains {
            let last = train.stops.last();
            insert.execute(params![
                i64::from(train.index),
                train.code,
                train.train_no,
                train.kind,
                i64::from(train.date_from),
                i64::from(train.date_to),
                last.map(|stop| i64::from(stop.km)),
                last.map(|stop| i64::from(stop.day)),
                train.stops.first().map(|stop| stop.station.as_str()),
                last.map(|stop| stop.station.as_str()),
                i64::from(train.rule_index),
            ])?;
        }
    }

    {
        let mut insert = tx.prepare("INSERT INTO train_rules VALUES (?1, ?2, ?3)")?;
        for (index, rule) in extras.rules.iter().enumerate() {
            insert.execute(params![index as i64, rule.period, rule.mask])?;
        }
    }

    {
        let mut insert = tx.prepare(
            "INSERT INTO stops VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?;
        for train in &data.trains {
            for (index, stop) in train.stops.iter().enumerate() {
                insert.execute(params![
                    i64::from(train.index),
                    train.code,
                    stop.display_train,
                    (index + 1) as i64,
                    i64::from(stop.station_index),
                    stop.station,
                    i64::from(stop.km),
                    stop.arrive,
                    stop.depart,
                    i64::from(stop.dwell),
                    i64::from(stop.day),
                    i64::from(stop.arrive_min),
                    i64::from(stop.depart_min),
                    stop.platform,
                ])?;
            }
        }
    }

    write_extras(&tx, extras)?;

    tx.commit().context("提交 SQLite 事务失败")?;
    drop(conn);
    Ok(())
}

/// 写入经由 / 交路 / 餐车等附加表。
fn write_extras(tx: &rusqlite::Transaction<'_>, extras: &Extras) -> Result<()> {
    if let Some(routes) = &extras.routes {
        let mut insert_line = tx.prepare("INSERT INTO lines VALUES (?1, ?2)")?;
        for line in routes.lines() {
            insert_line.execute(params![line.code, line.name])?;
        }

        let mut insert_item =
            tx.prepare("INSERT INTO line_stations VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        let mut lines: Vec<_> = routes.lines().collect();
        lines.sort_by_key(|line| line.code);
        for line in lines {
            for (index, item) in line.items.iter().enumerate() {
                insert_item.execute(params![
                    line.code,
                    (index + 1) as i64,
                    item.station,
                    item.tele_code,
                    item.flag,
                    item.km,
                ])?;
            }
        }
    }

    {
        let mut insert =
            tx.prepare("INSERT INTO train_routes VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for via in &extras.vias {
            for (index, point) in via.points.iter().enumerate() {
                // 经由点是否是 s.i 里的车站；不是则多半是线路所 / 信号所。
                let station_index = extras.station_of(&point.station);
                insert.execute(params![
                    via.train_index as i64,
                    (index + 1) as i64,
                    point.station,
                    point.line,
                    point.km,
                    i64::from(station_index.is_some()),
                    station_index.map(|value| value as i64),
                ])?;
            }
        }
    }

    if let Some(duties) = &extras.duties {
        let mut insert_duty = tx.prepare("INSERT INTO duties VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        let mut insert_segment = tx.prepare("INSERT INTO duty_segments VALUES (?1, ?2, ?3, ?4)")?;
        for (id, duty) in duties.items.iter().enumerate() {
            let id = id as i64;
            insert_duty.execute(params![
                id,
                duty.start_date,
                duty.end_date,
                duty.company,
                duty.formation,
                duty.chain,
            ])?;
            for (index, segment) in duty.segments.iter().enumerate() {
                insert_segment.execute(params![
                    id,
                    (index + 1) as i64,
                    segment.label,
                    segment.code,
                ])?;
            }
        }

        // 原始数据里有少量完全重复的餐车记录，去重后写入。
        let mut insert_train =
            tx.prepare("INSERT OR IGNORE INTO duty_trains VALUES (?1, ?2, ?3)")?;
        for link in &extras.duty_trains {
            insert_train.execute(params![
                link.train_index as i64,
                link.duty_id as i64,
                link.seq as i64,
            ])?;
        }

        let mut insert_dining =
            tx.prepare("INSERT OR IGNORE INTO dining_cars VALUES (?1, ?2, ?3, ?4)")?;
        for car in &duties.dining {
            insert_dining.execute(params![
                car.train_index,
                car.start_date,
                car.end_date,
                car.coach,
            ])?;
        }
    }

    if let Some(sn) = &extras.same_city {
        let mut insert_city = tx.prepare("INSERT INTO city_nodes VALUES (?1, ?2)")?;
        for (index, city) in sn.cities.iter().enumerate() {
            insert_city.execute(params![samecity::VIRTUAL_BASE + index as i64, city])?;
        }

        let mut insert_group =
            tx.prepare("INSERT INTO same_city_groups VALUES (?1, ?2, ?3, ?4)")?;
        let mut insert_member =
            tx.prepare("INSERT INTO same_city_members VALUES (?1, ?2, ?3, ?4)")?;
        for group in &sn.groups {
            let city = group.is_city_node();
            let name = if city {
                sn.node_name(group.id).unwrap_or("?").to_owned()
            } else {
                extras
                    .station_name(group.id as usize)
                    .unwrap_or("?")
                    .to_owned()
            };
            insert_group.execute(params![
                group.id,
                name,
                (!city).then_some(group.id),
                i64::from(city),
            ])?;
            for (seq, member) in group.members.iter().enumerate() {
                insert_member.execute(params![
                    group.id,
                    member,
                    extras.station_name(*member as usize).unwrap_or("?"),
                    (seq + 1) as i64,
                ])?;
            }
        }

        let mut insert_alias = tx.prepare("INSERT INTO tele_aliases VALUES (?1, ?2)")?;
        for (station, code) in &sn.aliases {
            insert_alias.execute(params![station, code])?;
        }
    }
    Ok(())
}

/// `meta` 表的内容：数据版本、生成时间、来源，以及便于校验的行数。
fn meta_rows(data: &Dataset, extras: &Extras, meta: &Meta) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("generator", format!("llt {}", env!("CARGO_PKG_VERSION"))),
        ("built_at", meta.built_at.clone()),
        ("source", meta.source.clone()),
        ("station_count", data.stations.len().to_string()),
        ("train_count", data.trains.len().to_string()),
        ("stop_count", data.stop_count().to_string()),
        ("train_route_count", extras.vias.len().to_string()),
        ("route_point_count", extras.route_point_count().to_string()),
        (
            "route_point_station_count",
            extras.route_point_station_count().to_string(),
        ),
        ("duty_count", extras.duty_count().to_string()),
        ("dining_car_count", extras.dining_count().to_string()),
        ("duty_train_count", extras.duty_trains.len().to_string()),
        (
            "same_city_group_count",
            extras.same_city_group_count().to_string(),
        ),
        ("tele_alias_count", extras.tele_alias_count().to_string()),
        (
            "platform_record_count",
            extras.platform_record_count().to_string(),
        ),
        (
            "train_change_count",
            extras.train_change_count().to_string(),
        ),
    ];
    if let Some(version) = &meta.data_version {
        rows.push(("data_version", version.clone()));
    }
    if let Some(date) = meta.data_date() {
        rows.push(("data_date", date));
    }
    if let Some(value) = &meta.source_last_modified {
        rows.push(("source_last_modified", value.clone()));
    }
    if let Some(value) = &meta.etag {
        rows.push(("etag", value.clone()));
    }
    if let Some(bytes) = meta.archive_bytes {
        rows.push(("archive_bytes", bytes.to_string()));
    }
    if let Some(value) = &meta.remote_data_version {
        rows.push(("remote_data_version", value.clone()));
    }
    if let Some(value) = &meta.remote_app_version {
        rows.push(("remote_app_version", value.clone()));
    }
    if let Some(value) = &meta.version_source {
        rows.push(("version_source", value.clone()));
    }
    rows
}

/// 带 BOM、CRLF 行尾的 CSV 写入器（与 Python `csv` 模块的输出一致）。
struct CsvFile {
    path: PathBuf,
    writer: csv::Writer<BufWriter<File>>,
}

impl CsvFile {
    fn create(path: &Path) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("无法创建 {}", path.display()))?;
        let mut inner = BufWriter::new(file);
        inner
            .write_all(BOM)
            .with_context(|| format!("无法写入 {}", path.display()))?;
        let writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(inner);
        Ok(Self {
            path: path.to_path_buf(),
            writer,
        })
    }

    fn header(&mut self, fields: &[&str]) -> Result<()> {
        self.write(fields.iter().map(|field| (*field).to_owned()).collect())
    }

    fn record(&mut self, fields: Vec<String>) -> Result<()> {
        self.write(fields)
    }

    fn write(&mut self, fields: Vec<String>) -> Result<()> {
        self.writer
            .write_record(fields)
            .with_context(|| format!("写入 {} 失败", self.path.display()))
    }

    fn finish(mut self) -> Result<PathBuf> {
        self.writer
            .flush()
            .with_context(|| format!("写入 {} 失败", self.path.display()))?;
        Ok(self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{Dataset, Stop, Train};

    fn sample() -> Dataset {
        let stop = |index: u32, name: &str, arrive: &str, depart: &str| Stop {
            station_index: index,
            station: name.to_owned(),
            display_train: "G1".to_owned(),
            platform: if index == 0 {
                "3".to_owned()
            } else {
                String::new()
            },
            km: index * 100,
            arrive: arrive.to_owned(),
            depart: depart.to_owned(),
            dwell: 2,
            day: 1,
            arrive_min: index * 60,
            depart_min: index * 60 + 2,
        };
        Dataset {
            stations: vec!["甲".to_owned(), "乙".to_owned()],
            train_codes: vec!["G1".to_owned()],
            trains: vec![Train {
                index: 0,
                code: "G1".to_owned(),
                train_no: "1".to_owned(),
                kind: "高速动车".to_owned(),
                date_from: 20260101,
                date_to: 20261231,
                rule_index: 0,
                stops: vec![
                    stop(0, "甲", "08:00", "08:02"),
                    stop(1, "乙", "09:00", "09:02"),
                ],
            }],
            station_train_counts: vec![1, 1],
            notes: Vec::new(),
        }
    }

    /// 最小附加数据：一条线路、一趟车的经由、一条交路和一条餐车记录。
    fn sample_extras() -> Extras {
        use crate::duty::{DiningCar, Duties, Duty, Segment};
        use crate::extras::TrainVia;
        use crate::routes::{Line, LineItem, Routes, ViaPoint};

        let routes = Routes::from_lines(vec![Line {
            code: 7,
            name: "京沪线".to_owned(),
            items: vec![
                LineItem {
                    station: "甲".to_owned(),
                    tele_code: "AAA".to_owned(),
                    flag: 0,
                    km: 0,
                },
                LineItem {
                    station: "乙".to_owned(),
                    tele_code: "BBB".to_owned(),
                    flag: 1,
                    km: 100,
                },
            ],
        }]);
        Extras {
            rules: Vec::new(),
            changes: None,
            platforms: None,
            routes: Some(routes),
            duties: Some(Duties {
                items: vec![Duty {
                    start_date: 20260101,
                    end_date: 20261231,
                    company: "上局上海段".to_owned(),
                    formation: "CR400BF".to_owned(),
                    chain: "G1#G2".to_owned(),
                    segments: vec![
                        Segment {
                            label: "G1".to_owned(),
                            code: "G1".to_owned(),
                        },
                        Segment {
                            label: "G2".to_owned(),
                            code: "G2".to_owned(),
                        },
                    ],
                }],
                dining: vec![DiningCar {
                    train_index: 0,
                    start_date: 20260101,
                    end_date: 20261231,
                    coach: "9车[餐]".to_owned(),
                }],
            }),
            duty_trains: vec![crate::extras::DutyTrain {
                duty_id: 0,
                seq: 1,
                train_index: 0,
            }],
            vias: vec![TrainVia {
                train_index: 0,
                points: vec![
                    ViaPoint {
                        station: "甲".to_owned(),
                        line: "京沪线".to_owned(),
                        km: 0,
                    },
                    ViaPoint {
                        station: "乙".to_owned(),
                        line: "京沪线".to_owned(),
                        km: 100,
                    },
                ],
            }],
            station_index: [("甲".to_owned(), 0usize), ("乙".to_owned(), 1usize)]
                .into_iter()
                .collect(),
            station_names: vec!["甲".to_owned(), "乙".to_owned()],
            same_city: Some(crate::samecity::SameCity {
                cities: vec!["甲城".to_owned()],
                groups: vec![
                    crate::samecity::Group {
                        id: 0,
                        members: vec![1],
                    },
                    crate::samecity::Group {
                        id: sample_virtual_node(),
                        members: vec![0, 1],
                    },
                ],
                aliases: vec![("甲".to_owned(), "AA".to_owned())],
            }),
            notes: Vec::new(),
        }
    }

    fn sample_virtual_node() -> i64 {
        crate::samecity::VIRTUAL_BASE
    }

    fn sample_meta() -> Meta {
        Meta {
            remote_data_version: Some("20261108".to_owned()),
            remote_app_version: Some("5.3.7".to_owned()),
            version_source: Some("http://down.lltskb.com/android.ver".to_owned()),
            data_version: Some("20261108".to_owned()),
            built_at: "2026-10-31T13:00:00Z".to_owned(),
            source: "http://down.lltskb.com/an.db".to_owned(),
            source_last_modified: Some("Sat, 31 Oct 2026 02:01:24 GMT".to_owned()),
            etag: Some("\"07245bbdb68dd1:0\"".to_owned()),
            archive_bytes: Some(2_312_058),
        }
    }

    fn meta_value(path: &Path, key: &str) -> Option<String> {
        let conn = Connection::open(path).unwrap();
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .ok()
    }

    #[test]
    fn sqlite_records_data_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite");
        write_sqlite(&path, &sample(), &sample_extras(), &sample_meta()).unwrap();

        let conn = Connection::open(&path).unwrap();
        let user_version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let application_id: i64 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 20261108);
        assert_eq!(application_id, APPLICATION_ID);
        drop(conn);

        assert_eq!(
            meta_value(&path, "data_version").as_deref(),
            Some("20261108")
        );
        assert_eq!(
            meta_value(&path, "data_date").as_deref(),
            Some("2026-11-08")
        );
        assert_eq!(meta_value(&path, "station_count").as_deref(), Some("2"));
        assert_eq!(meta_value(&path, "stop_count").as_deref(), Some("2"));
        assert_eq!(
            meta_value(&path, "schema_version").as_deref(),
            Some(SCHEMA_VERSION.to_string().as_str())
        );
        assert_eq!(meta_value(&path, "route_point_count").as_deref(), Some("2"));
        assert_eq!(
            meta_value(&path, "route_point_station_count").as_deref(),
            Some("2")
        );
        assert_eq!(meta_value(&path, "duty_count").as_deref(), Some("1"));
        assert_eq!(meta_value(&path, "dining_car_count").as_deref(), Some("1"));
        assert_eq!(
            meta_value(&path, "archive_bytes").as_deref(),
            Some("2312058")
        );
        assert_eq!(
            meta_value(&path, "source_last_modified").as_deref(),
            Some("Sat, 31 Oct 2026 02:01:24 GMT")
        );
    }

    #[test]
    fn sqlite_without_version_keeps_zero_user_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite");
        let meta = Meta {
            built_at: "2026-10-31T13:00:00Z".to_owned(),
            source: "an.db".to_owned(),
            ..Default::default()
        };
        write_sqlite(&path, &sample(), &Extras::default(), &meta).unwrap();

        let conn = Connection::open(&path).unwrap();
        let user_version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 0);
        drop(conn);

        assert_eq!(meta_value(&path, "data_version"), None);
        assert_eq!(meta_value(&path, "data_date"), None);
    }

    #[test]
    fn data_date_accepts_only_plausible_dates() {
        let date = |version: &str| {
            Meta {
                data_version: Some(version.to_owned()),
                ..Default::default()
            }
            .data_date()
        };
        assert_eq!(date("20261108").as_deref(), Some("2026-11-08"));
        assert_eq!(date("20261308"), None);
        assert_eq!(date("20261132"), None);
        assert_eq!(date("2026110"), None);
        assert_eq!(date("2026-11-"), None);
        assert_eq!(date(""), None);
    }

    #[test]
    fn sqlite_records_same_city_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite");
        write_sqlite(&path, &sample(), &sample_extras(), &sample_meta()).unwrap();
        let conn = Connection::open(&path).unwrap();

        let groups: i64 = conn
            .query_row("SELECT count(*) FROM same_city_groups", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(groups, 2);

        // 车站组：gid 是车站下标，head 有下标；城市名节点：head 为 NULL。
        let (name, head, is_city): (String, Option<i64>, i64) = conn
            .query_row(
                "SELECT group_name, head_station_index, is_city_node
                 FROM same_city_groups WHERE group_id = 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((name.as_str(), head, is_city), ("甲", Some(0), 0));

        let city: (String, Option<i64>, i64) = conn
            .query_row(
                "SELECT group_name, head_station_index, is_city_node
                 FROM same_city_groups WHERE group_id = 10000",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((city.0.as_str(), city.1, city.2), ("甲城", None, 1));

        let members: Vec<String> = conn
            .prepare("SELECT station FROM same_city_members WHERE group_id = 0 ORDER BY seq")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(members, ["乙"]);
    }

    #[test]
    fn train_routes_reject_inconsistent_is_station() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite");
        write_sqlite(&path, &sample(), &sample_extras(), &sample_meta()).unwrap();
        let conn = Connection::open(&path).unwrap();

        // is_station = 1 却没有 station_index
        let missing_index = conn.execute(
            "INSERT INTO train_routes VALUES (0, 99, '丙', '京沪线', 200, 1, NULL)",
            [],
        );
        assert!(missing_index.is_err(), "is_station=1 必须带 station_index");

        // is_station = 0 却给了 station_index
        let extra_index = conn.execute(
            "INSERT INTO train_routes VALUES (0, 98, '丙', '京沪线', 200, 0, 0)",
            [],
        );
        assert!(extra_index.is_err(), "is_station=0 不应带 station_index");

        // 一致的两行可以写入
        conn.execute(
            "INSERT INTO train_routes VALUES (0, 99, '丙', '京沪线', 200, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO train_routes VALUES (0, 97, '丁所', '京沪线', 210, 0, NULL)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn csv_writes_extras() {
        let tmp = tempfile::tempdir().unwrap();
        let written = write_csvs(tmp.path(), &sample(), &sample_extras(), &sample_meta()).unwrap();
        let names: Vec<String> = written
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "stations.csv",
                "trains.csv",
                "stops.csv",
                "train_routes.csv",
                "duties.csv",
                "dining_cars.csv",
                "same_city.csv",
                "city_nodes.csv",
                "tele_aliases.csv"
            ]
        );

        let body = std::fs::read_to_string(tmp.path().join("train_routes.csv")).unwrap();
        assert!(body.starts_with('\u{feff}'), "应带 UTF-8 BOM");
        assert!(body.contains("train_index,seq,station,line,km,is_station,station_index"));
        assert!(body.contains("0,1,甲,京沪线,0,1,0"), "{body}");
        assert!(body.contains("0,2,乙,京沪线,100,1,1"), "{body}");

        let duties = std::fs::read_to_string(tmp.path().join("duties.csv")).unwrap();
        assert!(duties.contains("上局上海段"));
        assert!(duties.contains("G1 -> G2"));
    }

    #[test]
    fn csv_uses_bom_crlf_and_minimal_quotes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.csv");
        let mut csv = CsvFile::create(&path).unwrap();
        csv.header(&["a", "b"]).unwrap();
        csv.record(vec!["普通".into(), "含,逗号".into()]).unwrap();
        csv.record(vec!["引\"号".into(), String::new()]).unwrap();
        csv.finish().unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..3], BOM);
        assert_eq!(
            String::from_utf8(bytes[3..].to_vec()).unwrap(),
            "a,b\r\n普通,\"含,逗号\"\r\n\"引\"\"号\",\r\n"
        );
    }
}
