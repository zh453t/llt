//! `sp.dat`：车次内部编号与车站附属信息的解码。
//!
//! 结构（`com.lltskb.lltskb.engine.SpDataMgr`，与 `tools/decode_sp.py` 一致）：
//!
//! ```text
//! S1  int32 n1   n1 × 16B { int32, int32, int32, base255, base255 }    TSDD 表
//! S2  int32 n2   n2 × (1B len + UTF-8)   车次内部编号（与 t.i 一一对应）
//! S3  base255 n3 n3 × (base255 id + 1B len + UTF-8)                   省
//! S4  int32 n4   n4 × (int32 + base255 + base255)                     车站 -> 省
//! S5  int32 n5   n5 × (base255 + 1B len + UTF-8)                      车站 -> 所在地
//! S6  int32 n6   n6 × (1B len + UTF-8 "站名,城市")                     站名 -> 城市别名
//! ```
//!
//! 这里只用到 S2：`routes.dat` 的车次经由表以内部编号为 key，
//! 需要它才能把 `train_index`（`t.i` 下标）映射到经由记录。

use anyhow::{Result, bail};

use crate::format::Reader;

/// 解析出的车次内部编号，下标即 `t.i` / `train_index`。
pub fn train_codes(bytes: &[u8]) -> Result<Vec<String>> {
    let mut reader = Reader::new(bytes);

    // S1：TSD D 表，每条 16 字节，直接跳过。
    let records = reader.count("sp.dat S1 条数")?;
    reader.skip(records * 16, "sp.dat S1 记录")?;

    // S2：车次内部编号。
    let count = reader.count("sp.dat S2 条数")?;
    let mut codes = Vec::with_capacity(count.min(32_768));
    for _ in 0..count {
        codes.push(reader.string("sp.dat S2 车次内部编号")?);
    }
    if count == 0 {
        bail!("sp.dat 里没有车次内部编号");
    }
    Ok(codes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> Option<PathBuf> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an/sp.dat");
        path.exists().then_some(path)
    }

    #[test]
    fn reads_train_codes() {
        let Some(path) = fixture() else {
            return;
        };
        let codes = train_codes(&std::fs::read(path).unwrap()).unwrap();
        assert!(codes.len() > 14_000, "{}", codes.len());
        assert!(codes.iter().all(|code| !code.is_empty()));
        assert!(codes.iter().any(|code| code.contains('G')));
    }

    #[test]
    fn rejects_truncated_input() {
        // S1 声明 1 条（16 字节）但数据不足。
        let bytes = [0u8, 0, 0, 1, 0, 0];
        assert!(train_codes(&bytes).is_err());
    }
}
