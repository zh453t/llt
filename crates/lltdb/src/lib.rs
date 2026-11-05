//! 路路通离线时刻表 SQLite 的只读查询层，供 `llt-gui` 与 `llt`（CLI 的查询子命令）共用。
//!
//! 数据库由 `cli/`（Rust）或 `tools/decode_llt.py`（Python）生成，表结构见
//! `out/timetable/README.md`；`meta` 表由 CLI 额外写入，用于记录数据版本：
//!
//! ```sql
//! meta    (key, value)
//! stations(station_index, station)
//! trains  (train_index, train, train_no, type, date_from, date_to,
//!          total_km, days, first_station, last_station)
//! stops   (train_index, train, seq, station_index, station, km,
//!          arrive, depart, dwell_min, day, arrive_min, depart_min)
//! ```
//!
//! 附加表（经由 / 交路 / 餐车）见 [`extras`]，旧库缺少它们时相关查询返回空结果。

pub mod date;
pub mod extras;
pub mod path;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params, params_from_iter};

const TRAIN_COLUMNS: &str = "train_index, train, type, first_station, last_station, \
                             total_km, days, date_from, date_to";

#[derive(Clone, Copy, Debug, Default)]
pub struct DbStats {
    pub trains: i64,
    pub stations: i64,
    pub stops: i64,
}

