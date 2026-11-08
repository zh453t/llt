//! 解码数据分片，得到站表 / 车次 / 停站。
//!
//! 布局与 `tools/decode_llt.py` 一致：
//!
//! ```text
//! 车次分片 tN.dat（车次 i 在 t[(i+1)%20].dat）
//!   key  2B base255 车次编号
//!   head 15B [0]车型id [1][2]保留 [3..6]开行起 YYYYMMDD [7..10]开行止
//!            [11..12] 未知 [13..14] base255 停站数 N
//!   停站 N × 7B：站号 base255 | 时 | 分 | 停留分 | 累计 km base255
//!
//! 车站分片 sN.dat（车站 i 在 s[(i+1)%10].dat）
//!   key 2B base255 站号 | count 2B base255 车次数 M | M × 2B base255 车次编号
//! ```

use std::fs;

use anyhow::{Context, Result, bail};

use crate::archive::{ShardDir, summarize};
use crate::format::{b128, b255, read_index, train_no, type_name};

/// 一趟车的全部信息。
#[derive(Debug)]
pub struct Train {
    pub index: u32,
    pub code: String,
    pub train_no: String,
    pub kind: String,
    pub date_from: u32,
    pub date_to: u32,
    pub stops: Vec<Stop>,
}

/// 一条停站记录（时间已按跨天累加）。
#[derive(Debug)]
pub struct Stop {
    pub station_index: u32,
    pub station: String,
    pub km: u32,
    pub arrive: String,
    pub depart: String,
    pub dwell: u8,
    pub day: u32,
    pub arrive_min: u32,
    pub depart_min: u32,
}

/// 解析后的完整数据集。
#[derive(Debug)]
pub struct Dataset {
    pub stations: Vec<String>,
    /// `t.i` 的车次号，下标即 `train_index`。
    pub train_codes: Vec<String>,
    pub trains: Vec<Train>,
    /// 与 `stations` 等长：`sN.dat` 里该站的车次数。
    pub station_train_counts: Vec<usize>,
    /// 值得告诉用户的非致命问题。
    pub notes: Vec<String>,
}

impl Dataset {
    pub fn stop_count(&self) -> usize {
        self.trains.iter().map(|train| train.stops.len()).sum()
    }
}

/// 从分片目录解出全部数据。
pub fn load(shards: &ShardDir) -> Result<Dataset> {
    let stations = read_index(&shards.read("s.i")?, "s.i")?;
    let codes = read_index(&shards.read("t.i")?, "t.i")?;

    let mut trains = Vec::new();
    for shard in 0..20 {
        let name = format!("t{shard}.dat");
        let bytes = shards.read(&name)?;
        let mut parsed = parse_train_shard(&bytes, &stations, &codes)
            .with_context(|| format!("解析 {name} 失败"))?;
        trains.append(&mut parsed);
    }
    trains.sort_by_key(|train| train.index);

    let mut station_train_counts = vec![0usize; stations.len()];
    let mut missing_shards = Vec::new();
    for shard in 0..10 {
        let name = format!("s{shard}.dat");
        let Some(path) = shards.path(&name) else {
            missing_shards.push(name);
            continue;
        };
        let bytes = fs::read(&path).with_context(|| format!("无法读取 {}", path.display()))?;
        for (station, list) in
            parse_station_shard(&bytes).with_context(|| format!("解析 {name} 失败"))?
        {
            if let Some(slot) = station_train_counts.get_mut(station as usize) {
                *slot = list.len();
            }
        }
    }

    let notes = collect_notes(&trains, &codes, &stations, &missing_shards);

    Ok(Dataset {
        stations,
        train_codes: codes,
        trains,
        station_train_counts,
        notes,
    })
}

