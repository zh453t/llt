//! `sn.dat`：同城车站组、城市名虚拟节点、站名-电报码别名。
//!
//! 结构（`com.lltskb.lltskb.engine.FuzzyStationMgr.init()`，
//! 与 `tools/decode_sn.py` 一致；整文件是 Java `DataInputStream` 风格，大端）：
//!
//! ```text
//! S1  u16 n1;  n1 × writeUTF（u16 长度 + UTF-8）        城市/地区名，虚拟节点 id 从 10000 起
//! S2  u16 n2;  n2 × { s16 gid, u8 k, k × s16 车站索引 }  同城组
//! S3  u16 n3;  n3 × (u8 长度 + UTF-8 "站名-电报码")      别名
//! ```
//!
//! - `gid` 是组的规范节点：车站下标（`< s.i` 条数）或 S1 的虚拟 id（`>= 10000`，代表“城市名”）。
//! - **`gid` 自身不出现在自己的成员向量里**，App 的 `OooO0Oo()` 查询时把它插到首位。
//! - 因此同一个城市可能出现两个互为镜像的组（如天津的 `192(滨海)` 与 `2310(天津)`），
//!   成员仅差自身；取并集才是完整同城列表。
//! - `ResMgr.getCity(name) = getStationName(OooO0O0(getStationIndex(name)))`，
//!   其中 `OooO0O0()` 就是“找包含该站的组的 gid”。

use anyhow::{Context, Result, bail};

use crate::format::Reader;

/// 城市名虚拟节点的起始 id。
pub const VIRTUAL_BASE: i64 = 10_000;

/// 一个同城组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// 组的规范节点：车站下标，或城市名虚拟 id（`>= 10000`）。
    pub id: i64,
    /// 成员车站下标，**不含** `id` 自身。
    pub members: Vec<i64>,
}

impl Group {
    pub fn is_city_node(&self) -> bool {
        self.id >= VIRTUAL_BASE
    }

    /// 组内全部节点（`id` 自身插到首位）。
    pub fn nodes(&self) -> Vec<i64> {
        let mut out = Vec::with_capacity(self.members.len() + 1);
        out.push(self.id);
        out.extend(self.members.iter().copied().filter(|&m| m != self.id));
        out
    }
}

/// 解析后的 `sn.dat`。
#[derive(Debug, Clone, Default)]
pub struct SameCity {
    /// S1：城市/地区名，下标 + [`VIRTUAL_BASE`] 即虚拟节点 id。
    pub cities: Vec<String>,
    /// S2：同城组。
    pub groups: Vec<Group>,
    /// S3：`(站名, 电报码)`。
    pub aliases: Vec<(String, String)>,
}

impl SameCity {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);

        let city_count = reader.u16("sn.dat S1 条数")?;
        let mut cities = Vec::with_capacity(city_count);
        for _ in 0..city_count {
            cities.push(reader.utf("sn.dat S1 城市名")?);
        }

        let group_count = reader.u16("sn.dat S2 组数")?;
        let mut groups = Vec::with_capacity(group_count);
        for _ in 0..group_count {
            let id = i64::from(reader.i16("sn.dat S2 组 id")?);
            let size = usize::from(reader.u8("sn.dat S2 成员数")?);
            let mut members = Vec::with_capacity(size);
            for _ in 0..size {
                members.push(i64::from(reader.i16("sn.dat S2 成员")?));
            }
            groups.push(Group { id, members });
        }

        let alias_count = reader.u16("sn.dat S3 条数")?;
        let mut aliases = Vec::with_capacity(alias_count);
        for _ in 0..alias_count {
            let text = reader.string("sn.dat S3 别名")?;
            let (station, code) = text
                .split_once('-')
                .map_or((text.as_str(), ""), |(station, code)| (station, code));
            aliases.push((station.to_owned(), code.to_owned()));
        }

        if groups.is_empty() {
            bail!("sn.dat 里没有同城组");
        }
        Ok(Self {
            cities,
            groups,
            aliases,
        })
    }

    /// 节点 id -> 名字（城市名节点或车站名）。
    pub fn node_name(&self, id: i64) -> Option<&str> {
        if id >= VIRTUAL_BASE {
            self.cities
                .get((id - VIRTUAL_BASE) as usize)
                .map(String::as_str)
        } else {
            None
        }
    }

    /// 包含某站的全部组（可能有镜像组）。
    pub fn groups_with(&self, station_index: i64) -> Vec<&Group> {
        self.groups
            .iter()
            .filter(|group| group.id == station_index || group.members.contains(&station_index))
            .collect()
    }
}

