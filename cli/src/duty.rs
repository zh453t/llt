//! `jlb.dat`：交路（车底周转交路）、编组、担当企业、餐车。
//!
//! 结构（`com.lltskb.lltskb.engine.JlbDataMgr`，与 `tools/decode_jlb.py` 一致）：
//!
//! ```text
//! S1  int32 duty_count
//!     duty_count × { int32 start_date, int32 end_date, int32 reserved1, int32 reserved2,
//!                    base255 company_id, str8 formation, str8 chain }
//! S2  int32 dining_count
//!     dining_count × { int32 train_index, int32 start_date, int32 end_date, str8 coach }
//! ```
//!
//! - `chain` 形如 `DJ5904#G549#G748#G1049`：`#` 分隔交路里的每一段，
//!   `/` 表示同一车次的反向变体（如 `G1806/7`）。
//! - `company_id` 指向 `extra.dat` 首段的企业表。

use std::collections::HashSet;

use anyhow::Result;

use crate::format::{Reader, b255};

/// 交路里的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 原始写法，如 `G1806/7`。
    pub label: String,
    /// 段首车次号，用于匹配，如 `G1806`。
    pub code: String,
}

/// 一条交路记录。
#[derive(Debug, Clone)]
pub struct Duty {
    pub start_date: i64,
    pub end_date: i64,
    pub company: String,
    pub formation: String,
    /// 原始交路串，如 `DJ5904#G549#G748#G1049`。
    pub chain: String,
    pub segments: Vec<Segment>,
}

/// 一条餐车记录。
#[derive(Debug, Clone)]
pub struct DiningCar {
    pub train_index: i64,
    pub start_date: i64,
    pub end_date: i64,
    pub coach: String,
}

/// 解析后的 `jlb.dat`。
#[derive(Debug, Clone, Default)]
pub struct Duties {
    pub items: Vec<Duty>,
    pub dining: Vec<DiningCar>,
}

impl Duties {
    /// 解析 `jlb.dat`；`companies` 来自 [`read_companies`]。
    pub fn parse(bytes: &[u8], companies: &[String]) -> Result<Self> {
        let mut reader = Reader::new(bytes);

        let duty_count = reader.count("jlb.dat 交路数")?;
        let mut items = Vec::with_capacity(duty_count);
        for _ in 0..duty_count {
            let start_date = reader.i32("jlb.dat 交路起始日")?;
            let end_date = reader.i32("jlb.dat 交路终止日")?;
            reader.skip(8, "jlb.dat 交路保留字段")?;
            let company_id = reader.b255("jlb.dat 担当企业")? as usize;
            let formation = reader.string("jlb.dat 编组")?;
            let chain = reader.string("jlb.dat 交路串")?;
            let company = companies
                .get(company_id)
                .cloned()
                .unwrap_or_else(|| format!("?{company_id}"));
            items.push(Duty {
                start_date,
                end_date,
                company,
                formation,
                segments: split_chain(&chain),
                chain,
            });
        }

        let dining_count = reader.count("jlb.dat 餐车数")?;
        let mut dining = Vec::with_capacity(dining_count);
        // 原始数据里有少量完全重复的记录，这里按内容去重。
        let mut seen = HashSet::with_capacity(dining_count);
        for _ in 0..dining_count {
            let car = DiningCar {
                train_index: reader.i32("jlb.dat 餐车车次")?,
                start_date: reader.i32("jlb.dat 餐车起始日")?,
                end_date: reader.i32("jlb.dat 餐车终止日")?,
                coach: reader.string("jlb.dat 餐车车厢")?,
            };
            if seen.insert((
                car.train_index,
                car.start_date,
                car.end_date,
                car.coach.clone(),
            )) {
                dining.push(car);
            }
        }

        Ok(Self { items, dining })
    }
}

/// 把 `A#B/1#C` 拆成 `[A, B/1, C]`，同时记录段首车次号。
pub fn split_chain(chain: &str) -> Vec<Segment> {
    chain
        .split('#')
        .filter(|group| !group.is_empty())
        .map(|group| Segment {
            label: group.to_owned(),
            code: group.split('/').next().unwrap_or(group).to_owned(),
        })
        .collect()
}

/// `extra.dat` 首段：`base255 count` + `count × str8`，即担当企业表。
pub fn read_companies(bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.len() < 2 {
        return Ok(Vec::new());
    }
    let count = b255(bytes, 0) as usize;
    let mut reader = Reader::new(bytes);
    reader.skip(2, "extra.dat 企业表头")?;
    let mut out = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        match reader.string("extra.dat 企业名") {
            Ok(name) => out.push(name),
            // 企业表损坏时保留已读到的部分，交路仍可用（企业名显示 ?id）。
            Err(_) => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn data(name: &str) -> Option<Vec<u8>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../out/data/an")
            .join(name);
        path.exists().then(|| std::fs::read(path).unwrap())
    }

    #[test]
    fn splits_chain_into_segments() {
        let segments = split_chain("G1806/7#G82/79#G32#G1");
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0].label, "G1806/7");
        assert_eq!(segments[0].code, "G1806");
        assert_eq!(segments[3].code, "G1");
        assert!(split_chain("").is_empty());
        assert_eq!(split_chain("##K54#").len(), 1);
    }

    #[test]
    fn parses_real_file() {
        let (Some(jlb), Some(extra)) = (data("jlb.dat"), data("extra.dat")) else {
            return;
        };
        let companies = read_companies(&extra).unwrap();
        assert!(companies.len() > 300, "{}", companies.len());
        let duties = Duties::parse(&jlb, &companies).unwrap();
        assert!(duties.items.len() > 3_000, "{}", duties.items.len());
        // 去重后仍有一万多条，且不应再有重复。
        assert!(duties.dining.len() > 13_000, "{}", duties.dining.len());
        let unique: HashSet<_> = duties
            .dining
            .iter()
            .map(|car| {
                (
                    car.train_index,
                    car.start_date,
                    car.end_date,
                    car.coach.clone(),
                )
            })
            .collect();
        assert_eq!(unique.len(), duties.dining.len());
        // 少数记录的交路串为空，其余都能拆出至少一段。
        let with_segments = duties
            .items
            .iter()
            .filter(|duty| !duty.segments.is_empty())
            .count();
        assert!(
            with_segments * 100 / duties.items.len() > 90,
            "{with_segments}"
        );

        // 参考值来自 tools/decode_jlb.py（20261108 数据）。
        let duty = duties
            .items
            .iter()
            .find(|duty| duty.segments.iter().any(|segment| segment.code == "G1"))
            .expect("G1 的交路");
        assert_eq!(duty.company, "上局上海段");
        assert!(
            duty.formation.starts_with("CR400BF-S"),
            "{}",
            duty.formation
        );
        let labels: Vec<&str> = duty.segments.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            ["G1806/7", "G82/79", "G32", "G1", "G1956/7", "G1958/5"]
        );
        // 反向变体靠段首车次号匹配：G1807 只是 G1806/7 的另一种走法。
        let found = |code: &str| {
            duties
                .items
                .iter()
                .any(|duty| duty.segments.iter().any(|segment| segment.code == code))
        };
        assert!(found("G1806"));
        assert!(!found("G1807"));
    }

    #[test]
    fn truncated_file_is_an_error() {
        assert!(Duties::parse(&[0u8, 0, 0, 1], &[]).is_err());
    }
}
