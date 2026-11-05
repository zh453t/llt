//! 附加数据（经由 / 交路 / 餐车）的只读查询。
//!
//! 数据由 `cli/` 的 `llt build` 写入下列表（见 `cli/src/export.rs`）：
//!
//! ```sql
//! lines        (line_code, line_name)
//! line_stations(line_code, seq, station, tele_code, flag, km)
//! train_routes (train_index, seq, station, line, km)          经由（已展开）
//! duties       (duty_id, start_date, end_date, company, formation, chain)
//! duty_segments(duty_id, seq, label, code)
//! duty_trains  (train_index, duty_id, seq)      交路段 -> 车次（构建时解析）
//! dining_cars  (train_index, start_date, end_date, coach)
//! city_nodes      (node_id, city)
//! same_city_groups(group_id, group_name, head_station_index, is_city_node)
//! same_city_members(group_id, station_index, station, seq)
//! tele_aliases (station, tele_code)
//! ```
//!
//! Python 版 `tools/decode_llt.py` 生成的旧库没有这些表，
//! 因此所有查询在表缺失时返回空结果而不是报错，界面/CLI 可以照常工作。

use rusqlite::OptionalExtension;

use crate::{Db, int, text};

/// 经由上的一个点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViaPoint {
    pub seq: i64,
    pub station: String,
    /// 所在线路名；跨线时用 `/` 合并（如 `京哈线/皇姑屯线`）。
    pub line: String,
    pub km: i64,
    /// 站名是否在 `stations` 表里，也就是能否用 `query station` 查到。
    /// `false` 的多半是线路所 / 信号所（名字常以「所」结尾）或未收录的货运站。
    pub is_station: bool,
    /// 对应 `stations.station_index`；`is_station` 为 `false` 时是 `None`。
    pub station_index: Option<i64>,
}

/// 同城组里的一个节点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SameCityStation {
    /// 车站下标；城市名节点的 head 为 `None`。
    pub station_index: Option<i64>,
    pub station: String,
    /// 是否是组的 gid 本身（App 查询时会被插到首位）。
    pub is_group_head: bool,
}

/// 同城集合解析结果：`(标题, [(station_index, 站名)])`。
pub type StationSelection = (String, Vec<(i64, String)>);

/// 一个同城组（`sn.dat` S2）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SameCityGroup {
    pub group_id: i64,
    /// 组名：车站名，或城市名（`is_city_node` 为真时）。
    pub group_name: String,
    /// gid 是城市名虚拟节点（`>= 10000`）。
    pub is_city_node: bool,
    /// 组成员，head 排第一，其余保持数据里的顺序。
    pub members: Vec<SameCityStation>,
}

/// 交路（车底周转交路）里的一段车次。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DutySegment {
    pub seq: i64,
    /// 原始写法，如 `G1806/7`。
    pub label: String,
    /// 段首车次号，用于匹配。
    pub code: String,
    /// 是否为当前查询的车次。
    pub current: bool,
}

/// 一条交路记录：同一组车次在某个日期区间内由同一个车底担当。
#[derive(Clone, Debug)]
pub struct Duty {
    pub id: i64,
    pub start_date: i64,
    pub end_date: i64,
    /// 担当企业（如 `上局上海段`）。
    pub company: String,
    /// 编组 / 车型（如 `CR400BF-S型重联`）。
    pub formation: String,
    /// 原始交路串（如 `G1806/7#G82/79#G32#G1`）。
    pub chain: String,
    pub segments: Vec<DutySegment>,
}

impl Duty {
    /// 该交路在 `date`（YYYYMMDD）是否有效。
    pub fn runs_on(&self, date: i64) -> bool {
        self.start_date <= date && date <= self.end_date
    }
}

/// 餐车记录。
#[derive(Clone, Debug)]
pub struct DiningCar {
    pub start_date: i64,
    pub end_date: i64,
    pub coach: String,
}

impl DiningCar {
    pub fn runs_on(&self, date: i64) -> bool {
        self.start_date <= date && date <= self.end_date
    }
}

