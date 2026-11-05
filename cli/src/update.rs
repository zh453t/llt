//! 线上版本探测：`android.ver`（官方「检查更新」接口）。
//!
//! App 用 `UpgradeModel` 解析这个接口，响应**不是严格 JSON**（偶有裸值，
//! 例如 `"name": 应用宝"`，App 靠 Gson 的 lenient 模式容忍），因此这里
//! 先尝试严格解析，失败后按行修补再解析。见 `tools/check_update.py`。
//!
//! ```json
//! { "version": { "app": "5.3.7", "desc": "…", "data": "20261108", "minSdk": 19 },
//!   "dataUrl": ["http://…/an.db", "http://down.lltskb.com/an.db"],
//!   "appUrl":  ["…apk"],
//!   "market":  [ { "name": "小米", "version": { … } } ] }
//! ```
//!
//! `version.data` 与数据包里的 `ver.txt` 同义（都是 `YYYYMMDD` 的数据版本），
//! 所以“是否需要更新”只需一次 2 KB 的请求。

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// 从 `android.ver` 解析出的信息。
#[derive(Debug, Clone, Default)]
pub struct RemoteVersion {
    /// `version.data`，数据版本（`YYYYMMDD`）。
    pub data_version: Option<String>,
    /// `version.app`，APK 版本号。
    pub app_version: Option<String>,
    /// `version.minSdk`（App 会据此丢弃响应；工具只记录，不拦截）。
    pub min_sdk: Option<i64>,
    /// `version.desc`，更新说明。
    pub desc: Option<String>,
    /// `dataUrl`，数据包地址（按优先级）。
    pub data_urls: Vec<String>,
    /// `appUrl`，APK 地址。
    pub app_urls: Vec<String>,
    /// 渠道版本：`(渠道名, app 版本, data 版本)`。
    pub markets: Vec<(String, Option<String>, Option<String>)>,
    /// 响应里实际用到的 URL。
    pub source_url: String,
}

impl RemoteVersion {
    /// 解析 `android.ver` 的正文。
    pub fn parse(text: &str, source_url: &str) -> Result<Self> {
        let text = text.trim_start_matches('\u{feff}');
        let value = parse_lenient(text).context("android.ver 不是可解析的 JSON")?;

        let version = value
            .get("version")
            .context("android.ver 缺少 version 字段")?;

        let markets = value
            .get("market")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let inner = item.get("version")?;
                        Some((
                            scalar(item.get("name"))?,
                            scalar(inner.get("app")),
                            scalar(inner.get("data")),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            data_version: scalar(version.get("data")),
            app_version: scalar(version.get("app")),
            min_sdk: version.get("minSdk").and_then(Value::as_i64),
            desc: scalar(version.get("desc")),
            data_urls: urls(value.get("dataUrl")),
            app_urls: urls(value.get("appUrl")),
            markets,
            source_url: source_url.to_owned(),
        })
    }

    /// 下载地址列表：显式给了 `--url` 就只用它，否则用接口给的镜像。
    pub fn mirrors(&self, explicit: Option<&str>) -> Vec<String> {
        match explicit {
            Some(url) => vec![url.to_owned()],
            None if !self.data_urls.is_empty() => self.data_urls.clone(),
            None => vec![crate::cli::DEFAULT_DATA_URL.to_owned()],
        }
    }

    /// `data` 版本是否比 `local` 新（8 位数字串，按数值比较）。
    pub fn data_is_newer_than(&self, local: Option<&str>) -> Option<bool> {
        let remote = self.data_version.as_deref()?;
        let local = local?;
        let remote = version_number(remote)?;
        let local = version_number(local)?;
        Some(remote > local)
    }
}

/// 拉取并解析 `android.ver`。
pub fn fetch(url: &str, timeout: Duration) -> Result<RemoteVersion> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .user_agent(concat!("llt/", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("无法访问版本接口 {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("版本接口 {url} 返回 HTTP {status}");
    }
    let body = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("读取版本接口失败：{url}"))?;
    RemoteVersion::parse(&body, url)
}

