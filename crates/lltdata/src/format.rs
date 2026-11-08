//! 底层二进制编码（base255 / base128 / 索引文件 / 车次与车型标签）。
//!
//! 规则来自反编译 `engine.IndexMgr`、`engine.DataMgr`、`engine.QueryCC`，
//! 与 `tools/decode_llt.py` 保持一致。详见 `docs/llt-format-reverse-engineering.md`。

use anyhow::{Context, Result, bail};

/// `base255`：两字节大端，`b0 * 255 + b1`（0..=65279）。
#[inline]
pub fn b255(bytes: &[u8], offset: usize) -> u32 {
    debug_assert!(offset + 1 < bytes.len());
    u32::from(bytes[offset]) * 255 + u32::from(bytes[offset + 1])
}

/// `base128`：小端 7 位一字节，用于 YYYYMMDD。
#[inline]
pub fn b128(bytes: &[u8], offset: usize, len: usize) -> u32 {
    debug_assert!(offset + len <= bytes.len());
    bytes[offset..offset + len]
        .iter()
        .enumerate()
        .fold(0u32, |acc, (i, &b)| acc + (u32::from(b) << (7 * i)))
}

/// 读取索引文件：`u16be count`，随后 `count × (u16be len + UTF-8)`。
///
/// `s.i` = 车站编号 -> 站名，`t.i` = 车次编号 -> 车次号。
pub fn read_index(bytes: &[u8], label: &str) -> Result<Vec<String>> {
    if bytes.len() < 2 {
        bail!("{label} 只有 {} 字节，不是有效的索引文件", bytes.len());
    }
    let count = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
    let mut out = Vec::with_capacity(count);
    let mut offset = 2;
    for i in 0..count {
        if offset + 2 > bytes.len() {
            bail!("{label} 损坏：第 {} 条缺少长度字段", i + 1);
        }
        let len = usize::from(u16::from_be_bytes([bytes[offset], bytes[offset + 1]]));
        offset += 2;
        let end = offset + len;
        if end > bytes.len() {
            bail!(
                "{label} 损坏：第 {} 条字符串越界（需要 {len} 字节，只剩 {}）",
                i + 1,
                bytes.len() - offset
            );
        }
        let text = std::str::from_utf8(&bytes[offset..end])
            .with_context(|| format!("{label} 损坏：第 {} 条不是合法 UTF-8", i + 1))?;
        out.push(text.to_owned());
        offset = end;
    }
    Ok(out)
}

/// 二进制读取器：`int32` 大端、`base255` 两字节、`base128`、
/// 「1 字节长度 + UTF-8」字符串。`routes.dat` / `jlb.dat` / `sp.dat` 都用这套原语。
pub struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        if self.remaining() < n {
            bail!(
                "{what} 被截断：偏移 {} 需要 {n} 字节，只剩 {}",
                self.offset,
                self.remaining()
            );
        }
        let slice = &self.bytes[self.offset..self.offset + n];
        self.offset += n;
        Ok(slice)
    }

    /// `u16` 大端。
    pub fn u16(&mut self, what: &str) -> Result<usize> {
        let slice = self.take(2, what)?;
        Ok(usize::from(u16::from_be_bytes([slice[0], slice[1]])))
    }

    /// `int16` 大端（有符号，`sn.dat` 的节点 id 用它）。
    pub fn i16(&mut self, what: &str) -> Result<i16> {
        let slice = self.take(2, what)?;
        Ok(i16::from_be_bytes([slice[0], slice[1]]))
    }

    /// `u8`。
    pub fn u8(&mut self, what: &str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    /// Java `writeUTF`：u16 长度 + UTF-8。
    pub fn utf(&mut self, what: &str) -> Result<String> {
        let len = self.u16(what)?;
        let slice = self.take(len, what)?;
        String::from_utf8(slice.to_vec())
            .map_err(|_| anyhow::anyhow!("{what} 损坏：不是合法 UTF-8"))
    }

    /// `int32` 大端。
    pub fn i32(&mut self, what: &str) -> Result<i64> {
        let slice = self.take(4, what)?;
        Ok(i64::from(i32::from_be_bytes([
            slice[0], slice[1], slice[2], slice[3],
        ])))
    }

    /// 计数（`int32` 大端），负数视为损坏。
    pub fn count(&mut self, what: &str) -> Result<usize> {
        let value = self.i32(what)?;
        usize::try_from(value).map_err(|_| anyhow::anyhow!("{what} 损坏：计数为负数 {value}"))
    }

    /// `base255`：两字节大端，`b0 * 255 + b1`。
    pub fn b255(&mut self, what: &str) -> Result<u32> {
        let slice = self.take(2, what)?;
        Ok(u32::from(slice[0]) * 255 + u32::from(slice[1]))
    }

    pub fn skip(&mut self, n: usize, what: &str) -> Result<()> {
        self.take(n, what).map(|_| ())
    }

    /// 「1 字节长度 + UTF-8」。
    pub fn string(&mut self, what: &str) -> Result<String> {
        let len = usize::from(self.take(1, what)?[0]);
        let slice = self.take(len, what)?;
        String::from_utf8(slice.to_vec())
            .map_err(|_| anyhow::anyhow!("{what} 损坏：不是合法 UTF-8"))
    }
}

