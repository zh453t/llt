//! 附加数据（经由 / 交路 / 餐车 / 同城车站 / 站台 / 车次变化）的汇总加载。
//!
//! 这些数据来自 `an.db` 里的 `routes.dat`、`jlb.dat`、`sp.dat`、`extra.dat`、
//! `sn.dat`、`xw.dat`、`plat.dat`。
//! 它们属于“增强信息”，缺失或损坏时不应该让整次解码失败，
//! 因此这里统一记进 `notes`，由调用方提示用户。

use std::collections::HashMap;

use crate::archive::ShardDir;
use crate::decode::Train;
use crate::duty::{self, Duties};
use crate::format::variants;
use crate::platform::Platforms;
use crate::routes::{self, Routes, ViaPoint};
use crate::samecity::{self, SameCity};
use crate::spdata;
use crate::xw::Changes;

/// 一趟车的经由。
#[derive(Debug)]
pub struct TrainVia {
    pub train_index: usize,
    pub points: Vec<ViaPoint>,
}

/// 交路里的一段与实际车次的对应关系（构建时解析，查询时不必再做字符串匹配）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DutyTrain {
    pub duty_id: usize,
    /// 交路里的第几段（1 起）。
    pub seq: usize,
    pub train_index: usize,
}

/// 一条开行周期：`period` 天一轮，`mask` 的第 `k` 位为 1 表示第 `k` 天开行。
///
/// 来自 `t.rule`，第 `k` 行对应车次头里的 `ruleIndex = k`；周期为 1 时恒开行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRule {
    pub period: i64,
    pub mask: i64,
}

/// 与主表一起写进 SQLite 的附加数据。
#[derive(Debug, Default)]
pub struct Extras {
    pub routes: Option<Routes>,
    pub duties: Option<Duties>,
    pub vias: Vec<TrainVia>,
    /// 交路段 -> `train_index`。
    pub duty_trains: Vec<DutyTrain>,
    /// 同城车站 / 城市名节点 / 电报码别名（`sn.dat`）。
    pub same_city: Option<SameCity>,
    /// `s.i` 车站名 -> `station_index`，用于标记经由点能否按车站搜索。
    pub station_index: HashMap<String, usize>,
    /// `s.i` 车站顺序表，`station_index` -> 站名。
    pub station_names: Vec<String>,
    /// `t.rule` 开行周期表，下标即车次头里的 `ruleIndex`。
    pub rules: Vec<RunRule>,
    /// `xw.dat` 车次变化（同一趟车途中改车次号）。
    pub changes: Option<Changes>,
    /// `plat.dat` 站台 / 出站口提示。
    pub platforms: Option<Platforms>,
    pub notes: Vec<String>,
}

impl Extras {
    /// 读取并解析附加数据；文件缺失或解析失败都只记 note，不报错。
    ///
    /// `train_codes` 是 `t.i` 的车次号（下标即 `train_index`），
    /// 用于把交路的每一段关联到具体车次。
    pub fn load(shards: &ShardDir, train_codes: &[String], stations: &[String]) -> Self {
        let mut notes = Vec::new();

        // 经由：routes.dat + sp.dat（内部编号）缺一不可。
        let (routes, vias) = load_routes(shards, &mut notes);
        // 交路：jlb.dat，企业名来自 extra.dat 首段（缺失时显示 ?id）。
        let (duties, duty_trains) = load_duties(shards, train_codes, &mut notes);
        // 同城车站：sn.dat，越界的组会被丢弃并提示。
        let same_city = load_same_city(shards, stations.len(), &mut notes);
        // 开行周期：t.rule，缺了只是没有日历，不影响时刻表。
        let rules = load_rules(shards, &mut notes);
        // 站台：plat.dat，缺了只是详情页少一列。
        let platforms = load_platforms(shards, &mut notes);
        // 车次变化：xw.dat，缺了每站显示车次名的第一段。
        let changes = load_changes(shards, &mut notes);

        Self {
            routes,
            duties,
            vias,
            duty_trains,
            same_city,
            station_index: stations
                .iter()
                .enumerate()
                .map(|(index, name)| (name.clone(), index))
                .collect(),
            station_names: stations.to_vec(),
            rules,
            changes,
            platforms,
            notes,
        }
    }