/// `"20261108"` -> `Some(20261108)`；非 8 位数字返回 `None`。
pub fn version_number(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn urls(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|item| scalar(Some(item))).collect())
        .unwrap_or_default()
}

/// 取出标量（字符串或数字）转成字符串。
fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// 严格解析 → 失败则按行修补再解析。
fn parse_lenient(text: &str) -> Result<Value, serde_json::Error> {
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(strict_error) => {
            let repaired = repair_bare_values(text);
            serde_json::from_str(&repaired).map_err(|_| strict_error)
        }
    }
}

/// 修补 `"key": 裸值` 这种不符合 JSON 的写法（官方接口偶发）。
///
/// 官方接口曾出现 `"name": 应用宝"`（少了开引号）与 `"name": 应用宝`（完全没有引号）。
/// 这里按『键: 值』扫描：值不是字符串/数字/布尔/null 时补上成对引号，
/// 其余内容原样保留，因此不会破坏已经合法的 JSON。
fn repair_bare_values(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 32);
    let mut index = 0;
    let mut expect_value = false;

    while index < chars.len() {
        let current = chars[index];

        if current == '"' {
            // 整段复制字符串（跳过转义），再看它后面是不是冒号，即它是不是 key。
            let start = index;
            index += 1;
            while index < chars.len() {
                match chars[index] {
                    '\\' => index += 2,
                    '"' => {
                        index += 1;
                        break;
                    }
                    _ => index += 1,
                }
            }
            out.extend(&chars[start..index.min(chars.len())]);

            let mut look = index;
            while look < chars.len() && chars[look].is_whitespace() {
                look += 1;
            }
            if look < chars.len() && chars[look] == ':' {
                // 连同空白和冒号一起复制，后面紧跟的就是值。
                out.extend(&chars[index..=look]);
                index = look + 1;
                expect_value = true;
            }
            continue;
        }

        if expect_value && !current.is_whitespace() {
            expect_value = false;
            if matches!(current, '{' | '[' | ']' | '}' | ',') {
                out.push(current);
                index += 1;
                continue;
            }

            // 裸值：读到分隔符或行尾为止。
            let start = index;
            while index < chars.len() && !matches!(chars[index], ',' | '}' | ']' | '\n' | '\r') {
                index += 1;
            }
            let raw: String = chars[start..index].iter().collect();
            let token = raw.trim();
            // 官方接口有时多一个结尾引号（`应用宝"`），补成对引号前先去掉。
            let token = token.strip_suffix('"').unwrap_or(token);

            if is_json_literal(token) {
                out.push_str(&raw);
            } else {
                let lead: String = raw.chars().take_while(|ch| ch.is_whitespace()).collect();
                let trail: String = {
                    let reversed: String = raw
                        .chars()
                        .rev()
                        .take_while(|ch| ch.is_whitespace())
                        .collect();
                    reversed.chars().rev().collect()
                };
                out.push_str(&lead);
                out.push('"');
                out.push_str(token);
                out.push('"');
                out.push_str(&trail);
            }
            continue;
        }

        out.push(current);
        index += 1;
    }
    out
}

fn is_json_literal(value: &str) -> bool {
    value.starts_with('"')
        || value.starts_with('{')
        || value.starts_with('[')
        || matches!(value, "true" | "false" | "null")
        || value.parse::<f64>().is_ok()
}