fn collect_notes(
    trains: &[Train],
    codes: &[String],
    stations: &[String],
    missing_shards: &[String],
) -> Vec<String> {
    let mut notes = Vec::new();
    if !missing_shards.is_empty() {
        notes.push(format!(
            "缺少 {} 个车站分片（{}），stations.csv 的「车站车次数」会记为 0",
            missing_shards.len(),
            summarize(missing_shards)
        ));
    }

    let unnamed_trains = trains
        .iter()
        .filter(|train| train.index as usize >= codes.len())
        .count();
    if unnamed_trains > 0 {
        notes.push(format!(
            "{unnamed_trains} 个车次编号超出 t.i 范围，车次号用 ?<编号> 占位"
        ));
    }

    let unknown_stops = trains
        .iter()
        .flat_map(|train| &train.stops)
        .filter(|stop| stop.station_index as usize >= stations.len())
        .count();
    if unknown_stops > 0 {
        notes.push(format!(
            "{unknown_stops} 条停站记录的站号超出 s.i 范围，站名用 ?<编号> 占位"
        ));
    }
    notes
}

/// 解析一个车次分片。记录长度不足时保留已解析部分，与 Python 实现一致。
fn parse_train_shard(bytes: &[u8], stations: &[String], codes: &[String]) -> Result<Vec<Train>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset + 17 <= bytes.len() {
        let index = b255(bytes, offset);
        let header = &bytes[offset + 2..offset + 17];
        let count = b255(header, 13) as usize;

        let mut raw = Vec::with_capacity(count.min(64));
        let mut cursor = offset + 17;
        for _ in 0..count {
            if cursor + 7 > bytes.len() {
                break;
            }
            let record = &bytes[cursor..cursor + 7];
            let station_index = b255(record, 0);
            raw.push(RawStop {
                station_index,
                station: station_name(stations, station_index),
                hour: record[2],
                minute: record[3],
                dwell: record[4],
                km: b255(record, 5),
            });
            cursor += 7;
        }

        let code = codes
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| format!("?{index}"));
        out.push(Train {
            index,
            train_no: train_no(&code).to_owned(),
            kind: type_name(&code, header[0]),
            code,
            date_from: b128(header, 3, 4),
            date_to: b128(header, 7, 4),
            stops: build_stops(raw),
        });
        offset = cursor;
    }
    Ok(out)
}

struct RawStop {
    station_index: u32,
    station: String,
    hour: u8,
    minute: u8,
    dwell: u8,
    km: u32,
}

/// 把「时:分 + 停留」换算成自首站起的绝对分钟数，并处理跨天。
fn build_stops(raw: Vec<RawStop>) -> Vec<Stop> {
    let mut stops = Vec::with_capacity(raw.len());
    let mut previous_departure: Option<u32> = None;

    for stop in raw {
        let clock = u32::from(stop.hour) * 60 + u32::from(stop.minute);
        let (arrive_min, day) = match previous_departure {
            None => (clock, 1),
            Some(previous) => {
                let mut day = 0;
                while clock + 1440 * day < previous {
                    day += 1;
                }
                (clock + 1440 * day, day + 1)
            }
        };
        let depart_min = arrive_min + u32::from(stop.dwell);

        stops.push(Stop {
            station_index: stop.station_index,
            station: stop.station,
            km: stop.km,
            arrive: hhmm(arrive_min),
            depart: hhmm(depart_min),
            dwell: stop.dwell,
            day,
            arrive_min,
            depart_min,
        });
        previous_departure = Some(depart_min);
    }
    stops
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

fn station_name(stations: &[String], index: u32) -> String {
    stations
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| format!("?{index}"))
}

