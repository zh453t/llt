//! `plat.dat`：车次 × 车站 → 站台 / 出站口 / 检票口提示。
//!
//! 结构来自 `com.lltskb.lltskb.engine.PlatformMgr`，与 `tools/decode_plat.py` 一致：
//!
//! ```text
//! u32 count                              记录条数
//! count × 记录
//!     u32 trainIndex                     与 t.i 下标一致
//!     u32 stationIndex                   与 s.i 下标一致
//!     1B  len
//!     len 字节 UTF-8                     站台文本，如 "13"、"3站台"、"出站口/站台"
//! ```
//!
//! map 的 key 是 `"trainIndex_stationIndex"`，同一 (车次, 车站) 只保留最后一条。
//! 界面上显示在详情页「出站口/站台」列；只有办客的主要停靠站有值，
//! 20261127 数据里 97,394 条覆盖 11,906 / 15,151 趟车。

use std::collections::HashMap;

use anyhow::Result;

use crate::format::Reader;

/// 解析后的 `plat.dat`。
#[derive(Debug, Default)]
pub struct Platforms {
    map: HashMap<(u32, u32), String>,
}

impl Platforms {
    /// 解析 `plat.dat`。
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let count = reader.count("plat.dat 记录数")?;
        let mut map = HashMap::with_capacity(count);
        for _ in 0..count {
            // 每条记录至少 9 字节（两个 u32 + 1B 长度）。
            if reader.remaining() < 9 {
                break;
            }
            let train_index = reader.u32("plat.dat 车次")?;
            let station_index = reader.u32("plat.dat 车站")?;
            let text = reader.string("plat.dat 站台")?;
            map.insert((train_index, station_index), text);
        }
        Ok(Self { map })
    }

    /// 该车次在该站的站台 / 出站口提示；没有时返回 `None`。
    pub fn get(&self, train_index: u32, station_index: u32) -> Option<&str> {
        self.map
            .get(&(train_index, station_index))
            .map(String::as_str)
            .filter(|text| !text.is_empty())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> Option<Vec<u8>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an/plat.dat");
        path.exists().then(|| std::fs::read(path).unwrap())
    }

    #[test]
    fn parses_repo_fixture() {
        let Some(bytes) = fixture() else {
            return;
        };
        let platforms = Platforms::parse(&bytes).unwrap();
        assert!(platforms.len() > 90_000, "{}", platforms.len());
    }

    #[test]
    fn reads_train_station_pairs() {
        // count = 1；车次 3、车站 5、站台 "13"。
        let bytes = [0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0, 5, 2, b'1', b'3'];
        let platforms = Platforms::parse(&bytes).unwrap();
        assert_eq!(platforms.get(3, 5), Some("13"));
        assert_eq!(platforms.get(5, 3), None);
    }
}
