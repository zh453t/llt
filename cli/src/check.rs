//! `llt check`：比对本地库与线上版本，判断是否需要更新。

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::cli::CheckArgs;
use crate::dbpath::LocalState;
use crate::export::SCHEMA_VERSION;
use crate::update::{self, RemoteVersion};

/// 已是最新。
pub const EXIT_OK: i32 = 0;
/// 运行错误（由 `main` 统一处理）。
pub const EXIT_ERROR: i32 = 1;
/// 线上有更新的数据。
pub const EXIT_UPDATE: i32 = 3;
/// 本地库缺失、结构过旧或不完整。
pub const EXIT_LOCAL_BROKEN: i32 = 4;
/// 无法访问版本接口（不联网 / 服务器故障）。
pub const EXIT_NETWORK: i32 = 5;

/// 版本比对结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 本地库已是最新，可以跳过构建。
    UpToDate,
    /// 需要（重新）构建。
    Rebuild(String),
    /// 本地比线上新，默认不覆盖。
    LocalNewer,
}

/// 判定逻辑（`check` 与 `build` 共用）。
pub fn decide(local: Option<&LocalState>, remote: Option<&RemoteVersion>, force: bool) -> Decision {
    if force {
        return Decision::Rebuild("--force".to_owned());
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

/// `llt check` 的实现。
pub fn run(args: &CheckArgs) -> Result<i32> {
    let local = LocalState::inspect(args.db.as_deref(), args.strict);
    let remote = update::fetch(&args.ver_url, Duration::from_secs(20));

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report(&local, &remote))?
        );
    } else {
        print_human(&local, &remote);
    }

    // 退出码优先反映“需要做什么”。
    Ok(match &remote {
        Err(_) => EXIT_NETWORK,
        Ok(remote) => match decide(local.as_ref(), Some(remote), false) {
            Decision::UpToDate => EXIT_OK,
            Decision::Rebuild(_) if local.as_ref().is_some_and(LocalState::usable) => EXIT_UPDATE,
            Decision::Rebuild(_) => EXIT_LOCAL_BROKEN,
            Decision::LocalNewer => EXIT_OK,
        },
    })
}

fn report(local: &Option<LocalState>, remote: &Result<RemoteVersion>) -> serde_json::Value {
    let decision = match remote {
        Ok(remote) => format!("{:?}", decide(local.as_ref(), Some(remote), false)),
        Err(_) => format!("{:?}", decide(local.as_ref(), None, false)),
    };
    json!({
        "local": local.as_ref().map(|state| json!({
            "path": state.path.display().to_string(),
            "data_version": state.data_version,
            "schema_version": state.schema_version,
            "generator": state.generator,
            "built_at": state.built_at,
            "problems": state.problems,
        })),
        "remote": match remote {
            Ok(remote) => update::to_json(remote),
            Err(err) => json!({"error": err.to_string()}),
        },
        "decision": decision,
    })
}

fn print_human(local: &Option<LocalState>, remote: &Result<RemoteVersion>) {
    match local {
        Some(state) => {
            println!(
                "本地库    : {} · 数据 {} · schema {} · 构建于 {}",
                state.path.display(),
                state.data_version.as_deref().unwrap_or("未知"),
                state
                    .schema_version
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "未知".to_owned()),
                state.built_at.as_deref().unwrap_or("未知"),
            );
            for problem in &state.problems {
                println!("            ! {problem}");
            }
        }
        None => println!("本地库    : 未找到（先运行 `llt build`）"),
    }

    match remote {
        Ok(remote) => {
            println!(
                "线上版本  : data={}  app={}  minSdk={}{}",
                remote.data_version.as_deref().unwrap_or("未知"),
                remote.app_version.as_deref().unwrap_or("未知"),
                remote
                    .min_sdk
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "未知".to_owned()),
                remote
                    .desc
                    .as_deref()
                    .map(|desc| format!("（{desc}）"))
                    .unwrap_or_default(),
            );
            let mirrors = remote.mirrors(None);
            if !mirrors.is_empty() {
                println!("下载镜像  : {}", mirrors.join(", "));
            }
            if let (Some(local_app), Some(remote_app)) = (
                local
                    .as_ref()
                    .and_then(|state| state.remote_app_version.as_ref()),
                remote.app_version.as_ref(),
            ) && local_app != remote_app
            {
                println!(
                    "注意      : 线上 APK 版本从 {local_app} 变为 {remote_app}，解码器可能需要适配"
                );
            }
        }
        Err(err) => println!("线上版本  : 无法获取（{err}）"),
    }

    println!();
    println!("结论      : {}", conclusion(local, remote));
}

fn conclusion(local: &Option<LocalState>, remote: &Result<RemoteVersion>) -> String {
    if remote.is_err() {
        return "无法判断（版本接口不可达）；`llt build` 会照常下载重建。".to_owned();
    }
    match decide(local.as_ref(), remote.as_ref().ok(), false) {
        Decision::UpToDate => "已是最新，无需更新。".to_owned(),
        Decision::Rebuild(reason) => format!("需要构建：{reason}。"),
        Decision::LocalNewer => format!(
            "本地数据（{}）比线上（{}）新，默认不覆盖；需要覆盖请加 --force。",
            local
                .as_ref()
                .and_then(|state| state.data_version.as_deref())
                .unwrap_or("?"),
            remote
                .as_ref()
                .ok()
                .and_then(|remote| remote.data_version.as_deref())
                .unwrap_or("?"),
        ),
    }
}

/// 版本号便于在错误信息里展示。
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
    use crate::update::RemoteVersion;

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
