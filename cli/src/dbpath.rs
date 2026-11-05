//! 数据库路径定位与“本地库是否可用”的检查。
//!
//! 定位顺序见 [`lltdb::path`]（GUI 与 CLI 共用一份实现）。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::export::META_JSON;

/// `--csv` 模式必须存在的文件。
const CSV_FILES: [&str; 6] = [
    "stations.csv",
    "trains.csv",
    "stops.csv",
    "train_routes.csv",
    "duties.csv",
    "same_city.csv",
];

/// 当前版本写入的全部表；缺任何一张都说明库是旧工具建的或没写完。
pub const REQUIRED_TABLES: [&str; 15] = [
    "meta",
    "stations",
    "trains",
    "stops",
    "lines",
    "line_stations",
    "train_routes",
    "duties",
    "duty_segments",
    "duty_trains",
    "dining_cars",
    "city_nodes",
    "same_city_groups",
    "same_city_members",
    "tele_aliases",
];

/// 与 [`resolve`] 相同，但不存在时返回 `None`。
pub fn find(explicit: Option<&Path>, strict: bool) -> Option<PathBuf> {
    lltdb::path::find_optional(explicit, strict).map(|located| located.path)
}

/// 本地库的状态。
#[derive(Debug, Clone, Default)]
pub struct LocalState {
    pub path: PathBuf,
    /// `meta.data_version`，缺失时回退到 `PRAGMA user_version`。
    pub data_version: Option<String>,
    pub schema_version: Option<i64>,
    pub generator: Option<String>,
    pub built_at: Option<String>,
    /// 上次构建时线上声明的 APK 版本。
    pub remote_app_version: Option<String>,
    /// 缺表 / 行数不符等问题；非空即“不可直接跳过”。
    pub problems: Vec<String>,
}

impl LocalState {
    /// 读取本地库；完全没有库时返回 `None`。
    pub fn inspect(explicit: Option<&Path>, strict: bool) -> Option<Self> {
        let path = find(explicit, strict)?;
        Some(Self::read(path))
    }

    /// 读取 `--csv` 产物目录里的 `meta.json`；没有则返回 `None`。
    pub fn inspect_csv(dir: &Path) -> Option<Self> {
        let path = dir.join(META_JSON);
        let text = std::fs::read_to_string(&path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let field = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };

        let mut state = Self {
            path,
            data_version: field("data_version"),
            schema_version: value
                .get("schema_version")
                .and_then(serde_json::Value::as_i64),
            generator: field("generator"),
            built_at: field("built_at"),
            remote_app_version: field("remote_app_version"),
            problems: Vec::new(),
        };
        let missing: Vec<&str> = CSV_FILES
            .iter()
            .copied()
            .filter(|name| !dir.join(name).is_file())
            .collect();
        if !missing.is_empty() {
            state
                .problems
                .push(format!("缺少 CSV 文件：{}", missing.join(", ")));
        }
        Some(state)
    }

    /// 直接读取指定库（不检查存在性）。
    pub fn read(path: PathBuf) -> Self {
        let mut state = Self {
            path: path.clone(),
            ..Default::default()
        };
        let Ok(conn) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            state.problems.push("无法打开数据库".to_owned());
            return state;
        };

        let version = meta_value(&conn, "data_version");
        state.data_version = version.or_else(|| {
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .ok()
                .filter(|value| *value > 0)
                .map(|value| value.to_string())
        });
        state.schema_version =
            meta_value(&conn, "schema_version").and_then(|text| text.parse().ok());
        state.generator = meta_value(&conn, "generator");
        state.built_at = meta_value(&conn, "built_at");
        state.remote_app_version = meta_value(&conn, "remote_app_version");

        if state.schema_version.is_none() {
            state
                .problems
                .push("没有 meta 表（可能是 tools/decode_llt.py 生成的旧库）".to_owned());
        }

        let missing: Vec<&str> = REQUIRED_TABLES
            .iter()
            .copied()
            .filter(|table| !has_table(&conn, table))
            .collect();
        if !missing.is_empty() {
            state
                .problems
                .push(format!("缺少数据表：{}", missing.join(", ")));
        }

        // 行数一致性：上一次构建若被占用/中断，会留下数目对不上的库。
        for (key, table) in [
            ("station_count", "stations"),
            ("train_count", "trains"),
            ("stop_count", "stops"),
        ] {
            let Some(expected) = meta_value(&conn, key).and_then(|text| text.parse::<i64>().ok())
            else {
                continue;
            };
            let actual = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap_or(-1);
            if expected != actual {
                state.problems.push(format!(
                    "{table} 行数不符（meta 记 {expected}，实际 {actual}）"
                ));
            }
        }

        state
    }

    /// 库本身可用（表齐全、行数一致）。
    pub fn usable(&self) -> bool {
        self.problems.is_empty()
    }

    /// 结构版本是否与当前工具一致。
    pub fn schema_current(&self, expected: i64) -> bool {
        self.schema_version == Some(expected)
    }
}

fn meta_value(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("LLT_TEST_DB") {
            let path = PathBuf::from(path);
            return path.is_file().then_some(path);
        }
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/timetable/llt-timetable.sqlite");
        path.exists().then_some(path)
    }

    #[test]
    fn reads_versions_from_fixture() {
        let Some(path) = fixture() else {
            return;
        };
        let state = LocalState::read(path);
        assert!(state.data_version.is_some(), "{:?}", state.problems);
        assert!(state.schema_version.is_some());
        assert!(state.built_at.is_some());
        assert!(state.usable(), "{:?}", state.problems);
    }

    #[test]
    fn missing_file_is_none() {
        assert!(LocalState::inspect(Some(Path::new("no-such.sqlite")), false).is_none());
    }

    #[test]
    fn reports_broken_database() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE stations (station_index INTEGER)")
            .unwrap();
        let state = LocalState::read(path);
        assert!(!state.usable());
        assert!(state.problems.iter().any(|note| note.contains("meta")));
        assert!(
            state
                .problems
                .iter()
                .any(|note| note.contains("缺少数据表"))
        );
    }
}
