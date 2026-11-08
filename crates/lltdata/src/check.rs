//! 版本比对：本地库与线上数据哪个新、要不要重建。
//!
//! `llt check`、`llt build`、界面的「检查更新」都用这里的同一套判断，
//! 免得命令行与界面给出互相矛盾的结论。

use crate::export::SCHEMA_VERSION;
use crate::local::LocalState;
use crate::update::RemoteVersion;

/// 版本比对结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 本地库已是最新，可以跳过构建。
    UpToDate,
    /// 需要（重新）构建，附原因（给用户看的）。
    Rebuild(String),
    /// 本地比线上新，默认不覆盖。
    LocalNewer,
}

/// 判定逻辑。
///
/// `force` 直接把结论定成重建；本地库缺失 / 结构过旧 / 不完整一律重建；
/// 线上版本拿不到时**不当作最新**（宁可多下一次，也不要卡在坏库上）。
pub fn decide(local: Option<&LocalState>, remote: Option<&RemoteVersion>, force: bool) -> Decision {
    if force {
        return Decision::Rebuild("强制重建".to_owned());
    }
    let Some(local) = local else {
        return Decision::Rebuild("本地还没有数据库".to_owned());
    };
    if !local.usable() {
        return Decision::Rebuild(format!("本地库不可用：{}", local.problems.join("；")));
    }
    if !local.schema_current(SCHEMA_VERSION) {
        return Decision::Rebuild(format!(
            "库结构版本 {}，当前工具是 {SCHEMA_VERSION}，需要重建",
            local
                .schema_version
                .map(|value| value.to_string())
                .unwrap_or_else(|| "未知".to_owned())
        ));
    }

    let Some(remote) = remote else {
        return Decision::Rebuild("无法获取线上版本".to_owned());
    };
    let Some(remote_version) = remote.data_version.as_deref() else {
        return Decision::Rebuild("线上版本接口没有 data 字段".to_owned());
    };
    if local.data_version.is_none() {
        return Decision::Rebuild("本地库没有版本信息".to_owned());
    }

    match remote.data_is_newer_than(local.data_version.as_deref()) {
        Some(true) => Decision::Rebuild(format!(
            "发现新数据 {} → {remote_version}",
            local.data_version.as_deref().unwrap_or("?")
        )),
        Some(false) if local.data_version.as_deref() == Some(remote_version) => Decision::UpToDate,
        Some(false) => Decision::LocalNewer,
        // 版本号格式异常：当作“不确定”，宁可重建。
        None => Decision::Rebuild("版本号无法比较".to_owned()),
    }
}

/// 版本号便于在提示文案里展示。
pub fn describe_version(version: Option<&str>) -> String {
    match version {
        Some(text) => text.to_owned(),
        None => "未知".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn state(version: &str, schema: i64) -> LocalState {
        LocalState {
            path: PathBuf::from("db.sqlite"),
            data_version: Some(version.to_owned()),
            schema_version: Some(schema),
            generator: Some("llt 0.1.0".to_owned()),
            built_at: Some("2026-11-05T06:20:00Z".to_owned()),
            remote_app_version: Some("5.3.7".to_owned()),
            problems: Vec::new(),
        }
    }

    fn remote(version: &str) -> RemoteVersion {
        RemoteVersion {
            data_version: Some(version.to_owned()),
            app_version: Some("5.3.7".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn same_version_is_up_to_date() {
        let local = state("20261108", SCHEMA_VERSION);
        assert_eq!(
            decide(Some(&local), Some(&remote("20261108")), false),
            Decision::UpToDate
        );
    }

    #[test]
    fn newer_remote_triggers_rebuild() {
        let local = state("20261105", SCHEMA_VERSION);
        assert!(matches!(
            decide(Some(&local), Some(&remote("20261108")), false),
            Decision::Rebuild(_)
        ));
    }

    #[test]
    fn newer_local_is_not_overwritten() {
        let local = state("20261201", SCHEMA_VERSION);
        assert_eq!(
            decide(Some(&local), Some(&remote("20261108")), false),
            Decision::LocalNewer
        );
    }

    #[test]
    fn force_always_rebuilds() {
        let local = state("20261108", SCHEMA_VERSION);
        assert!(matches!(
            decide(Some(&local), Some(&remote("20261108")), true),
            Decision::Rebuild(_)
        ));
    }

    #[test]
    fn stale_schema_rebuilds_even_without_update() {
        let local = state("20261108", SCHEMA_VERSION - 1);
        assert!(matches!(
            decide(Some(&local), Some(&remote("20261108")), false),
            Decision::Rebuild(_)
        ));
    }

    #[test]
    fn broken_or_missing_local_rebuilds() {
        assert!(matches!(
            decide(None, Some(&remote("20261108")), false),
            Decision::Rebuild(_)
        ));

        let mut broken = state("20261108", SCHEMA_VERSION);
        broken.problems.push("缺少数据表：stops".to_owned());
        assert!(matches!(
            decide(Some(&broken), Some(&remote("20261108")), false),
            Decision::Rebuild(_)
        ));
    }

    #[test]
    fn unknown_remote_never_skips() {
        let local = state("20261108", SCHEMA_VERSION);
        assert!(matches!(
            decide(Some(&local), None, false),
            Decision::Rebuild(_)
        ));
        let blank = RemoteVersion {
            data_version: None,
            ..Default::default()
        };
        assert!(matches!(
            decide(Some(&local), Some(&blank), false),
            Decision::Rebuild(_)
        ));
    }
}