/// `20261108` 这样的 8 位数据版本号；位数或月/日不合法时视为无效。
fn parse_version(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let month: i64 = value[4..6].parse().ok()?;
    let day: i64 = value[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    value.parse().ok()
}

#[derive(Clone, Debug)]
pub struct TrainSummary {
    pub index: i64,
    pub code: String,
    pub kind: String,
    pub first: String,
    pub last: String,
    pub total_km: i64,
    pub days: i64,
    pub date_from: i64,
    pub date_to: i64,
}

#[derive(Clone, Debug)]
pub struct Stop {
    pub seq: i64,
    pub station: String,
    pub arrive: String,
    pub depart: String,
    pub dwell_min: i64,
    pub km: i64,
    pub day: i64,
    pub arrive_min: i64,
    pub depart_min: i64,
}

#[derive(Clone, Debug)]
pub struct TrainDetail {
    pub summary: TrainSummary,
    pub stops: Vec<Stop>,
}

#[derive(Clone, Debug)]
pub struct StationStop {
    /// 乘车站（同城查询时一屏里可能有多个车站）。
    pub station: String,
    pub train_index: i64,
    pub code: String,
    pub kind: String,
    pub first: String,
    pub last: String,
    pub arrive: String,
    pub depart: String,
    pub dwell_min: i64,
    pub day: i64,
    pub km: i64,
    pub date_from: i64,
    pub date_to: i64,
}

#[derive(Clone, Debug)]
pub struct OdResult {
    pub train_index: i64,
    pub code: String,
    pub kind: String,
    pub first: String,
    pub last: String,
    pub depart: String,
    pub depart_min: i64,
    pub depart_day: i64,
    pub from_km: i64,
    pub arrive: String,
    pub arrive_min: i64,
    pub arrive_day: i64,
    pub to_km: i64,
    /// 实际出发站（同城查询时是集合中的具体车站）。
    pub from_station: String,
    /// 实际到达站。
    pub to_station: String,
    pub date_from: i64,
    pub date_to: i64,
}

impl TrainSummary {
    /// 该车次在 `date`（YYYYMMDD）是否处于开行区间内。
    ///
    /// `date_from` / `date_to` 是车次的始发日期区间。
    pub fn runs_on(&self, date: i64) -> bool {
        self.date_from <= date && date <= self.date_to
    }
}

impl StationStop {
    /// `origin_date` 是按本站第几天换算出的始发日期（YYYYMMDD）。
    pub fn runs_on(&self, origin_date: i64) -> bool {
        self.date_from <= origin_date && origin_date <= self.date_to
    }
}

impl OdResult {
    /// `origin_date` 是按出发站第几天换算出的始发日期（YYYYMMDD）。
    pub fn runs_on(&self, origin_date: i64) -> bool {
        self.date_from <= origin_date && origin_date <= self.date_to
    }
}

impl OdResult {
    /// 从出发站发车到到达站到达的历时（分钟）。
    pub fn duration_min(&self) -> i64 {
        (self.arrive_min - self.depart_min).max(0)
    }

    /// 两站之间的里程。
    pub fn km(&self) -> i64 {
        (self.to_km - self.from_km).max(0)
    }
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// 以只读方式打开数据库，并做最基本的表结构校验。
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("无法打开数据库 {}：{e}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(|e| format!("设置 busy_timeout 失败：{e}"))?;

        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master \
                 WHERE type = 'table' AND name IN ('stations', 'trains', 'stops')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("读取表结构失败：{e}"))?;
        if tables < 3 {
            return Err(format!(
                "{} 缺少 stations / trains / stops 表，可能不是路路通时刻表数据库。",
                path.display()
            ));
        }

        // 提前准备常用语句，顺便确认关键索引/字段可用。
        conn.prepare_cached("SELECT count(*) FROM stops")
            .map_err(|e| format!("数据库不兼容：{e}"))?;

        Ok(Self { conn })
    }

    pub fn stats(&self) -> rusqlite::Result<DbStats> {
        Ok(DbStats {
            trains: self
                .conn
                .query_row("SELECT count(*) FROM trains", [], |row| row.get(0))?,
            stations: self
                .conn
                .query_row("SELECT count(*) FROM stations", [], |row| row.get(0))?,
            stops: self
                .conn
                .query_row("SELECT count(*) FROM stops", [], |row| row.get(0))?,
        })
    }

    /// `meta` 表里的一个值；由 `tools/decode_llt.py` 生成的旧库没有该表，返回 `None`。
    fn meta_value(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .ok()
    }

    /// 数据包版本（`ver.txt` 原文的 `YYYYMMDD` 整数，例如 `20261108`）。
    ///
    /// 先读 `meta.data_version`，没有 `meta` 表时退回 `PRAGMA user_version`
    /// （`llt build` 两者都写，`decode_llt.py` 两者都没有）；都拿不到时返回 `None`。
    pub fn data_version(&self) -> Option<i64> {
        let from_meta = self
            .meta_value("data_version")
            .and_then(|value| parse_version(&value));
        from_meta.or_else(|| {
            let pragma: i64 = self
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .ok()?;
            parse_version(&pragma.to_string())
        })
    }

    /// 数据日期 `2026-11-08`，来自 `meta.data_date`，否则由 [`Db::data_version`] 推出。
    pub fn data_date(&self) -> Option<String> {
        if let Some(date) = self.meta_value("data_date")
            && !date.trim().is_empty()
        {
            return Some(date);
        }
        self.data_version().map(date::date_ymd)
    }

    /// 按车次查询：支持车次前缀（G1）、完整车次（G100/G97）以及
    /// 去掉速度前缀的车次号（54 → G54 / K54 / Z54）。
    ///
    /// 同一车次可能在数据里有多个开行区间，这里全部返回。
    pub fn search_trains(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> rusqlite::Result<Vec<TrainSummary>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite 的 `LIMIT -1` 表示不限制条数。
        let limit = limit.map_or(-1, |limit| limit as i64);
        let prefix = format!("{query}%");
        let slash_segment = format!("%/{query}%");

        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {TRAIN_COLUMNS} FROM trains
             WHERE train LIKE ?1 OR train LIKE ?2 OR train_no LIKE ?1
             ORDER BY (train = ?3) DESC,
                      (train LIKE ?1) DESC,
                      (train_no = ?3) DESC,
                      length(train), train
             LIMIT ?4"
        ))?;

        let rows = stmt.query_map(params![prefix, slash_segment, query, limit], map_train)?;
        rows.collect()
    }

    /// 车站名模糊搜索，精确匹配、前缀匹配排在前面。
    pub fn station_suggestions(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let contains = format!("%{query}%");
        let prefix = format!("{query}%");

        let mut stmt = self.conn.prepare_cached(
            "SELECT station FROM stations
             WHERE station LIKE ?1
             ORDER BY (station = ?2) DESC,
                      (station LIKE ?3) DESC,
                      length(station), station
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![contains, query, prefix, limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect()
    }

    pub fn find_station(&self, station: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT station FROM stations WHERE station = ?1 LIMIT 1",
                [station],
                |row| row.get(0),
            )
            .optional()
    }

    /// 某车站的经停列车，按本站发车时间排序。
    /// 站名 -> `stations.station_index`（精确匹配）。
    pub fn station_index(&self, station: &str) -> rusqlite::Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT station_index FROM stations WHERE station = ?1 LIMIT 1")?;
        let mut rows = stmt.query([station])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    pub fn station_board(&self, station: &str) -> rusqlite::Result<Vec<StationStop>> {
        let Some(index) = self.station_index(station)? else {
            return Ok(Vec::new());
        };
        self.station_board_multi(&[index])
    }

    /// 多个车站的经停列车；同城查询与车站筛选共用。
    pub fn station_board_multi(&self, indices: &[i64]) -> rusqlite::Result<Vec<StationStop>> {
        if indices.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", indices.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT s.station, t.train_index, t.train, t.type, t.first_station, t.last_station,
                    s.arrive, s.depart, s.dwell_min, s.day, s.km, t.date_from, t.date_to
             FROM stops s
             JOIN trains t ON t.train_index = s.train_index
             WHERE s.station_index IN ({placeholders})
             ORDER BY s.depart, s.depart_min, t.train"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(indices.iter()), |row| {
            Ok(StationStop {
                station: text(row, 0)?,
                train_index: row.get(1)?,
                code: text(row, 2)?,
                kind: text(row, 3)?,
                first: text(row, 4)?,
                last: text(row, 5)?,
                arrive: text(row, 6)?,
                depart: text(row, 7)?,
                dwell_min: int(row, 8)?,
                day: int(row, 9)?,
                km: int(row, 10)?,
                date_from: int(row, 11)?,
                date_to: int(row, 12)?,
            })
        })?;
        rows.collect()
    }

    /// 站到站直达查询：找出同一趟车先经停 `from`、后经停 `to` 的所有车次。
    ///
    /// 返回 `(结果, 是否被截断)`。
    pub fn od_search(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> rusqlite::Result<(Vec<OdResult>, bool)> {
        let Some(from_index) = self.station_index(from)? else {
            return Ok((Vec::new(), false));
        };
        let Some(to_index) = self.station_index(to)? else {
            return Ok((Vec::new(), false));
        };
        self.od_search_multi(&[from_index], &[to_index], limit)
    }

    /// 多车站集合的站到站查询（同城模糊搜索）。
    pub fn od_search_multi(
        &self,
        from_indices: &[i64],
        to_indices: &[i64],
        limit: usize,
    ) -> rusqlite::Result<(Vec<OdResult>, bool)> {
        if limit == 0 || from_indices.is_empty() || to_indices.is_empty() {
            return Ok((Vec::new(), false));
        }
        let from_marks = std::iter::repeat_n("?", from_indices.len())
            .collect::<Vec<_>>()
            .join(",");
        let to_marks = std::iter::repeat_n("?", to_indices.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT t.train_index, t.train, t.type, t.first_station, t.last_station,
                    s1.depart, s1.depart_min, s1.day, s1.km,
                    s2.arrive, s2.arrive_min, s2.day, s2.km,
                    s1.station, s2.station,
                    t.date_from, t.date_to
             FROM stops s1
             JOIN stops s2 ON s2.train_index = s1.train_index AND s2.seq > s1.seq
             JOIN trains t ON t.train_index = s1.train_index
             WHERE s1.station_index IN ({from_marks})
               AND s2.station_index IN ({to_marks})
             ORDER BY s1.depart, s1.depart_min, t.train
             LIMIT ?"
        );
        let mut params: Vec<i64> = from_indices.iter().chain(to_indices).copied().collect();
        params.push((limit as i64) + 1);

        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mut rows = stmt.query(params_from_iter(params.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(OdResult {
                train_index: row.get(0)?,
                code: text(row, 1)?,
                kind: text(row, 2)?,
                first: text(row, 3)?,
                last: text(row, 4)?,
                depart: text(row, 5)?,
                depart_min: int(row, 6)?,
                depart_day: int(row, 7)?,
                from_km: int(row, 8)?,
                arrive: text(row, 9)?,
                arrive_min: int(row, 10)?,
                arrive_day: int(row, 11)?,
                to_km: int(row, 12)?,
                from_station: text(row, 13)?,
                to_station: text(row, 14)?,
                date_from: int(row, 15)?,
                date_to: int(row, 16)?,
            });
        }

        let truncated = out.len() > limit;
        out.truncate(limit);
        Ok((out, truncated))
    }

    /// 车次详情（含全部停站）。
    pub fn train_detail(&self, train_index: i64) -> rusqlite::Result<TrainDetail> {
        let summary = self.conn.query_row(
            &format!("SELECT {TRAIN_COLUMNS} FROM trains WHERE train_index = ?1"),
            [train_index],
            map_train,
        )?;

        let mut stmt = self.conn.prepare_cached(
            "SELECT seq, station, arrive, depart, dwell_min, km, day,
                    arrive_min, depart_min
             FROM stops WHERE train_index = ?1 ORDER BY seq",
        )?;
        let stops = stmt
            .query_map([train_index], |row| {
                Ok(Stop {
                    seq: int(row, 0)?,
                    station: text(row, 1)?,
                    arrive: text(row, 2)?,
                    depart: text(row, 3)?,
                    dwell_min: int(row, 4)?,
                    km: int(row, 5)?,
                    day: int(row, 6)?,
                    arrive_min: int(row, 7)?,
                    depart_min: int(row, 8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(TrainDetail { summary, stops })
    }
}

fn map_train(row: &Row<'_>) -> rusqlite::Result<TrainSummary> {
    Ok(TrainSummary {
        index: int(row, 0)?,
        code: text(row, 1)?,
        kind: text(row, 2)?,
        first: text(row, 3)?,
        last: text(row, 4)?,
        total_km: int(row, 5)?,
        days: int(row, 6)?,
        date_from: int(row, 7)?,
        date_to: int(row, 8)?,
    })
}

fn text(row: &Row<'_>, index: usize) -> rusqlite::Result<String> {
    Ok(row.get::<_, Option<String>>(index)?.unwrap_or_default())
}

fn int(row: &Row<'_>, index: usize) -> rusqlite::Result<i64> {
    Ok(row.get::<_, Option<i64>>(index)?.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn open_fixture() -> Db {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../out/timetable/llt-timetable.sqlite");
        Db::open(&path).expect("fixture database")
    }

    #[test]
    fn stats_are_populated() {
        let stats = open_fixture().stats().unwrap();
        assert!(stats.trains > 10_000, "{stats:?}");
        assert!(stats.stations > 3_000, "{stats:?}");
        assert!(stats.stops > 100_000, "{stats:?}");
    }

    #[test]
    fn fixture_records_data_version() {
        let db = open_fixture();
        let version = db.data_version().expect("fixture 库应记录数据版本");
        assert!(version > 20_000_000, "{version}");
        assert_eq!(db.data_date().unwrap(), date::date_ymd(version));
    }

    #[test]
    fn data_version_falls_back_to_pragma() {
        let path = std::env::temp_dir().join(format!(
            "llt-version-test-{}-{:?}.sqlite",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);

        // 旧库（decode_llt.py）没有 meta 表，只可能从 PRAGMA user_version 拿到版本。
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE stations (station_index INTEGER PRIMARY KEY, station TEXT);
                 CREATE TABLE trains (train_index INTEGER PRIMARY KEY);
                 CREATE TABLE stops (train_index INTEGER);
                 PRAGMA user_version = 20261107;",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.data_version(), Some(20261107));
        assert_eq!(db.data_date().as_deref(), Some("2026-11-07"));
        drop(db);

        // 有 meta 表时以 meta 为准。
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('data_version', '20261108'),
                                         ('data_date', '2026-11-08');",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.data_version(), Some(20261108));
        assert_eq!(db.data_date().as_deref(), Some("2026-11-08"));
        drop(db);

        // 版本号格式不对（如全 0）时视为没有版本信息。
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("DELETE FROM meta; PRAGMA user_version = 0;")
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.data_version(), None);
        assert_eq!(db.data_date(), None);
        drop(db);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn finds_train_by_code_and_number() {
        let db = open_fixture();

        let g1 = db.search_trains("G1", Some(50)).unwrap();
        assert!(g1.iter().any(|t| t.code == "G1"), "G1 missing");
        let g1 = g1.into_iter().find(|t| t.code == "G1").unwrap();
        assert_eq!(g1.first, "北京南");
        assert_eq!(g1.last, "上海虹桥");

        let by_number = db.search_trains("54", Some(100)).unwrap();
        assert!(by_number.iter().any(|t| t.code == "G54"));
        assert!(by_number.iter().any(|t| t.code == "K54"));
        assert!(by_number.iter().any(|t| t.code == "Z54"));

        // 车次号带斜杠的车次也能按其中一个号找到
        let slash = db.search_trains("G97", Some(100)).unwrap();
        assert!(slash.iter().any(|t| t.code == "G100/G97"));
    }

    #[test]
    fn train_search_without_limit_returns_all_matches() {
        let db = open_fixture();
        let capped = db.search_trains("G1", Some(25)).unwrap();
        assert_eq!(capped.len(), 25);

        let all = db.search_trains("G1", None).unwrap();
        assert!(all.len() > capped.len(), "None should mean no LIMIT");
        assert!(all.windows(2).all(|w| w[0].date_from != 0));
    }

    #[test]
    fn train_detail_has_ordered_stops() {
        let db = open_fixture();
        let g1 = db
            .search_trains("G1", Some(50))
            .unwrap()
            .into_iter()
            .find(|t| t.code == "G1")
            .unwrap();
        let detail = db.train_detail(g1.index).unwrap();
        assert!(detail.stops.len() >= 2);
        assert_eq!(detail.stops.first().unwrap().station, "北京南");
        assert_eq!(detail.stops.last().unwrap().station, "上海虹桥");
        assert!(
            detail.stops.windows(2).all(|w| w[0].seq < w[1].seq),
            "stops must be ordered"
        );
        assert!(
            detail
                .stops
                .windows(2)
                .all(|w| w[0].depart_min <= w[1].arrive_min),
            "time must move forward"
        );
    }

    #[test]
    fn station_search_and_board() {
        let db = open_fixture();
        let suggestions = db.station_suggestions("北京", 20).unwrap();
        assert_eq!(suggestions.first().map(String::as_str), Some("北京"));
        assert!(suggestions.iter().any(|s| s == "北京南"));

        let board = db.station_board("北京").unwrap();
        assert!(board.len() > 20, "北京 should have many trains");
        assert!(
            board.windows(2).all(|w| w[0].depart <= w[1].depart),
            "board should be sorted by clock time"
        );
        let t1461 = board
            .iter()
            .find(|row| row.code == "1461")
            .expect("1461 should stop at 北京");
        assert_eq!(t1461.depart, "11:59");
        assert_eq!(t1461.station, "北京");
        assert!(t1461.date_from > 0 && t1461.date_from <= t1461.date_to);
        assert!(t1461.runs_on(t1461.date_from));
        assert!(!t1461.runs_on(t1461.date_from - 1));
    }

    #[test]
    fn od_search_returns_direct_trains() {
        let db = open_fixture();
        let (rows, truncated) = db.od_search("北京", "上海", 500).unwrap();
        assert!(!truncated);
        assert!(!rows.is_empty());
        assert!(
            rows.windows(2).all(|w| w[0].depart <= w[1].depart),
            "results should be sorted by clock time"
        );

        let t1461 = rows.iter().find(|r| r.code == "1461").unwrap();
        assert_eq!(t1461.depart, "11:59");
        assert_eq!(t1461.from_station, "北京");
        assert_eq!(t1461.to_station, "上海");
        assert_eq!(t1461.arrive, "06:45");
        assert_eq!(t1461.depart_day, 1);
        assert_eq!(t1461.arrive_day, 2);
        assert_eq!(t1461.duration_min(), 1126);
        assert_eq!(t1461.km(), 1463);
        assert!(t1461.date_from > 0 && t1461.date_from <= t1461.date_to);
        assert!(t1461.runs_on(t1461.date_from));
        assert!(!t1461.runs_on(t1461.date_from - 1));

        let (reverse, _) = db.od_search("上海", "北京", 500).unwrap();
        assert!(reverse.iter().any(|r| r.code == "1462"));
    }

    #[test]
    fn same_city_selection_merges_city_and_station() {
        let db = open_fixture();
        let (title, stations) = db.same_city_selection("北京").unwrap().unwrap();
        assert_eq!(title, "北京");
        assert!(stations.iter().any(|(_, name)| name == "北京南"));
        assert!(stations.len() >= 8);

        let (station_title, station_stations) = db.same_city_selection("北京南").unwrap().unwrap();
        assert_eq!(station_title, "北京");
        assert_eq!(station_stations.len(), stations.len());
    }

    #[test]
    fn same_city_selection_city_node_has_no_virtual_station() {
        let db = open_fixture();
        let (title, stations) = db.same_city_selection("成都").unwrap().unwrap();
        assert_eq!(title, "成都");
        assert!(stations.iter().all(|(_, name)| name != "成都"));
        assert!(stations.iter().any(|(_, name)| name == "成都东"));
    }

    #[test]
    fn city_suggestions_include_group_names() {
        let db = open_fixture();
        let cities = db.city_suggestions("北", 20).unwrap();
        assert!(cities.iter().any(|name| name == "北京"));
    }

    #[test]
    fn station_board_multi_marks_boarding_station() {
        let db = open_fixture();
        let beijing = db.station_index("北京南").unwrap().unwrap();
        let shanghai = db.station_index("上海虹桥").unwrap().unwrap();
        let board = db.station_board_multi(&[beijing, shanghai]).unwrap();
        assert!(board.iter().any(|row| row.station == "北京南"));
        assert!(board.iter().any(|row| row.station == "上海虹桥"));
        let g1 = board.iter().find(|row| row.code == "G1").unwrap();
        assert_eq!(g1.station, "北京南");
    }

    #[test]
    fn od_search_multi_reports_actual_segment() {
        let db = open_fixture();
        let beijing_south = db.station_index("北京南").unwrap().unwrap();
        let shanghai_hongqiao = db.station_index("上海虹桥").unwrap().unwrap();
        let (rows, _) = db
            .od_search_multi(&[beijing_south], &[shanghai_hongqiao], 100)
            .unwrap();
        let g1 = rows.iter().find(|row| row.code == "G1").unwrap();
        assert_eq!(g1.from_station, "北京南");
        assert_eq!(g1.to_station, "上海虹桥");

        let beijing = db.station_index("北京").unwrap().unwrap();
        let shanghai = db.station_index("上海").unwrap().unwrap();
        let (both, _) = db
            .od_search_multi(
                &[beijing_south, beijing],
                &[shanghai_hongqiao, shanghai],
                500,
            )
            .unwrap();
        assert!(
            both.iter()
                .any(|row| row.from_station == "北京南" && row.to_station == "上海虹桥")
        );
    }

    #[test]
    fn unknown_station_is_empty() {
        let db = open_fixture();
        assert!(db.find_station("不存在的车站").unwrap().is_none());
        assert!(db.station_board("不存在的车站").unwrap().is_empty());
        assert!(
            db.od_search("不存在的车站", "北京", 10)
                .unwrap()
                .0
                .is_empty()
        );
    }
}