impl Db {
    /// 表是否存在；旧库缺少附加表时返回 `false`。
    fn has_column(&self, table: &str, column: &str) -> bool {
        let sql = format!("PRAGMA table_info({table})");
        let Ok(mut stmt) = self.conn.prepare(&sql) else {
            return false;
        };
        let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
            return false;
        };
        rows.flatten().any(|name| name == column)
    }

    fn has_table(&self, name: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// 某趟车的经由（按站序）。
    pub fn via_points(&self, train_index: i64) -> rusqlite::Result<Vec<ViaPoint>> {
        if !self.has_table("train_routes") {
            return Ok(Vec::new());
        }
        // 早期版本没有 is_station / station_index 两列，缺列时按 0 / NULL 处理。
        let annotated = self.has_column("train_routes", "is_station");
        let sql = if annotated {
            "SELECT seq, station, line, km, is_station, station_index FROM train_routes
             WHERE train_index = ?1 ORDER BY seq"
        } else {
            "SELECT seq, station, line, km, 0, NULL FROM train_routes
             WHERE train_index = ?1 ORDER BY seq"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let points = stmt
            .query_map([train_index], |row| {
                Ok(ViaPoint {
                    seq: int(row, 0)?,
                    station: text(row, 1)?,
                    line: text(row, 2)?,
                    km: int(row, 3)?,
                    is_station: int(row, 4)? != 0,
                    station_index: row.get::<_, Option<i64>>(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(points)
    }

    /// 某趟车所属的交路。同一车次可能有多个日期区间，按起始日排序。
    ///
    /// 段与车次的对应关系在 `llt build` 时已经解析好（`duty_trains` 表），
    /// 因此 `Z158/5` 与 `Z155/Z158` 这类不同写法也能命中同一条交路。
    pub fn duties_for(&self, train_index: i64) -> rusqlite::Result<Vec<Duty>> {
        if !self.has_table("duties")
            || !self.has_table("duty_segments")
            || !self.has_table("duty_trains")
        {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare_cached(
            "SELECT d.duty_id, d.start_date, d.end_date, d.company, d.formation, d.chain
             FROM duties d
             WHERE d.duty_id IN (SELECT duty_id FROM duty_trains WHERE train_index = ?1)
             ORDER BY d.start_date, d.duty_id",
        )?;
        let duties = stmt
            .query_map([train_index], |row| {
                Ok(Duty {
                    id: int(row, 0)?,
                    start_date: int(row, 1)?,
                    end_date: int(row, 2)?,
                    company: text(row, 3)?,
                    formation: text(row, 4)?,
                    chain: text(row, 5)?,
                    segments: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if duties.is_empty() {
            return Ok(Vec::new());
        }

        // 本车次在交路里的位置：duty_id -> 段号集合。
        let mut current: std::collections::HashMap<i64, Vec<i64>> =
            std::collections::HashMap::new();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT duty_id, seq FROM duty_trains WHERE train_index = ?1")?;
        for row in stmt.query_map([train_index], |row| Ok((int(row, 0)?, int(row, 1)?)))? {
            let (duty_id, seq) = row?;
            current.entry(duty_id).or_default().push(seq);
        }

        let mut segments = self.conn.prepare_cached(
            "SELECT seq, label, code FROM duty_segments WHERE duty_id = ?1 ORDER BY seq",
        )?;
        let mut out = Vec::with_capacity(duties.len());
        for mut duty in duties {
            let positions = current.get(&duty.id).cloned().unwrap_or_default();
            duty.segments = segments
                .query_map([duty.id], |row| {
                    let seq: i64 = int(row, 0)?;
                    Ok(DutySegment {
                        seq,
                        label: text(row, 1)?,
                        code: text(row, 2)?,
                        current: positions.contains(&seq),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(duty);
        }
        Ok(out)
    }

    /// 某站所属的同城组（「模糊站名」）。
    ///
    /// 数据里有互为镜像的组（如天津的 `192(滨海)` 与 `2310(天津)`，成员仅差自身），
    /// 因此一个车站可能返回多个组；把它们的 `members` 取并集才是完整同城列表。
    pub fn same_city(&self, station_index: i64) -> rusqlite::Result<Vec<SameCityGroup>> {
        if !self.has_table("same_city_groups") || !self.has_table("same_city_members") {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT group_id FROM same_city_groups WHERE group_id = ?1
             UNION
             SELECT group_id FROM same_city_members WHERE station_index = ?1",
        )?;
        let ids = stmt
            .query_map([station_index], |row| int(row, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_groups(&ids)
    }

    /// 按城市名节点查同城组（`s.i` 里没有「成都」，但城市名节点有）。
    pub fn city_groups(&self, city: &str) -> rusqlite::Result<Vec<SameCityGroup>> {
        if !self.has_table("same_city_groups") || !self.has_table("same_city_members") {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT group_id FROM same_city_groups
             WHERE is_city_node = 1 AND group_name = ?1 ORDER BY group_id",
        )?;
        let ids = stmt
            .query_map([city], |row| int(row, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_groups(&ids)
    }

    /// 同名同城组（组名就是城市名或车站名）。
    ///
    /// `is_city_node = 1` 的城市名组（如成都）优先，其次才是以车站为头的组（如北京）。
    pub fn same_city_named(&self, name: &str) -> rusqlite::Result<Vec<SameCityGroup>> {
        if !self.has_table("same_city_groups") || !self.has_table("same_city_members") {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT group_id FROM same_city_groups
             WHERE group_name = ?1
             ORDER BY is_city_node DESC, group_id",
        )?;
        let ids = stmt
            .query_map([name], |row| int(row, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_groups(&ids)
    }

    /// 把一个站名/城市名解析成「同城车站集合」。
    ///
    /// 返回 `(显示用的组名, [(station_index, 站名)])`；先按组名精确匹配，
    /// 再按车站精确匹配并并入其所有同城组（镜像组取并集）。没有同城数据时
    /// 退化为该车站自身；站名也不存在时返回 `None`。
    pub fn same_city_selection(&self, query: &str) -> rusqlite::Result<Option<StationSelection>> {
        let mut groups = self.same_city_named(query)?;
        if let Some(index) = self.station_index(query)? {
            groups.extend(self.same_city(index)?);
        }
        if groups.is_empty() {
            let Some(index) = self.station_index(query)? else {
                return Ok(None);
            };
            return Ok(Some((query.to_owned(), vec![(index, query.to_owned())])));
        }

        // 镜像组里取成员最多的那个作为标题（天津 vs 滨海），顺序也以它为准。
        let title_group = groups
            .iter()
            .max_by_key(|group| group.members.len())
            .expect("groups is not empty");
        let title = title_group.group_name.clone();
        let title_id = title_group.group_id;

        let mut seen = std::collections::HashSet::new();
        let mut stations = Vec::new();
        for group in groups
            .iter()
            .filter(|group| group.group_id == title_id)
            .chain(groups.iter().filter(|group| group.group_id != title_id))
        {
            for member in &group.members {
                let Some(index) = member.station_index else {
                    continue;
                };
                if seen.insert(index) {
                    stations.push((index, member.station.clone()));
                }
            }
        }
        if stations.is_empty() {
            return Ok(None);
        }
        Ok(Some((title, stations)))
    }

    /// 同城组名的候选（输入「北」时给出「北京」等）。
    pub fn city_suggestions(&self, query: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
        if !self.has_table("same_city_groups") || query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let contains = format!("%{query}%");
        let prefix = format!("{query}%");
        let mut stmt = self.conn.prepare_cached(
            "SELECT group_name FROM same_city_groups
             WHERE group_name LIKE ?1
             GROUP BY group_name
             ORDER BY (group_name = ?2) DESC,
                      (group_name LIKE ?3) DESC,
                      length(group_name), group_name
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![contains, query, prefix, limit as i64],
            |row| row.get::<_, String>(0),
        )?;
        rows.collect()
    }

    /// 站名 -> 电报码（`sn.dat` S3，只有部分站有别名）。
    pub fn tele_code(&self, station: &str) -> rusqlite::Result<Option<String>> {
        if !self.has_table("tele_aliases") {
            return Ok(None);
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT tele_code FROM tele_aliases WHERE station = ?1 ORDER BY tele_code",
        )?;
        let mut rows = stmt.query([station])?;
        Ok(match rows.next()? {
            Some(row) => Some(text(row, 0)?),
            None => None,
        })
    }

    /// 按 group_id 装载完整组（head 插到首位）。
    fn load_groups(&self, ids: &[i64]) -> rusqlite::Result<Vec<SameCityGroup>> {
        let mut group_stmt = self.conn.prepare_cached(
            "SELECT group_name, head_station_index, is_city_node
             FROM same_city_groups WHERE group_id = ?1",
        )?;
        let mut member_stmt = self.conn.prepare_cached(
            "SELECT station_index, station FROM same_city_members
             WHERE group_id = ?1 ORDER BY seq",
        )?;

        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            let mut rows = group_stmt.query([id])?;
            let Some(row) = rows.next()? else {
                continue;
            };
            let group_name = text(row, 0)?;
            let head_index: Option<i64> = row.get(1)?;
            let is_city_node = int(row, 2)? != 0;
            drop(rows);

            let mut members = vec![SameCityStation {
                station_index: head_index,
                station: group_name.clone(),
                is_group_head: true,
            }];
            for row in member_stmt.query_map([id], |row| {
                Ok(SameCityStation {
                    station_index: Some(int(row, 0)?),
                    station: text(row, 1)?,
                    is_group_head: false,
                })
            })? {
                let member = row?;
                if member.station_index != head_index {
                    members.push(member);
                }
            }
            out.push(SameCityGroup {
                group_id: id,
                group_name,
                is_city_node,
                members,
            });
        }
        Ok(out)
    }

    /// 某趟车的餐车信息。
    pub fn dining_cars(&self, train_index: i64) -> rusqlite::Result<Vec<DiningCar>> {
        if !self.has_table("dining_cars") {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT start_date, end_date, coach FROM dining_cars
             WHERE train_index = ?1 ORDER BY start_date",
        )?;
        let cars = stmt
            .query_map([train_index], |row| {
                Ok(DiningCar {
                    start_date: int(row, 0)?,
                    end_date: int(row, 1)?,
                    coach: text(row, 2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(cars)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> Option<Db> {
        // 默认用仓库里的库；LLT_TEST_DB 可指向任意 `llt build` 产物。
        if let Some(dir) = std::env::var_os("LLT_TEST_DB") {
            let path = PathBuf::from(dir);
            return path.is_file().then(|| Db::open(&path).unwrap());
        }
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../out/timetable/llt-timetable.sqlite");
        path.exists().then(|| Db::open(&path).unwrap())
    }

    #[test]
    fn via_points_are_ordered() {
        let Some(db) = fixture() else {
            return;
        };
        let Some(train) = db
            .search_trains("G1", None)
            .unwrap()
            .into_iter()
            .find(|train| train.code == "G1")
        else {
            return;
        };
        let points = db.via_points(train.index).unwrap();
        if points.is_empty() {
            return; // 旧库没有经由表
        }
        assert!(points.len() > 10);
        assert_eq!(points[0].seq, 1);
        assert!(points.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        assert!(points.windows(2).all(|pair| pair[0].km <= pair[1].km));
        assert_eq!(points.last().unwrap().station, "上海虹桥");
    }

    /// 站名 -> `station_index`（用 `query station` 同款的站名精确匹配）。
    fn index_of(db: &Db, station: &str) -> Option<i64> {
        db.station_index(station).unwrap()
    }

    fn train_index(db: &Db, code: &str) -> Option<i64> {
        db.search_trains(code, None)
            .unwrap()
            .into_iter()
            .find(|train| train.code == code)
            .map(|train| train.index)
    }

    #[test]
    fn duties_resolve_segments() {
        let Some(db) = fixture() else {
            return;
        };
        let (Some(g1), Some(variant)) = (train_index(&db, "G1"), train_index(&db, "G1956/7"))
        else {
            return;
        };
        let duties = db.duties_for(g1).unwrap();
        if duties.is_empty() {
            return; // 旧库没有交路表
        }
        let duty = &duties[0];
        assert!(!duty.company.is_empty());
        assert!(duty.segments.iter().any(|segment| segment.current));
        assert_eq!(duty.segments.first().unwrap().seq, 1);

        // 同一条交路里的另一段（G1956/7）也能查到它。
        let sibling = db.duties_for(variant).unwrap();
        assert!(sibling.iter().any(|duty| duty.id == duties[0].id));
    }

    #[test]
    fn duties_match_direction_variants() {
        let Some(db) = fixture() else {
            return;
        };
        // t.i 写作 Z155/Z158，交路串写作 Z158/5。
        let Some(index) = train_index(&db, "Z155/Z158") else {
            return;
        };
        let duties = db.duties_for(index).unwrap();
        if duties.is_empty() {
            return;
        }
        let duty = &duties[0];
        assert!(
            duty.segments
                .iter()
                .any(|segment| segment.label == "Z158/5"),
            "{:?}",
            duty.segments
        );
        assert!(duty.segments.iter().any(|segment| segment.current));
    }

    #[test]
    fn same_city_groups_include_head_first() {
        let Some(db) = fixture() else {
            return;
        };
        let Some(index) = index_of(&db, "北京") else {
            return;
        };
        let groups = db.same_city(index).unwrap();
        if groups.is_empty() {
            return; // 旧库没有同城表
        }
        let group = &groups[0];
        assert_eq!(group.group_name, "北京");
        assert!(!group.is_city_node);
        assert!(group.members[0].is_group_head);
        assert_eq!(group.members.len(), 11);
        assert_eq!(group.members[0].station, "北京");
        let names: Vec<&str> = group.members.iter().map(|m| m.station.as_str()).collect();
        assert!(
            names.contains(&"北京南") && names.contains(&"亦庄"),
            "{names:?}"
        );
    }

    #[test]
    fn same_city_merges_mirror_groups() {
        let Some(db) = fixture() else {
            return;
        };
        let Some(index) = index_of(&db, "天津") else {
            return;
        };
        let groups = db.same_city(index).unwrap();
        if groups.is_empty() {
            return;
        }
        // 天津 同时在镜像组 192(滨海) 与自己所在的组里，并集是 12 个不同车站。
        let mut names: Vec<String> = groups
            .iter()
            .flat_map(|group| group.members.iter().map(|m| m.station.clone()))
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 12, "{names:?}");
    }

    #[test]
    fn city_node_lists_its_stations() {
        let Some(db) = fixture() else {
            return;
        };
        // s.i 里没有「成都」，但 sn.dat 有城市名节点。
        let groups = db.city_groups("成都").unwrap();
        if groups.is_empty() {
            return;
        }
        let group = &groups[0];
        assert!(group.is_city_node);
        assert_eq!(group.members[0].station, "成都");
        assert_eq!(group.members[0].station_index, None);
        assert_eq!(group.members.len(), 5); // head + 成都东/南/西/犀浦
    }

    #[test]
    fn tele_codes_are_optional() {
        let Some(db) = fixture() else {
            return;
        };
        if !db.has_table("tele_aliases") {
            return;
        }
        // 只有部分站有电报码别名。
        assert_eq!(db.tele_code("北京").unwrap(), None);
        assert!(db.tele_code("一步滩").unwrap().is_some());
    }

    #[test]
    fn unknown_train_gives_empty_results() {
        let Some(db) = fixture() else {
            return;
        };
        assert!(db.via_points(-1).unwrap().is_empty());
        assert!(db.duties_for(-1).unwrap().is_empty());
        assert!(db.dining_cars(-1).unwrap().is_empty());
    }
}
