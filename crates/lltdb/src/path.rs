//! 数据库位置解析（GUI 与 CLI 共用，只有这一份实现）。
//!
//! 查找顺序：
//!
//! 1. 显式路径（`--db`）：给了就必须存在，不存在直接报错，**不再回退**；
//! 2. 环境变量 `LLT_DB`：同上；
//! 3. 可执行文件目录（`llt build` 的默认输出位置）；
//! 4. 当前目录（`./llt-timetable.sqlite`、`./out/timetable/…`）；
//! 5. 仅在“开发环境”里：自当前目录向上逐级找 `out/timetable/…`。
//!
//! 第 5 条取代了以前散落在各处的 `../out/timetable`、`exe/../../out/timetable`
//! 这类“假设自己在第几层”的硬编码，任意目录深度都能命中。
//!
//! “开发环境”按**运行时**判断（`exe 路径里有 target 段`，或当前目录的某个祖先
//! 有带 `[workspace]` 的 `Cargo.toml`），而不是按 `debug/release` 编译模式——
//! 仓库里跑 `cargo build --release` 的产物同样需要找到 `out/timetable`。

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// 数据文件名。
pub const DB_FILE: &str = "llt-timetable.sqlite";

/// 向上找 `out/timetable` 的最大层数（避免跑到盘符根之外）。
const MAX_ANCESTORS: usize = 6;

/// 命中的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `--db`
    Explicit,
    /// `$LLT_DB`
    Env,
    /// 可执行文件目录
    ExeDir,
    /// 当前目录
    CurrentDir,
    /// 向上找到的仓库 `out/timetable`
    Workspace,
}

impl Source {
    /// 给日志/错误信息用的中文说明。
    pub fn describe(self) -> &'static str {
        match self {
            Self::Explicit => "--db",
            Self::Env => "环境变量 LLT_DB",
            Self::ExeDir => "可执行文件目录",
            Self::CurrentDir => "当前目录",
            Self::Workspace => "仓库 out/timetable",
        }
    }
}

/// 解析结果：实际路径 + 命中来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    pub source: Source,
}

/// 找不到（或显式路径不存在）时返回的错误，带上尝试过的所有路径。
#[derive(Debug, Clone)]
pub struct LocateError {
    pub message: String,
    pub searched: Vec<PathBuf>,
}

impl fmt::Display for LocateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)?;
        if !self.searched.is_empty() {
            write!(formatter, "（已查找：")?;
            for (index, path) in self.searched.iter().enumerate() {
                if index > 0 {
                    write!(formatter, "、")?;
                }
                write!(formatter, "{}", path.display())?;
            }
            write!(formatter, "）")?;
        }
        Ok(())
    }
}

impl std::error::Error for LocateError {}

/// 查找数据库。
///
/// - `explicit`：命令行给的路径，存在即用；
/// - `strict`：只认 `explicit` 与 `LLT_DB`，不做隐式搜索（给脚本用）。
pub fn find(explicit: Option<&Path>, strict: bool) -> Result<Located, LocateError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let exe = std::env::current_exe().ok();
    // 环境变量优先于隐式搜索，因此先单独处理。
    let env = std::env::var_os("LLT_DB")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());

    let candidates = collect(explicit, env.as_deref(), &cwd, exe.as_deref(), strict);
    let mut searched = Vec::with_capacity(candidates.len());
    for (path, source) in candidates {
        if path.is_file() {
            return Ok(Located { path, source });
        }
        searched.push(path);
    }

    let message = if strict {
        format!("找不到 {DB_FILE}（--strict：只认 --db 与 $LLT_DB）")
    } else {
        format!("找不到 {DB_FILE}：先运行 `llt build`，或用 --db 指定")
    };
    Err(LocateError { message, searched })
}

/// 与 [`find`] 相同，但找不到时返回 `None`。
pub fn find_optional(explicit: Option<&Path>, strict: bool) -> Option<Located> {
    find(explicit, strict).ok()
}