/// 按车次号前缀直接给出的车型（不看车型 id）。
const TYPE_FIXED: [(char, &str); 5] = [
    ('G', "高速动车"),
    ('D', "动车组"),
    ('C', "城际高速"),
    ('S', "市域"),
    ('Y', "旅游"),
];

/// 2026-11-25 车次改革：`V` 取代「数字车次」和 `K`，`N` 取代 `T`。
/// 数据里两种写法并存，标签按等价等级处理。
fn speed_alias(first: char) -> char {
    match first {
        'V' => 'K',
        'N' => 'T',
        other => other,
    }
}

/// 车次号去掉速度前缀：`K54` / `V54` / `K54/K51` -> `54`。
///
/// 等价于 Python 的 `re.match(r'[A-Z]*(\d+)', code.split('/')[0])`。
pub fn train_no(code: &str) -> &str {
    let head = code.split('/').next().unwrap_or("");
    let rest = head.trim_start_matches(|c: char| c.is_ascii_uppercase());
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    &rest[..end]
}

/// 车次的等价写法，用于在交路串里匹配同一趟车。
///
/// 铁路写法里斜杠后面是**回程车次的末尾几位**：
/// `G1956/7` = G1956 / G1957，`Z158/5` = Z158 / Z155，`Z170/67` = Z170 / Z167。
/// 末尾的字母是版本标记（`Z16B`，2026-11-25 车次改革后的新版），不参与匹配。
///
/// 交路串与 `t.i` 里同一趟车的写法常常不同（`Z158/5` 对 `Z155/Z158`），
/// 因此匹配时用两边展开后的集合求交集。
pub fn variants(code: &str) -> Vec<String> {
    let parts: Vec<&str> = code.split('/').collect();
    let Some(head) = parts.first() else {
        return Vec::new();
    };
    let letters: String = head
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    // 末尾字母是版本标记（如 Z16B），不参与匹配。
    let digits: String = head
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .to_owned();

    let mut out: Vec<String> = Vec::new();
    if !digits.is_empty() {
        out.push(format!("{letters}{digits}"));
    }
    for part in parts.iter().skip(1) {
        let tail: String = part.chars().filter(|c| c.is_ascii_digit()).collect();
        if tail.is_empty() || tail.len() > digits.len() {
            continue;
        }
        // 用回程的末尾几位替换去程的末尾几位。
        let keep = digits.len() - tail.len();
        let variant = format!("{letters}{}{tail}", &digits[..keep]);
        if !out.contains(&variant) {
            out.push(variant);
        }
    }
    if out.is_empty() {
        let trimmed = code.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_owned());
        }
    }
    out
}

/// 两组写法是否有交集（`Z158/5` 与 `Z155/Z158` 命中 `Z155` / `Z158`）。
#[cfg(test)]
pub fn same_train(a: &str, b: &str) -> bool {
    let right = variants(b);
    variants(a).iter().any(|variant| right.contains(variant))
}

