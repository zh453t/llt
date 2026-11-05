//! `routes.dat`：线路表与车次经由。
//!
//! 结构（`com.lltskb.lltskb.engine.RoutesMgr`，与 `tools/decode_routes.py` 一致）：
//!
//! ```text
//! S1  int32 line_count
//!     line_count × { int32 line_code, str8 line_name, int32 item_count,
//!                    item_count × { str8 station, str8 tele_code, int32 flag, int32 km } }
//! S2  int32 route_count
//!     route_count × { str8 train_key, str8 end_station, int32 point_count,
//!                     point_count × { str8 station, int32 line_code } }
//! ```
//!
//! 车次经由表的 key **不是车次号**，而是 `sp.dat` 里的 12306 内部编号
//! （例如 `D6420/D6421` 对应 `5c000D642005`），映射见 [`crate::spdata`]。
//!
//! 经由点的位置由相邻两站在线路表里的区间推出，里程按「上一节点里程 +
//! 本节点与上一节点在线路表里的里程差」累计（复刻 App 的 `getRoutes()`）。

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::format::Reader;

/// 线路上的一个节点。
#[derive(Debug, Clone)]
pub struct LineItem {
    pub station: String,
    pub tele_code: String,
    pub flag: i64,
    pub km: i64,
}

/// 一条线路。
#[derive(Debug, Clone)]
pub struct Line {
    pub code: i64,
    pub name: String,
    pub items: Vec<LineItem>,
}

/// 车次经由表里的一条记录。
#[derive(Debug, Clone)]
pub struct TrainRoute {
    pub end_station: String,
    pub points: Vec<RoutePoint>,
}

/// 车次经由的一个节点：站名 + 所在线路。
#[derive(Debug, Clone)]
pub struct RoutePoint {
    pub station: String,
    pub line_code: i64,
}

/// 展开后的经由点（站名、线路名、累计里程），即 App「经由」页的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViaPoint {
    pub station: String,
    pub line: String,
    pub km: i64,
}

/// 解析后的 `routes.dat`。
#[derive(Debug)]
pub struct Routes {
    lines: HashMap<i64, Line>,
    trains: HashMap<String, TrainRoute>,
}

impl Routes {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);

        let line_count = reader.count("routes.dat 线路数")?;
        let mut lines = HashMap::with_capacity(line_count);
        for _ in 0..line_count {
            let code = reader.i32("routes.dat 线路编号")?;
            let name = reader.string("routes.dat 线路名")?;
            let item_count = reader.count("routes.dat 线路节点数")?;
            let mut items = Vec::with_capacity(item_count.min(1024));
            for _ in 0..item_count {
                items.push(LineItem {
                    station: reader.string("routes.dat 线路节点站名")?,
                    tele_code: reader.string("routes.dat 电报码")?,
                    flag: reader.i32("routes.dat 标记")?,
                    km: reader.i32("routes.dat 里程")?,
                });
            }
            // code 0 是占位模板行（线路名/车站名），App 里也不用它。
            if code != 0 {
                lines.insert(code, Line { code, name, items });
            }
        }

        let route_count = reader.count("routes.dat 车次经由数")?;
        let mut trains = HashMap::with_capacity(route_count);
        for _ in 0..route_count {
            let key = reader.string("routes.dat 车次编号")?;
            let end_station = reader.string("routes.dat 终点站")?;
            let point_count = reader.count("routes.dat 经由节点数")?;
            let mut points = Vec::with_capacity(point_count.min(1024));
            for _ in 0..point_count {
                points.push(RoutePoint {
                    station: reader.string("routes.dat 经由站名")?,
                    line_code: reader.i32("routes.dat 经由线路")?,
                });
            }
            trains.insert(
                key,
                TrainRoute {
                    end_station,
                    points,
                },
            );
        }
        Ok(Self { lines, trains })
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn route_count(&self) -> usize {
        self.trains.len()
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.values()
    }

    pub fn has_route(&self, key: &str) -> bool {
        self.trains.contains_key(key)
    }

    /// 展开某个车次（内部编号）的经由。
    pub fn via(&self, key: &str) -> Vec<ViaPoint> {
        let Some(route) = self.trains.get(key) else {
            return Vec::new();
        };

        let mut raw: Vec<ViaPoint> = Vec::new();
        let mut carry = 0;
        for (index, point) in route.points.iter().enumerate() {
            let next = match route.points.get(index + 1) {
                Some(next) => next.station.as_str(),
                None => route.end_station.as_str(),
            };
            let Some(line) = self.lines.get(&point.line_code) else {
                continue;
            };
            // 复刻 App 的查找方式：两站都找到之前持续覆盖下标，因此取到的是
            // 「最后一次出现」，同一线路绕行时结果才与 App 一致。
            let mut start = None;
            let mut end = None;
            for (index, item) in line.items.iter().enumerate() {
                if item.station == point.station {
                    start = Some(index);
                }
                if item.station == next {
                    end = Some(index);
                }
                if start.is_some() && end.is_some() {
                    break;
                }
            }
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };

            let span = if start <= end {
                &line.items[start..=end]
            } else {
                &line.items[end..=start]
            };
            let forward: Vec<&LineItem> = if start <= end {
                span.iter().collect()
            } else {
                span.iter().rev().collect()
            };

            let mut previous_raw = 0;
            for (position, item) in forward.iter().enumerate() {
                let km = if position == 0 {
                    carry
                } else {
                    let last = raw.last().map(|point| point.km).unwrap_or(carry);
                    last + (item.km - previous_raw).abs()
                };
                previous_raw = item.km;
                raw.push(ViaPoint {
                    station: item.station.clone(),
                    line: line.name.clone(),
                    km,
                });
                carry = km;
            }
        }

        // 相邻重复站合并，线路名拼成 "A/B"。
        let mut out: Vec<ViaPoint> = Vec::with_capacity(raw.len());
        let mut last = String::new();
        for point in raw {
            if point.station == last {
                if let Some(previous) = out.last_mut()
                    && !previous.line.split('/').any(|name| name == point.line)
                {
                    previous.line.push('/');
                    previous.line.push_str(&point.line);
                }
            } else {
                last = point.station.clone();
                out.push(point);
            }
        }
        out
    }
}