    /// 把 `xw.dat` / `plat.dat` 的结果补进停站记录。
    ///
    /// `display_train` 按该站累计里程选分段（没有数据时保持 `t.i` 车次名的第一段）；
    /// `platform` 在数据里没有该 (车次, 车站) 时留空。
    pub fn annotate_stops(&self, trains: &mut [Train]) {
        for train in trains {
            for stop in &mut train.stops {
                if let Some(changes) = &self.changes {
                    stop.display_train = changes.shown(train.index, &train.code, stop.km);
                }
                if let Some(platforms) = &self.platforms {
                    stop.platform = platforms
                        .get(train.index, stop.station_index)
                        .unwrap_or_default()
                        .to_owned();
                }
            }
        }
    }

    /// `xw.dat` 里有分段的记录数。
    pub fn train_change_count(&self) -> usize {
        self.changes
            .as_ref()
            .map_or(0, Changes::changed_record_count)
    }

    /// `plat.dat` 的记录数（不等同于实际有站台的停站条数）。
    pub fn platform_record_count(&self) -> usize {
        self.platforms.as_ref().map_or(0, Platforms::len)
    }

    /// `station_index` -> 车站名。
    pub fn station_name(&self, index: usize) -> Option<&str> {
        self.station_names.get(index).map(String::as_str)
    }

    /// 同城组的显示名（车站名或城市名）。
    pub fn group_name(&self, group: &samecity::Group) -> String {
        if group.is_city_node() {
            self.same_city
                .as_ref()
                .and_then(|sn| sn.node_name(group.id))
                .unwrap_or("?")
                .to_owned()
        } else {
            self.station_name(group.id as usize)
                .unwrap_or("?")
                .to_owned()
        }
    }

    pub fn same_city_group_count(&self) -> usize {
        self.same_city.as_ref().map_or(0, |sn| sn.groups.len())
    }

    pub fn tele_alias_count(&self) -> usize {
        self.same_city.as_ref().map_or(0, |sn| sn.aliases.len())
    }

    /// 有同城组的车站数（`sn.dat` 的「模糊站名」覆盖范围）。
    pub fn same_city_covered(&self) -> usize {
        let Some(sn) = &self.same_city else {
            return 0;
        };
        // 只要出现在任何组里（作为 gid 或成员）就算被覆盖；
        // 只挂在城市名节点下的车站（如成都东）也算，所以不能用「同城站数 > 1」过滤。
        (0..self.station_names.len())
            .filter(|&index| !sn.groups_with(index as i64).is_empty())
            .count()
    }

    /// 经由点对应的车站下标；`None` 表示它不是 `s.i` 里的车站
    /// （线路所 / 信号所 / 让乘降所，或该数据版本未收录的货运站）。
    pub fn station_of(&self, name: &str) -> Option<usize> {
        self.station_index.get(name).copied()
    }

    pub fn route_point_count(&self) -> usize {
        self.vias.iter().map(|via| via.points.len()).sum()
    }

    /// 经由点里属于 `s.i` 车站（可用 `query station` 查到）的数量。
    pub fn route_point_station_count(&self) -> usize {
        self.vias
            .iter()
            .flat_map(|via| &via.points)
            .filter(|point| self.station_of(&point.station).is_some())
            .count()
    }

    pub fn duty_count(&self) -> usize {
        self.duties.as_ref().map_or(0, |duties| duties.items.len())
    }

    pub fn dining_count(&self) -> usize {
        self.duties.as_ref().map_or(0, |duties| duties.dining.len())
    }
}