/// 车型标签：车型 id 高 4 位表示新/空调，低 4 位表示等级。
pub fn type_name(train: &str, type_id: u8) -> String {
    let Some(first) = train.chars().next() else {
        return String::new();
    };
    if let Some((_, name)) = TYPE_FIXED.iter().find(|(c, _)| *c == first) {
        return (*name).to_owned();
    }

    let high = type_id & 0xF0;
    let low = type_id & 0x0F;
    let speed = speed_alias(first);

    let mut prefix = String::new();
    if high == 0 || high == 16 {
        prefix.push('新');
    }
    if high == 0 || high == 32 {
        prefix.push_str("空调");
    }

    let level = match low {
        7 => "市郊",
        4 | 6 => "高速",
        3 => "动车",
        1 => match speed {
            'K' | 'N' => "快速",
            'Z' => "直快",
            _ => "特快",
        },
        0 => "普快",
        _ => "普客",
    };
    format!("{prefix}{level}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base255_and_base128() {
        assert_eq!(b255(&[0x01, 0x02], 0), 257);
        assert_eq!(b255(&[0xFF, 0xFE], 0), 65279);
        // 20261105 = 0x1352A71 -> base128 小端 4 字节
        let date = 20261105u32;
        let encoded: Vec<u8> = (0..4).map(|i| ((date >> (7 * i)) & 0x7F) as u8).collect();
        assert_eq!(b128(&encoded, 0, 4), date);
    }

    #[test]
    fn index_round_trip() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u16.to_be_bytes());
        for name in ["北京南", "上海虹桥"] {
            bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
            bytes.extend_from_slice(name.as_bytes());
        }
        assert_eq!(read_index(&bytes, "s.i").unwrap(), ["北京南", "上海虹桥"]);
    }

    #[test]
    fn index_detects_truncation() {
        let bytes = [0u8, 2, 0, 8, b'a'];
        let err = read_index(&bytes, "s.i").unwrap_err().to_string();
        assert!(err.contains("越界"), "{err}");
    }

    #[test]
    fn train_no_strips_speed_prefix() {
        assert_eq!(train_no("K54"), "54");
        assert_eq!(train_no("V54"), "54");
        assert_eq!(train_no("G100/G97"), "100");
        assert_eq!(train_no("1461"), "1461");
        assert_eq!(train_no(""), "");
        assert_eq!(train_no("K"), "");
    }

    #[test]
    fn variants_expand_direction_and_version_marks() {
        assert_eq!(variants("G1956/7"), ["G1956", "G1957"]);
        assert_eq!(variants("Z158/5"), ["Z158", "Z155"]);
        assert_eq!(variants("Z170/67"), ["Z170", "Z167"]);
        assert_eq!(variants("Z155/Z158"), ["Z155", "Z158"]);
        assert_eq!(variants("Z16B"), ["Z16"]);
        assert_eq!(variants("1461"), ["1461"]);
        assert!(variants("").is_empty());
    }

    #[test]
    fn same_train_matches_app_style_codes() {
        // t.i 的写法 <-> 交路串的写法
        assert!(same_train("Z155/Z158", "Z158/5"));
        assert!(same_train("Z167/Z170", "Z170/67"));
        assert!(same_train("G1806/7", "G1806"));
        assert!(same_train("Z16B", "Z16"));
        // 不同等级/车次不能误判
        assert!(!same_train("G5", "D5"));
        assert!(!same_train("K54", "K53"));
    }

    #[test]
    fn type_names() {
        // 前缀直接决定
        assert_eq!(type_name("G1", 0x04), "高速动车");
        assert_eq!(type_name("C2000", 0x00), "城际高速");
        // 高 4 位 0 = 新空调；低 4 位 1 = 快速 / 特快
        assert_eq!(type_name("K54", 0x01), "新空调快速");
        assert_eq!(type_name("V54", 0x01), "新空调快速");
        assert_eq!(type_name("T10", 0x01), "新空调特快");
        assert_eq!(type_name("N10", 0x01), "新空调特快");
        assert_eq!(type_name("Z20", 0x01), "新空调直快");
        assert_eq!(type_name("1461", 0x00), "新空调普快");
        assert_eq!(type_name("1461", 0x20), "空调普快");
        assert_eq!(type_name("D1", 0x03), "动车组");
        // 空车次
        assert_eq!(type_name("", 0x00), "");
    }
}