/// 解析车站分片：站号 -> 车次编号列表。
fn parse_station_shard(bytes: &[u8]) -> Result<Vec<(u32, Vec<u32>)>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        let station = b255(bytes, offset);
        let count = b255(bytes, offset + 2) as usize;
        offset += 4;

        let mut list = Vec::with_capacity(count.min(1024));
        for i in 0..count {
            if offset + 2 > bytes.len() {
                bail!("记录被截断：车站 {station} 的第 {} 个车次编号缺失", i + 1);
            }
            list.push(b255(bytes, offset));
            offset += 2;
        }
        out.push((station, list));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::progress::Silent;

    fn fixture() -> Option<PathBuf> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an.db");
        path.exists().then_some(path)
    }

    #[test]
    fn decodes_repo_fixture() {
        let Some(archive) = fixture() else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let shards = crate::archive::unpack(&archive, tmp.path(), &Silent).unwrap();
        let data = load(&shards).unwrap();

        assert!(data.stations.len() > 3_000, "{}", data.stations.len());
        assert!(data.trains.len() > 13_000, "{}", data.trains.len());
        assert!(data.stop_count() > 120_000, "{}", data.stop_count());
        assert!(
            data.trains
                .windows(2)
                .all(|pair| pair[0].index < pair[1].index),
            "车次应按编号排序"
        );

        // README 里的已知样例：G1 北京南 06:30 -> 上海虹桥 11:24，1318 km。
        let g1 = data
            .trains
            .iter()
            .find(|train| train.code == "G1" && train.stops.len() > 2)
            .expect("G1");
        assert_eq!(g1.stops.first().unwrap().station, "北京南");
        assert_eq!(g1.stops.first().unwrap().depart, "06:30");
        assert_eq!(g1.stops.last().unwrap().station, "上海虹桥");
        assert_eq!(g1.stops.last().unwrap().arrive, "11:24");
        assert_eq!(g1.stops.last().unwrap().km, 1318);
        assert_eq!(g1.train_no, "1");

        // 跨天：1461 北京 11:59 -> 次日 06:45，1463 km。
        let t1461 = data
            .trains
            .iter()
            .find(|train| train.code == "1461")
            .expect("1461");
        let last = t1461.stops.last().unwrap();
        assert_eq!(last.station, "上海");
        assert_eq!(last.arrive, "06:45");
        assert_eq!(last.day, 2);
        assert_eq!(last.km, 1463);
    }

    #[test]
    fn keeps_partially_parsed_shard() {
        // 15 字节头 + 1 条完整停站 + 1 条残缺停站。
        let mut bytes = vec![0x00, 0x00];
        let mut header = [0u8; 15];
        header[13] = 0;
        header[14] = 2;
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&[0x00, 0x01, 7, 30, 3, 0x00, 0x0A]);
        bytes.extend_from_slice(&[0x00]);

        let stations = vec!["甲".to_string(), "乙".to_string(), "丙".to_string()];
        let codes = vec!["G1".to_string()];
        let trains = parse_train_shard(&bytes, &stations, &codes).unwrap();
        assert_eq!(trains.len(), 1);
        assert_eq!(trains[0].index, 0);
        assert_eq!(trains[0].code, "G1");
        assert_eq!(trains[0].stops.len(), 1);
        assert_eq!(trains[0].stops[0].station, "乙");
        assert_eq!(trains[0].stops[0].arrive, "07:30");
        assert_eq!(trains[0].stops[0].depart, "07:33");
    }

    #[test]
    fn rolls_day_over_midnight() {
        let raw = vec![
            RawStop {
                station_index: 0,
                station: "甲".into(),
                hour: 23,
                minute: 50,
                dwell: 5,
                km: 0,
            },
            RawStop {
                station_index: 1,
                station: "乙".into(),
                hour: 0,
                minute: 20,
                dwell: 0,
                km: 100,
            },
            RawStop {
                station_index: 2,
                station: "丙".into(),
                hour: 1,
                minute: 0,
                dwell: 0,
                km: 150,
            },
        ];
        let stops = build_stops(raw);
        assert_eq!(stops[0].day, 1);
        assert_eq!(stops[0].depart_min, 23 * 60 + 55);
        assert_eq!(stops[1].day, 2);
        assert_eq!(stops[1].arrive, "00:20");
        assert_eq!(stops[1].arrive_min, 1440 + 20);
        assert_eq!(stops[2].day, 2);
        assert_eq!(stops[2].arrive_min, 1440 + 60);
    }

    #[test]
    fn detects_truncated_station_shard() {
        // 站号 3，车次数 2，但只有 1 个车次编号。
        let bytes = [0x00, 0x03, 0x00, 0x02, 0x00, 0x01];
        let err = parse_station_shard(&bytes).unwrap_err().to_string();
        assert!(err.contains("截断"), "{err}");
    }
}