/// 解析 `sn.dat`，并校验节点下标没有越界。
///
/// 越界的组会被丢弃（返回被丢弃的数量），避免后续按 `stations` 取值时出错。
pub fn parse_checked(bytes: &[u8], station_count: usize) -> Result<(SameCity, usize)> {
    let parsed = SameCity::parse(bytes).context("解析 sn.dat 失败")?;
    let valid = station_count as i64;
    let (groups, dropped): (Vec<_>, Vec<_>) = parsed.groups.into_iter().partition(|group| {
        (group.id >= VIRTUAL_BASE || group.id < valid)
            && group
                .members
                .iter()
                .all(|&member| (0..valid).contains(&member))
    });
    Ok((SameCity { groups, ..parsed }, dropped.len()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn data() -> Option<Vec<u8>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an/sn.dat");
        path.exists().then(|| std::fs::read(path).unwrap())
    }

    #[test]
    fn parses_real_file() {
        let Some(bytes) = data() else {
            return;
        };
        let sn = SameCity::parse(&bytes).unwrap();
        assert_eq!(sn.cities.len(), 73, "城市名节点数");
        assert_eq!(sn.groups.len(), 658, "同城组数");
        assert_eq!(sn.aliases.len(), 364, "电报码别名数");

        // 组大小：最大 12（含 gid 自身）、中位 2。
        let mut sizes: Vec<usize> = sn.groups.iter().map(|g| g.nodes().len()).collect();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(sizes[0], 12);
        assert_eq!(sizes[sizes.len() / 2], 2);
        // gid 自身不应出现在自己的成员里。
        assert!(sn.groups.iter().all(|g| !g.members.contains(&g.id)));
    }

    #[test]
    fn beijing_group_has_eleven_stations() {
        let Some(bytes) = data() else {
            return;
        };
        let sn = SameCity::parse(&bytes).unwrap();
        // 北京市站的下标是 165（20261108 数据），组 gid 也是 165。
        let group = sn
            .groups
            .iter()
            .find(|group| group.id == 165)
            .expect("北京组");
        assert_eq!(group.nodes().len(), 11);
        assert_eq!(
            sn.cities.is_empty(),
            sn.groups.iter().all(|group| !group.is_city_node())
        );
    }

    #[test]
    fn tianjin_mirror_groups_merge() {
        let Some(bytes) = data() else {
            return;
        };
        let sn = SameCity::parse(&bytes).unwrap();
        // 天津（2310）落在镜像组 192(滨海) 里，自身也是组 2310 的 gid。
        let mirrors = sn.groups_with(2310);
        assert!(mirrors.len() >= 2, "{}", mirrors.len());
        // 合并镜像组（只保留车站节点，虚拟节点单独统计）。
        let mut merged: Vec<i64> = Vec::new();
        for group in &mirrors {
            for node in group.nodes() {
                if node < VIRTUAL_BASE && !merged.contains(&node) {
                    merged.push(node);
                }
            }
        }
        assert_eq!(merged.len(), 12);
        assert!(merged.contains(&2310));
    }

    #[test]
    fn city_node_lists_stations() {
        let Some(bytes) = data() else {
            return;
        };
        let sn = SameCity::parse(&bytes).unwrap();
        // s.i 里没有「成都」（站改中），但 S1 有城市名节点，成员是成都东/南/西/犀浦。
        let node = 10_004;
        let group = sn
            .groups
            .iter()
            .find(|group| group.id == node)
            .expect("成都节点");
        assert_eq!(group.members.len(), 4);
        assert!(group.is_city_node());
        assert_eq!(sn.node_name(node), Some("成都"));
        assert_eq!(sn.node_name(10_003), Some("朝阳南"));
    }

    #[test]
    fn rejects_truncated_file() {
        assert!(SameCity::parse(&[0, 1, 0]).is_err());
    }
}
