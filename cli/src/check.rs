//! `llt check`：比对本地库与线上版本，判断是否需要更新。
//!
//! 判定逻辑在 `lltdata::check`（界面「检查更新」用的也是它）；这里只负责打印。

use std::time::Duration;

use anyhow::Result;
use lltdata::check::{Decision, decide};
use lltdata::local::LocalState;
use lltdata::update::{self, RemoteVersion};
use serde_json::json;

use crate::cli::CheckArgs;

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