#[cfg(test)]
impl Routes {
    /// 测试用：只用给定线路构造，不含车次经由。
    pub fn from_lines(lines: Vec<Line>) -> Self {
        Self {
            lines: lines.into_iter().map(|line| (line.code, line)).collect(),
            trains: HashMap::new(),
        }
    }
}

/// 校验关系统的一致性，返回值得提示给用户的说明。
pub fn check(routes: &Routes, train_codes: &[String]) -> Result<Vec<String>> {
    if routes.line_count() == 0 {
        bail!("routes.dat 里没有线路");
    }
    if routes.route_count() == 0 {
        bail!("routes.dat 里没有车次经由");
    }
    let mut notes = Vec::new();
    if train_codes.len()
        != train_codes
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
    {
        notes.push("sp.dat 的车次内部编号有重复，经由映射可能不唯一".to_owned());
    }
    let missing = train_codes
        .iter()
        .filter(|code| !routes.has_route(code))
        .count();
    if missing > 0 {
        notes.push(format!("{missing} 趟车没有经由数据"));
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> Option<PathBuf> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an/routes.dat");
        path.exists().then_some(path)
    }

    fn load() -> Option<Routes> {
        let path = fixture()?;
        Some(Routes::parse(&std::fs::read(path).unwrap()).unwrap())
    }

    #[test]
    fn parses_lines_and_routes() {
        let Some(routes) = load() else {
            return;
        };
        assert!(routes.line_count() > 700, "{}", routes.line_count());
        assert!(routes.route_count() > 14_000, "{}", routes.route_count());
        // 占位线路 0 已被剔除。
        assert!(!routes.lines.contains_key(&0));
        let line = routes.lines().next().unwrap();
        assert!(!line.name.is_empty() && !line.items.is_empty());
    }

    #[test]
    fn via_matches_app_output() {
        let Some(routes) = load() else {
            return;
        };
        // 参考值由 tools/decode_routes.py 生成（20261108 数据）。
        let g1 = routes.via("24000000G10L");
        assert_eq!(g1.len(), 27);
        assert_eq!(
            g1.first().unwrap(),
            &ViaPoint {
                station: "北京南".to_owned(),
                line: "京沪高速线".to_owned(),
                km: 0
            }
        );
        assert_eq!(
            g1.get(1).unwrap(),
            &ViaPoint {
                station: "廊坊".to_owned(),
                line: "京沪高速线".to_owned(),
                km: 60
            }
        );
        assert_eq!(
            g1.last().unwrap(),
            &ViaPoint {
                station: "上海虹桥".to_owned(),
                line: "京沪高速线".to_owned(),
                km: 1318
            }
        );

        // K54：跨多条线，线路名用 '/' 合并。
        let k54 = routes.via("1200000K540D");
        assert_eq!(k54.len(), 67);
        assert_eq!(k54[1].station, "大成");
        assert_eq!(k54[1].line, "京哈线/皇姑屯线");
        assert_eq!(k54[1].km, 8);
        assert_eq!(k54[2].line, "皇姑屯线/沈山线");
        assert_eq!(k54.last().unwrap().km, 728);

        // 1461：全程京沪线，1463 km。
        let slow = routes.via("240000146100");
        assert_eq!(slow.len(), 61);
        assert_eq!(slow.last().unwrap().station, "上海");
        assert_eq!(slow.last().unwrap().km, 1463);
    }

    #[test]
    fn unknown_key_gives_empty_result() {
        let Some(routes) = load() else {
            return;
        };
        assert!(routes.via("no-such-key").is_empty());
        assert!(!routes.has_route("no-such-key"));
    }
}
