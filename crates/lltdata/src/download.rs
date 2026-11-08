//! 从官网下载数据包。

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::progress::Progress;

/// 下载结果中除文件本身之外还有用的信息（下载时间写进 sqlite 的 meta 表）。
#[derive(Debug, Default)]
pub struct DownloadInfo {
    pub bytes: u64,
    /// 服务器上的文件时间（HTTP `Last-Modified`），通常就是数据包的发布时间。
    pub last_modified: Option<String>,
    pub etag: Option<String>,
}

/// 依次尝试镜像，返回成功的地址与下载信息。
pub fn download_first(
    urls: &[String],
    dest: &Path,
    progress: &dyn Progress,
) -> Result<(String, DownloadInfo)> {
    if urls.is_empty() {
        bail!("没有可用的下载地址");
    }
    let mut errors = Vec::new();
    for url in urls {
        match download(url, dest, progress) {
            Ok(info) => return Ok((url.clone(), info)),
            Err(err) => {
                progress.note(&format!("镜像失败（{url}）：{err:#}"));
                errors.push(format!("{url}: {err:#}"));
            }
        }
    }
    bail!(
        "所有下载地址都失败：
  {}",
        errors.join(
            "
  "
        )
    )
}

/// 把数据包下载到 `dest`。临时文件的清理由调用方（临时目录）负责。
pub fn download(url: &str, dest: &Path, progress: &dyn Progress) -> Result<DownloadInfo> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .http_status_as_error(false)
        .user_agent(concat!("llt/", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    progress.note(&format!("下载 {url}"));
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("请求失败：{url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("下载失败：{url} 返回 HTTP {status}");
    }
    let last_modified = header(&response, "last-modified");
    let etag = header(&response, "etag");
    let expected = response.body().content_length();
    // 若响应被压缩过，Content-Length 是压缩后的大小，不能和写入字节数直接比较。
    let encoded = response.headers().contains_key("content-encoding");

    let file =
        File::create(dest).with_context(|| format!("无法写入临时文件 {}", dest.display()))?;
    let mut out = BufWriter::new(file);
    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut next_report: u64 = 0;

    loop {
        let read = reader
            .read(&mut buf)
            .with_context(|| format!("下载中断：{url}"))?;
        if read == 0 {
            break;
        }
        out.write_all(&buf[..read])
            .with_context(|| format!("无法写入临时文件 {}", dest.display()))?;
        written += read as u64;
        if written >= next_report {
            progress.download(written, expected);
            next_report = written + 512 * 1024;
        }
    }
    out.flush()
        .with_context(|| format!("无法写入临时文件 {}", dest.display()))?;

    if written == 0 {
        bail!("下载失败：{url} 返回空响应");
    }
    if let Some(expected) = expected.filter(|_| !encoded)
        && expected != written
    {
        bail!("下载不完整：{url} 声明 {expected} 字节，实际收到 {written} 字节");
    }
    progress.note(&format!("下载完成：{:.1} MB", written as f64 / 1e6));
    if let Some(value) = &last_modified {
        progress.note(&format!("服务器更新时间：{value}"));
    }

    Ok(DownloadInfo {
        bytes: written,
        last_modified,
        etag,
    })
}

fn header<B>(response: &ureq::http::Response<B>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_owned)
}