/// 解析 `routes.dat` + `sp.dat`，得到线路表与每趟车的经由。
/// 读 `t.rule`：每行「周期 掩码」，第 k 行即 `ruleIndex = k`。
fn load_rules(shards: &ShardDir, notes: &mut Vec<String>) -> Vec<RunRule> {
    let Some(bytes) = shards.read("t.rule").ok() else {
        notes.push("缺少 t.rule，开行计划日历不可用".to_owned());
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut rules = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(period), Some(mask)) = (parts.next(), parts.next()) else {
            continue;
        };
        match (period.parse::<i64>(), mask.parse::<i64>()) {
            (Ok(period), Ok(mask)) => rules.push(RunRule { period, mask }),
            _ => notes.push(format!("t.rule 有一行读不出来：{line}")),
        }
    }
    rules
}

/// 解析 `plat.dat`：车次 × 车站 -> 站台 / 出站口。
fn load_platforms(shards: &ShardDir, notes: &mut Vec<String>) -> Option<Platforms> {
    let Ok(bytes) = shards.read("plat.dat") else {
        notes.push("缺少 plat.dat，停站表没有站台信息".to_owned());
        return None;
    };
    match Platforms::parse(&bytes) {
        Ok(platforms) => Some(platforms),
        Err(err) => {
            notes.push(format!("plat.dat 解析失败：{err}"));
            None
        }
    }
}

/// 解析 `xw.dat`：同一趟车途中改用车次号的分段点。
fn load_changes(shards: &ShardDir, notes: &mut Vec<String>) -> Option<Changes> {
    let Ok(bytes) = shards.read("xw.dat") else {
        notes.push("缺少 xw.dat，显示车次只取车次名第一段".to_owned());
        return None;
    };
    match Changes::parse(&bytes) {
        Ok(changes) => Some(changes),
        Err(err) => {
            notes.push(format!("xw.dat 解析失败：{err}"));
            None
        }
    }
}

fn load_routes(shards: &ShardDir, notes: &mut Vec<String>) -> (Option<Routes>, Vec<TrainVia>) {
    let (Ok(route_bytes), Ok(sp_bytes)) = (shards.read("routes.dat"), shards.read("sp.dat")) else {
        notes.push("缺少 routes.dat 或 sp.dat，跳过经由信息".to_owned());
        return (None, Vec::new());
    };

    let codes = match spdata::train_codes(&sp_bytes) {
        Ok(codes) => codes,
        Err(err) => {
            notes.push(format!("sp.dat 解析失败：{err}"));
            return (None, Vec::new());
        }
    };

    let routes = match Routes::parse(&route_bytes) {
        Ok(routes) => routes,
        Err(err) => {
            notes.push(format!("routes.dat 解析失败：{err}"));
            return (None, Vec::new());
        }
    };

    match routes::check(&routes, &codes) {
        Ok(check_notes) => notes.extend(check_notes),
        Err(err) => notes.push(format!("经由数据异常：{err}")),
    }

    let vias = codes
        .iter()
        .enumerate()
        .filter(|(_, key)| routes.has_route(key))
        .map(|(index, key)| TrainVia {
            train_index: index,
            points: routes.via(key),
        })
        .collect();

    (Some(routes), vias)
}

/// 解析 `sn.dat`（同城车站组 / 城市名节点 / 电报码别名）。
fn load_same_city(
    shards: &ShardDir,
    station_count: usize,
    notes: &mut Vec<String>,
) -> Option<SameCity> {
    let Ok(bytes) = shards.read("sn.dat") else {
        notes.push("缺少 sn.dat，跳过同城车站信息".to_owned());
        return None;
    };
    match samecity::parse_checked(&bytes, station_count) {
        Ok((parsed, 0)) => Some(parsed),
        Ok((parsed, dropped)) => {
            notes.push(format!(
                "sn.dat 有 {dropped} 个组引用了不存在的车站，已忽略"
            ));
            Some(parsed)
        }
        Err(err) => {
            notes.push(format!("sn.dat 解析失败：{err}"));
            None
        }
    }
}

