//! 数据包解包：`an.db` 其实是 zip 容器，里面是 64 个自定义二进制分片。
//!
//! 解包后统一通过 [`ShardDir`] 访问；分片名在包里是大写（`T0.dat` / `S0.dat`），
//! App 与本工具按小写引用，因此这里做大小写无关的解析。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// 解码必需的文件（`S*` 车站分片只影响 stations.csv 的车次数列）。
const REQUIRED: [&str; 22] = [
    "s.i", "t.i", "t0.dat", "t1.dat", "t2.dat", "t3.dat", "t4.dat", "t5.dat", "t6.dat", "t7.dat",
    "t8.dat", "t9.dat", "t10.dat", "t11.dat", "t12.dat", "t13.dat", "t14.dat", "t15.dat",
    "t16.dat", "t17.dat", "t18.dat", "t19.dat",
];

/// 可选分片：`sN.dat` 影响车站车次数；其余是经由 / 交路 / 餐车等附加数据。
const OPTIONAL: [&str; 16] = [
    "s0.dat",
    "s1.dat",
    "s2.dat",
    "s3.dat",
    "s4.dat",
    "s5.dat",
    "s6.dat",
    "s7.dat",
    "s8.dat",
    "s9.dat",
    "ver.txt",
    "sp.dat",
    "routes.dat",
    "jlb.dat",
    "extra.dat",
    "sn.dat",
];

/// 数据分片所在目录，文件名大小写不敏感。
#[derive(Debug)]
pub struct ShardDir {
    root: PathBuf,
    names: HashMap<String, String>,
}

impl ShardDir {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let entries =
            fs::read_dir(&root).with_context(|| format!("无法读取数据目录 {}", root.display()))?;

        let mut names = HashMap::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("无法读取数据目录 {}", root.display()))?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            names.insert(name.to_ascii_lowercase(), name);
        }
        Ok(Self { root, names })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.contains_key(&name.to_ascii_lowercase())
    }

    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.names
            .get(&name.to_ascii_lowercase())
            .map(|real| self.root.join(real))
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let path = self.path(name).ok_or_else(|| {
            anyhow::anyhow!(
                "缺少数据文件 {name}（目录 {}）；可用 -i 指定 an.db 或解包后的目录",
                self.root.display()
            )
        })?;
        fs::read(&path).with_context(|| format!("无法读取 {}", path.display()))
    }

    /// 读取小的文本文件（`ver.txt`），缺失或非 UTF-8 时返回 `None`。
    pub fn read_text(&self, name: &str) -> Option<String> {
        let bytes = self.read(name).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }

    pub fn missing(&self, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter(|name| !self.has(name))
            .map(|name| (*name).to_owned())
            .collect()
    }
}

/// 把输入（zip 数据包或已解包目录）准备成可读取的分片目录。
///
/// zip 时解包到 `work/an`，返回的目录随调用方的临时目录一起清理。
pub fn unpack(input: &Path, work: &Path, quiet: bool) -> Result<ShardDir> {
    let meta = fs::metadata(input).with_context(|| format!("无法访问输入 {}", input.display()))?;

    if meta.is_dir() {
        let shards = ShardDir::new(input)?;
        ensure_complete(&shards)?;
        return Ok(shards);
    }

    if !quiet {
        eprintln!("解包 {} …", input.display());
    }
    let dir = work.join("an");
    fs::create_dir_all(&dir).with_context(|| format!("无法创建临时解包目录 {}", dir.display()))?;

    extract_zip(input, &dir)?;

    let shards = ShardDir::new(&dir)?;
    ensure_complete(&shards)?;
    if !quiet {
        eprintln!("解包完成：{}", dir.display());
    }
    Ok(shards)
}

fn ensure_complete(shards: &ShardDir) -> Result<()> {
    let missing = shards.missing(&REQUIRED);
    if !missing.is_empty() {
        bail!(
            "数据不完整：{} 缺少 {} 个必需文件：{}",
            shards.root().display(),
            missing.len(),
            summarize(&missing)
        );
    }
    Ok(())
}

/// 校验 zip 魔数并解出需要的分片。
fn extract_zip(input: &Path, dir: &Path) -> Result<()> {
    let mut file = File::open(input).with_context(|| format!("无法打开 {}", input.display()))?;
    let mut magic = [0u8; 4];
    let read = file
        .read(&mut magic)
        .with_context(|| format!("无法读取 {}", input.display()))?;
    if read < 4 || magic[0] != b'P' || magic[1] != b'K' {
        bail!(
            "{} 不是 zip 数据包（an.db 应是 zip 容器，请确认下载完整、未被拦截）",
            input.display()
        );
    }
    file.seek(SeekFrom::Start(0))?;

    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("无法读取 zip 目录：{}", input.display()))?;

    // 先收集文件名：包内分片是大写的 T0.dat / S0.dat。
    let mut names: HashMap<String, String> = HashMap::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .with_context(|| format!("zip 目录损坏：{}", input.display()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .with_context(|| format!("zip 条目名不是合法文本：{}", input.display()))?
            .into_owned();
        if entry.encrypted() {
            bail!("zip 中的 {name} 已加密，不支持需要密码的数据包");
        }
        names.insert(name.to_ascii_lowercase(), name);
    }

    for wanted in REQUIRED.iter().chain(OPTIONAL.iter()) {
        let Some(real) = names.get(*wanted) else {
            continue;
        };
        let mut entry = archive
            .by_name(real)
            .with_context(|| format!("无法从 zip 读取 {real}"))?;
        let dest = dir.join(real);
        let mut out = BufWriter::new(
            File::create(&dest).with_context(|| format!("无法创建 {}", dest.display()))?,
        );
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("解包 {real} 失败（数据包可能已损坏）"))?;
        out.flush()
            .with_context(|| format!("无法写入 {}", dest.display()))?;
    }
    Ok(())
}

/// `a, b, c, d, e …`，用于把一长串文件名塞进错误信息。
pub fn summarize(names: &[String]) -> String {
    const MAX: usize = 5;
    let head = names
        .iter()
        .take(MAX)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX {
        format!("{head} …")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Option<PathBuf> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../out/data/an.db");
        path.exists().then_some(path)
    }

    #[test]
    fn unpacks_repo_fixture() {
        let Some(archive) = fixture() else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let shards = unpack(&archive, tmp.path(), true).unwrap();
        assert!(shards.has("s.i") && shards.has("t.i"));
        assert!(shards.has("t19.dat") && shards.has("s9.dat"));
        assert!(shards.read("s.i").unwrap().len() > 1000);
        assert_eq!(
            shards.read_text("ver.txt").as_deref().map(str::len),
            Some(8)
        );
    }

    #[test]
    fn rejects_non_zip_input() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("an.db");
        fs::write(&bogus, b"not a zip").unwrap();
        let err = unpack(&bogus, tmp.path(), true).unwrap_err().to_string();
        assert!(err.contains("不是 zip"), "{err}");
    }

    #[test]
    fn summarize_truncates_long_lists() {
        let names: Vec<String> = (0..7).map(|i| format!("f{i}")).collect();
        assert_eq!(summarize(&names), "f0, f1, f2, f3, f4 …");
        assert_eq!(summarize(&names[..2]), "f0, f1");
    }
}
