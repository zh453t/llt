//! 界面偏好的持久化：`llt-gui.json`（紧挨可执行文件，绿色版可直接带走）。
//!
//! 只存「下次打开还想保留」的少量状态：当前标签页、三种查询方式的查询词、
//! 车站/站站查询的模糊开关、车次详情经由的「只看车站」。会话现场（查询日期、选中车次、
//! 同城车站勾选、站站结果筛选）不落盘，重启后回到干净状态。
//!
//! 读写策略：
//!
//! - 读取失败一律回退默认值，绝不阻断启动；语法错的文件改名 `.bad` 留证；
//! - 写入先落临时文件再 `rename` 覆盖，避免写一半留下截断的 JSON；
//! - 所有结构体带 `#[serde(default)]`、忽略未知字段：老文件缺字段、新版加字段都不报错。

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 配置文件默认文件名（放在可执行文件旁）。
pub const CONFIG_FILE: &str = "llt-gui.json";
/// 当前格式版本；只在未来做不兼容迁移时才会变。
pub const CONFIG_VERSION: u32 = 1;
/// 查询词的长度上限：配置是手改的，防止一个误粘贴的巨型字符串拖慢界面。
const MAX_QUERY_CHARS: usize = 32;

/// 三个标签页。
///
/// 声明顺序就是界面顺序，也是第一次启动时的默认页（站站查询）。
/// 落盘时用 `snake_case`（`od` / `train` / `station`），手改配置也读得懂。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    #[default]
    Od,
    Train,
    Station,
}

/// 一份完整配置。字段按标签页分组，方便以后每个页面继续加。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 格式版本；解析时统一改写为 [`CONFIG_VERSION`]，见 [`load`]。
    pub version: u32,
    pub ui: UiSettings,
    pub train: TrainSettings,
    pub station: StationSettings,
    pub od: OdSettings,
    pub detail: DetailSettings,
}