/// 给 `llt check --json` 用的结构化输出。
pub fn to_json(remote: &RemoteVersion) -> Value {
    json!({
        "url": remote.source_url,
        "data_version": remote.data_version,
        "app_version": remote.app_version,
        "min_sdk": remote.min_sdk,
        "desc": remote.desc,
        "data_urls": remote.data_urls,
        "app_urls": remote.app_urls,
        "markets": remote
            .markets
            .iter()
            .map(|(name, app, data)| json!({"name": name, "app": app, "data": data}))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "version": {
            "app": "5.3.7",
            "desc": "增加车型图片在线查看功能",
            "data": "20261108",
            "minSdk": 19
      },
      "appUrl": ["http://x/lulutong.apk"],
      "dataUrl": [
        "http://223.107.87.50:8011/an.db",
        "http://down.lltskb.com/an.db"
      ],
      "market": [
        { "name": "小米", "id": "com.xiaomi.market",
          "version": { "app": "5.2.8", "desc": "…", "data": "20260328", "minSdk": 16 } },
        { "name": 应用宝", "id": "com.tencent.android.qqdownloader",
          "version": { "app": "5.3.1", "desc": "…", "data": "20260918", "minSdk": 19 } }
      ]
    }"#;

    #[test]
    fn parses_messy_response() {
        // `"name": 应用宝"` 少了开引号：严格 JSON 解析会失败，修补后应成功。
        assert!(serde_json::from_str::<Value>(SAMPLE).is_err());
        let remote = RemoteVersion::parse(SAMPLE, "http://test/android.ver").unwrap();
        assert_eq!(remote.data_version.as_deref(), Some("20261108"));
        assert_eq!(remote.app_version.as_deref(), Some("5.3.7"));
        assert_eq!(remote.min_sdk, Some(19));
        assert_eq!(remote.data_urls.len(), 2);
        assert_eq!(remote.mirrors(None)[0], "http://223.107.87.50:8011/an.db");
        assert_eq!(remote.markets.len(), 2);
        assert_eq!(remote.markets[1].0, "应用宝");
        assert_eq!(remote.markets[1].2.as_deref(), Some("20260918"));
    }

    #[test]
    fn explicit_url_overrides_mirrors() {
        let remote = RemoteVersion::parse(SAMPLE, "u").unwrap();
        assert_eq!(
            remote.mirrors(Some("http://mirror/an.db")),
            ["http://mirror/an.db"]
        );
    }

    #[test]
    fn compares_data_versions_numerically() {
        let mut remote = RemoteVersion {
            data_version: Some("20261108".to_owned()),
            ..Default::default()
        };
        assert_eq!(remote.data_is_newer_than(Some("20261105")), Some(true));
        assert_eq!(remote.data_is_newer_than(Some("20261108")), Some(false));
        assert_eq!(remote.data_is_newer_than(Some("20261201")), Some(false));
        assert_eq!(remote.data_is_newer_than(None), None);
        remote.data_version = Some("2026110".to_owned());
        assert_eq!(remote.data_is_newer_than(Some("20261105")), None);
    }

    #[test]
    fn rejects_response_without_version_block() {
        let err = RemoteVersion::parse(r#"{"dataUrl": []}"#, "u").unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");
        assert!(RemoteVersion::parse("not json", "u").is_err());
    }

    #[test]
    fn repairs_only_bare_values() {
        // 合法 JSON 原样保留
        let legal = r#"{"a": "小米", "b": 19, "c": true, "d": [1], "e": null}"#;
        assert_eq!(repair_bare_values(legal), legal);
        // 裸值补引号（含多对键值写在同一行的情况）
        let broken = r#"{"version": {"data": 20261108}, "market": [{"name": 应用宝, "id": "x"}]}"#;
        let repaired = repair_bare_values(broken);
        let value: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(value["version"]["data"], 20261108);
        assert_eq!(value["market"][0]["name"], "应用宝");
        assert_eq!(value["market"][0]["id"], "x");

        // 多一个结尾引号的情况
        let extra_quote = r#"{"name": 应用宝"}"#;
        let value: serde_json::Value =
            serde_json::from_str(&repair_bare_values(extra_quote)).unwrap();
        assert_eq!(value["name"], "应用宝");
    }
}