/// 构造候选列表（纯函数，便于测试）。
fn collect(
    explicit: Option<&Path>,
    env: Option<&Path>,
    cwd: &Path,
    exe: Option<&Path>,
    strict: bool,
) -> Vec<(PathBuf, Source)> {
    if let Some(path) = explicit {
        return vec![(path.to_path_buf(), Source::Explicit)];
    }
    if let Some(path) = env {
        return vec![(path.to_path_buf(), Source::Env)];
    }
    if strict {
        return Vec::new();
    }

    let mut out: Vec<(PathBuf, Source)> = Vec::new();
    let remember = |out: &mut Vec<(PathBuf, Source)>, path: PathBuf, source: Source| {
        // exe 目录与当前目录常常相同，去重后错误信息才不会重复。
        if !out.iter().any(|(known, _)| known == &path) {
            out.push((path, source));
        }
    };

    if let Some(dir) = exe.and_then(Path::parent) {
        remember(&mut out, dir.join(DB_FILE), Source::ExeDir);
    }
    remember(&mut out, cwd.join(DB_FILE), Source::CurrentDir);
    remember(
        &mut out,
        cwd.join("out").join("timetable").join(DB_FILE),
        Source::Workspace,
    );

    if is_dev_environment(cwd, exe) {
        for ancestor in cwd.ancestors().skip(1).take(MAX_ANCESTORS) {
            remember(
                &mut out,
                ancestor.join("out").join("timetable").join(DB_FILE),
                Source::Workspace,
            );
        }
    }
    out
}

/// 是否在开发环境（cargo 产物目录，或 cargo 工作区里）。
pub fn is_dev_environment(cwd: &Path, exe: Option<&Path>) -> bool {
    if let Some(exe) = exe
        && exe
            .components()
            .any(|component| matches!(component, Component::Normal(name) if name.eq_ignore_ascii_case("target")))
    {
        return true;
    }
    cwd.ancestors()
        .take(MAX_ANCESTORS + 1)
        .any(has_workspace_manifest)
}

/// 该目录下是否有声明了 `[workspace]` 的 `Cargo.toml`。
fn has_workspace_manifest(dir: &Path) -> bool {
    let manifest = dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return false;
    };
    text.lines()
        .any(|line| line.trim_start().starts_with("[workspace]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lltdb-path-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn explicit_missing_is_an_error_with_the_path() {
        let err = find(Some(Path::new("no-such-file.sqlite")), false).unwrap_err();
        assert!(err.message.contains("找不到"), "{err}");
        assert_eq!(err.searched.len(), 1);
        assert!(err.to_string().contains("no-such-file.sqlite"), "{err}");
    }

    #[test]
    fn explicit_existing_wins() {
        let dir = tempdir();
        let db = dir.join(DB_FILE);
        touch(&db);
        let located = find(Some(&db), true).unwrap();
        assert_eq!(located.source, Source::Explicit);
        assert_eq!(located.path, db);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn strict_without_explicit_finds_nothing() {
        let candidates = collect(None, None, Path::new("C:/work"), None, true);
        assert!(candidates.is_empty());
    }

    #[test]
    fn candidate_order_is_exe_then_cwd() {
        let cwd = Path::new("C:/work/app");
        let exe = Path::new("C:/tools/llt.exe");
        let candidates = collect(None, None, cwd, Some(exe), false);
        assert_eq!(
            candidates[0],
            (Path::new("C:/tools").join(DB_FILE), Source::ExeDir)
        );
        assert_eq!(candidates[1], (cwd.join(DB_FILE), Source::CurrentDir));
        assert_eq!(
            candidates[2],
            (
                cwd.join("out").join("timetable").join(DB_FILE),
                Source::Workspace
            )
        );
    }

    #[test]
    fn workspace_walk_finds_nested_repo_layout() {
        let root = tempdir();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"cli\"]\n",
        )
        .unwrap();
        touch(&root.join("out").join("timetable").join(DB_FILE));
        let nested = root.join("cli").join("src");
        std::fs::create_dir_all(&nested).unwrap();

        assert!(is_dev_environment(&nested, None));
        let candidates = collect(None, None, &nested, None, false);
        let hit = candidates
            .iter()
            .find(|(path, _)| path.is_file())
            .expect("应命中仓库 out/timetable");
        assert_eq!(hit.1, Source::Workspace);
        assert_eq!(hit.0, root.join("out").join("timetable").join(DB_FILE));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn release_binary_inside_cargo_tree_is_still_dev() {
        // 仓库里 `cargo build --release` 的产物：exe 位于 target/release。
        assert!(is_dev_environment(
            Path::new("C:/repo"),
            Some(Path::new("C:/repo/target/release/llt.exe"))
        ));
        // 真正发布形态：exe 在任意目录、无工作区。
        let dir = tempdir();
        assert!(!is_dev_environment(&dir, Some(&dir.join("llt.exe"))));
        std::fs::remove_dir_all(dir).ok();
    }
}