/// 与具体查询无关的界面状态。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// 上次停留的标签页。
    pub tab: Tab,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainSettings {
    pub query: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StationSettings {
    pub query: String,
    pub fuzzy: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OdSettings {
    pub from: String,
    pub to: String,
    pub fuzzy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetailSettings {
    /// 经由是否只看车站。默认开（与界面初始值一致）。
    pub via_stations_only: bool,
}

impl Default for DetailSettings {
    fn default() -> Self {
        Self {
            via_stations_only: true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            ui: UiSettings::default(),
            train: TrainSettings::default(),
            station: StationSettings::default(),
            od: OdSettings::default(),
            detail: DetailSettings::default(),
        }
    }
}

/// 读取配置。
///
/// 返回 `(配置, 可选提示)`：文件不存在是最常见的情况，静默用默认值；
/// 读不了或解析不了时附带一句给界面显示的话（调用方负责展示，不阻断启动）。
pub fn load(path: &Path) -> (Settings, Option<String>) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return (Settings::default(), None),
        Err(err) => {
            return (
                Settings::default(),
                Some(format!("读取配置 {} 失败：{err}", path.display())),
            );
        }
    };

    match serde_json::from_str::<Settings>(&text) {
        Ok(mut settings) => {
            settings.version = CONFIG_VERSION;
            settings.clamp_queries();
            (settings, None)
        }
        Err(err) => {
            // 坏文件改名留证，避免下一次写盘直接盖掉现场。
            let abandoned = path.with_extension("json.bad");
            let _ = fs::rename(path, &abandoned);
            (
                Settings::default(),
                Some(format!(
                    "配置文件 {} 解析失败，已使用默认设置（原文件改名为 {}）：{err}",
                    path.display(),
                    abandoned.display()
                )),
            )
        }
    }
}

/// 写入配置：先写 `llt-gui.json.tmp` 再改名覆盖。
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec_pretty(settings)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let tmp = path.with_extension("json.tmp");
    if let Err(err) = fs::write(&tmp, body) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// 解析配置路径：`--config` → `$LLT_CONFIG` → 可执行文件旁 → 当前目录。
///
/// 放在 exe 旁是「绿色版」语义：整个目录复制走，配置和数据库一起跟过去。
/// 只读目录（Program Files、只读共享盘）用 `--config` 或 `$LLT_CONFIG` 指到别处，
/// 写不进去时界面只提示一次，不影响使用。
pub fn resolve(
    explicit: Option<&Path>,
    env: Option<&Path>,
    exe_dir: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(path) = env {
        return path.to_path_buf();
    }
    exe_dir.unwrap_or(cwd).join(CONFIG_FILE)
}

/// 实际使用的配置路径（读取环境变量与可执行文件位置）。
pub fn resolve_path(explicit: Option<&Path>) -> PathBuf {
    let env = env::var_os("LLT_CONFIG")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve(explicit, env.as_deref(), exe_dir.as_deref(), &cwd)
}

impl Settings {
    /// 查询词截断到 [`MAX_QUERY_CHARS`]，其余原样保留（不 trim，避免吃掉用户输入）。
    fn clamp_queries(&mut self) {
        for field in [
            &mut self.train.query,
            &mut self.station.query,
            &mut self.od.from,
            &mut self.od.to,
        ] {
            if field.chars().count() > MAX_QUERY_CHARS {
                *field = field.chars().take(MAX_QUERY_CHARS).collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独占一个临时目录（与 `lltdb::path` 的测试写法一致，不引额外依赖）。
    fn tempdir() -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "llt-gui-config-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Settings {
        Settings {
            version: CONFIG_VERSION,
            ui: UiSettings { tab: Tab::Station },
            train: TrainSettings {
                query: "G1".to_owned(),
            },
            station: StationSettings {
                query: "北京".to_owned(),
                fuzzy: true,
            },
            od: OdSettings {
                from: "北京".to_owned(),
                to: "上海".to_owned(),
                fuzzy: true,
            },
            detail: DetailSettings {
                via_stations_only: false,
            },
        }
    }

    #[test]
    fn roundtrip_keeps_every_field() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        let settings = sample();
        save(&path, &settings).unwrap();

        let (loaded, warning) = load(&path);
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(loaded, settings);
        // 临时文件不该留在原地。
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_file_is_silent_defaults() {
        let dir = tempdir();
        let (settings, warning) = load(&dir.join(CONFIG_FILE));
        assert_eq!(settings, Settings::default());
        assert!(warning.is_none());
        assert!(Settings::default().detail.via_stations_only);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn broken_json_falls_back_and_keeps_a_copy() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        fs::write(&path, b"{ train: ").unwrap();

        let (settings, warning) = load(&path);
        assert_eq!(settings, Settings::default());
        assert!(warning.expect("应有提示").contains("解析失败"));
        assert!(!path.exists());
        assert!(path.with_extension("json.bad").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn partial_file_uses_defaults_for_missing_parts() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"{ "train": { "query": "K54" }, "station": { "fuzzy": true } }"#,
        )
        .unwrap();

        let (settings, _) = load(&path);
        assert_eq!(settings.train.query, "K54");
        assert!(settings.station.fuzzy);
        assert!(settings.station.query.is_empty());
        // 缺 detail 段时「只看车站」保持默认的 true。
        assert!(settings.detail.via_stations_only);
        // 缺 ui 段时回到默认的第一页（站站查询）。
        assert_eq!(settings.ui.tab, Tab::Od);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unknown_fields_and_newer_version_are_tolerated() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"{ "version": 99, "future": { "x": 1 }, "od": { "from": "北京", "to": "上海" } }"#,
        )
        .unwrap();

        let (settings, warning) = load(&path);
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(settings.od.from, "北京");
        // 版本统一改写为当前版本，避免「读到的版本」和「写回的版本」来回打架。
        assert_eq!(settings.version, CONFIG_VERSION);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tab_is_persisted_as_snake_case() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        save(&path, &sample()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"tab\": \"station\""), "{text}");
        assert_eq!(load(&path).0.ui.tab, Tab::Station);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unknown_tab_name_falls_back_to_defaults() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"{ "ui": { "tab": "diagram" }, "train": { "query": "G1" } }"#,
        )
        .unwrap();

        let (settings, warning) = load(&path);
        assert_eq!(settings, Settings::default());
        assert!(warning.expect("应有提示").contains("解析失败"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_queries_are_truncated() {
        let dir = tempdir();
        let path = dir.join(CONFIG_FILE);
        fs::write(
            &path,
            format!(
                r#"{{ "train": {{ "query": "{}" }} }}"#,
                "G".repeat(MAX_QUERY_CHARS + 20)
            ),
        )
        .unwrap();

        let (settings, _) = load(&path);
        assert_eq!(settings.train.query.chars().count(), MAX_QUERY_CHARS);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn save_failure_leaves_no_temp_file() {
        let dir = tempdir();
        // 父路径是个普通文件，create_dir_all 必然失败。
        let blocker = dir.join("blocked");
        fs::write(&blocker, b"x").unwrap();
        let path = blocker.join(CONFIG_FILE);

        assert!(save(&path, &Settings::default()).is_err());
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn resolve_order_is_explicit_env_exe_cwd() {
        let cwd = Path::new("C:/work");
        let exe = Path::new("C:/tools");
        assert_eq!(
            resolve(
                Some(Path::new("D:/a.json")),
                Some(Path::new("D:/b.json")),
                Some(exe),
                cwd
            ),
            Path::new("D:/a.json")
        );
        assert_eq!(
            resolve(None, Some(Path::new("D:/b.json")), Some(exe), cwd),
            Path::new("D:/b.json")
        );
        assert_eq!(
            resolve(None, None, Some(exe), cwd),
            Path::new("C:/tools").join(CONFIG_FILE)
        );
        // 取不到 exe 路径时退回当前目录。
        assert_eq!(resolve(None, None, None, cwd), cwd.join(CONFIG_FILE));
    }
}