/// 解析 `jlb.dat`，并在有 `sp.dat` 时把每段关联到具体车次。
fn load_duties(
    shards: &ShardDir,
    train_codes: &[String],
    notes: &mut Vec<String>,
) -> (Option<Duties>, Vec<DutyTrain>) {
    let Ok(bytes) = shards.read("jlb.dat") else {
        notes.push("缺少 jlb.dat，跳过交路信息".to_owned());
        return (None, Vec::new());
    };

    let companies = match shards.read("extra.dat") {
        Ok(extra) => duty::read_companies(&extra).unwrap_or_default(),
        Err(_) => {
            notes.push("缺少 extra.dat，担当企业显示为编号".to_owned());
            Vec::new()
        }
    };

    let duties = match Duties::parse(&bytes, &companies) {
        Ok(duties) => duties,
        Err(err) => {
            notes.push(format!("jlb.dat 解析失败：{err}"));
            return (None, Vec::new());
        }
    };

    let links = resolve_duty_trains(&duties, train_codes);
    (Some(duties), links)
}

/// 把交路的每一段关联到 `t.i` 里的车次下标。
///
/// 交路串与 `t.i` 的写法常不同（`Z158/5` 对 `Z155/Z158`），
/// 因此用 [`same_train`] 按展开后的车次集合匹配。
fn resolve_duty_trains(duties: &Duties, train_codes: &[String]) -> Vec<DutyTrain> {
    let mut by_variant: HashMap<String, Vec<usize>> = HashMap::with_capacity(train_codes.len());
    for (index, code) in train_codes.iter().enumerate() {
        for variant in variants(code) {
            by_variant.entry(variant).or_default().push(index);
        }
    }

    let mut out = Vec::new();
    for (duty_id, duty) in duties.items.iter().enumerate() {
        for (position, segment) in duty.segments.iter().enumerate() {
            let mut matched: Vec<usize> = segment
                .code
                .split('/')
                .flat_map(variants)
                .filter_map(|variant| by_variant.get(&variant))
                .flatten()
                .copied()
                .collect();
            matched.sort_unstable();
            matched.dedup();
            for train_index in matched {
                out.push(DutyTrain {
                    duty_id,
                    seq: position + 1,
                    train_index,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::archive;

    fn shards() -> Option<ShardDir> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an");
        shards_from(&dir)
    }

    fn shards_from(dir: &std::path::Path) -> Option<ShardDir> {
        if !dir.join("routes.dat").exists() {
            return None;
        }
        Some(ShardDir::new(dir).unwrap())
    }

    #[test]
    fn loads_everything_from_fixture() {
        let Some(shards) = shards() else {
            return;
        };
        let data = crate::decode::load(&shards).unwrap();
        let extras = Extras::load(&shards, &data.train_codes, &data.stations);
        assert!(extras.routes.is_some());
        assert!(extras.duties.is_some());
        assert!(extras.vias.len() > 14_000, "{}", extras.vias.len());
        assert!(extras.route_point_count() > 400_000);
        // 经由点里既有车站（s.i），也有线路所 / 未收录站。
        assert!(extras.station_of("北京南").is_some());
        assert!(extras.station_of("京津所").is_none());
        let stations = extras.route_point_station_count();
        assert!(stations > 300_000, "{stations}");
        assert!(stations < extras.route_point_count());
        assert!(extras.duty_count() > 3_000);
        assert!(!extras.duty_trains.is_empty(), "交路应能关联到车次");
        assert!(
            extras.duty_trains.len() > 10_000,
            "{}",
            extras.duty_trains.len()
        );

        // 经由点总数与 tools/decode_routes.py 的 431364 接近；
        // 具体某趟车的经由正确性由 routes.rs 的测试覆盖。
        assert!(
            extras
                .vias
                .iter()
                .any(|via| via.points.len() > 20 && via.points.last().unwrap().km > 1000)
        );
    }

    #[test]
    fn missing_files_are_notes_not_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("s.i"), b"").unwrap();
        let shards = archive::ShardDir::new(tmp.path()).unwrap();
        let extras = Extras::load(&shards, &[], &[]);
        assert!(extras.routes.is_none());
        assert!(extras.duties.is_none());
        assert!(extras.duty_trains.is_empty());
        assert!(extras.vias.is_empty());
        assert!(extras.notes.iter().any(|note| note.contains("routes.dat")));
        assert!(extras.notes.iter().any(|note| note.contains("jlb.dat")));
    }
}
