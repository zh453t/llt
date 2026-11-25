//! `xw.dat`：车次变化（同一趟车途中改用车次号）。
//!
//! 结构来自 `com.lltskb.lltskb.engine.DataMgr`，与 `tools/decode_xw.py` 一致：
//!
//! ```text
//! base255 n                                记录条数
//! n × 记录
//!     base255 trainIndex                   与 t.i 下标一致
//!     base255 v                            未使用（观察值多为 0）
//!     1B     k                             分段数
//!     k × { base255 km; 1B nameIdx }       切换点：里程 + t.i 里 "A/B" 的第几段
//! ```
//!
//! 判定（`QueryBase.OooO0o`）：把 `t.i` 的车次名按 `/` 拆开，取最后一个
//! `km <= 停站累计里程` 的分段；`k = 0` 或是没有记录时恒为第一段。
//! 实测 `A/B` 的书写顺序不等于运行顺序（`4167/4170` 首段是 1），所以不能只看车次名。

use std::collections::HashMap;

use anyhow::Result;

use crate::format::Reader;

/// 一个切换点：从 `km` 起显示车次名的第 `name` 段（0 起）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub km: u32,
    pub name: u8,
}

/// 解析后的 `xw.dat`：`train_index` -> 按里程升序的分段。
#[derive(Debug, Default)]
pub struct Changes {
    records: HashMap<u32, Vec<Segment>>,
    segment_count: usize,
}

impl Changes {
    /// 解析 `xw.dat`。记录不足时停止（与 Python 的 `while o < len(d) - 4` 一致）。
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let count = reader.b255("xw.dat 记录数")? as usize;
        let mut records = HashMap::with_capacity(count);
        let mut segment_count = 0;
        for _ in 0..count {
            // 每条记录至少 5 字节（两个 base255 + 1B 分段数）。
            if reader.remaining() < 5 {
                break;
            }
            let train_index = reader.b255("xw.dat 车次")?;
            reader.b255("xw.dat 保留字段")?;
            let segments = usize::from(reader.u8("xw.dat 分段数")?);
            let mut list = Vec::with_capacity(segments);
            for _ in 0..segments {
                list.push(Segment {
                    km: reader.b255("xw.dat 切换里程")?,
                    name: reader.u8("xw.dat 车次段")?,
                });
            }
            segment_count += list.len();
            // 同一车次出现多次时后者覆盖（Python 的 dict 行为）。
            records.insert(train_index, list);
        }
        Ok(Self {
            records,
            segment_count,
        })
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn segment_count(&self) -> usize {
        self.segment_count
    }

    /// 有分段的记录数（`k > 0`）。
    pub fn changed_record_count(&self) -> usize {
        self.records
            .values()
            .filter(|segments| !segments.is_empty())
            .count()
    }

    /// 该里程处应显示的车次号。`code` 是 `t.i` 里的完整车次名（可能带 `/`）。
    pub fn shown(&self, train_index: u32, code: &str, km: u32) -> String {
        let parts: Vec<&str> = code.split('/').collect();
        if parts.len() == 1 {
            return parts[0].to_owned();
        }
        let mut index = 0usize;
        if let Some(segments) = self.records.get(&train_index) {
            for segment in segments {
                if segment.km > km {
                    break;
                }
                index = usize::from(segment.name);
            }
        }
        parts.get(index).copied().unwrap_or(parts[0]).to_owned()
    }
}

/// 没有 `xw.dat` 时的近似：始终显示 `/` 前的第一段（App 在没有分段时也是这样）。
pub fn first_part(code: &str) -> &str {
    code.split('/').next().unwrap_or(code)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> Option<Vec<u8>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an/xw.dat");
        path.exists().then(|| std::fs::read(path).unwrap())
    }

    #[test]
    fn parses_repo_fixture() {
        let Some(bytes) = fixture() else {
            return;
        };
        let changes = Changes::parse(&bytes).unwrap();
        assert!(
            changes.record_count() > 10_000,
            "{}",
            changes.record_count()
        );
        assert!(
            changes.segment_count() > 10_000,
            "{}",
            changes.segment_count()
        );
        assert!(changes.changed_record_count() < changes.record_count());
    }

    #[test]
    fn picks_segment_by_km() {
        let bytes = [0, 1, 0, 7, 0, 0, 2, 0, 0, 0, 0, 10, 1];
        let changes = Changes::parse(&bytes).unwrap();
        // 0 km 显示第一段，10 km 起显示第二段。
        assert_eq!(changes.shown(7, "Z322/Z323", 0), "Z322");
        assert_eq!(changes.shown(7, "Z322/Z323", 9), "Z322");
        assert_eq!(changes.shown(7, "Z322/Z323", 10), "Z323");
        // 没有记录时回落到第一段。
        assert_eq!(changes.shown(99, "Z322/Z323", 400), "Z322");
        assert_eq!(changes.shown(7, "G1", 400), "G1");
    }

    #[test]
    fn out_of_range_segment_falls_back() {
        let bytes = [0, 1, 0, 7, 0, 0, 1, 0, 0, 5];
        let changes = Changes::parse(&bytes).unwrap();
        assert_eq!(changes.shown(7, "Z322/Z323", 100), "Z322");
    }
}
