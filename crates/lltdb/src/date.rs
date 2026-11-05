//! 日期换算。
//!
//! 数据里的日期都是 `YYYYMMDD` 整数（如 `20261108`），界面与 CLI 用
//! [`jiff::civil::Date`] 做日历运算；两者的“开行日期”语义必须完全一致，
//! 因此这些函数放在共用的 `lltdb` 里。

use jiff::civil::Date;

/// `Date` -> `YYYYMMDD`。
pub fn ymd(date: Date) -> i64 {
    i64::from(date.year()) * 10_000 + i64::from(date.month()) * 100 + i64::from(date.day())
}

/// 把查询日期按停站的「第几天」倒推该车次的始发日期（`YYYYMMDD`）。
///
/// 夜间车在次日到达本站时，`day = 2`，始发日期应是查询日的前一天。
pub fn origin_ymd(date: Date, day: i64) -> i64 {
    let offset = (day - 1).max(0);
    if offset == 0 {
        return ymd(date);
    }
    match date.checked_sub(jiff::Span::new().days(offset)) {
        Ok(origin) => ymd(origin),
        Err(_) => ymd(date),
    }
}

/// `YYYYMMDD` -> `2026-11-08`；无效时返回空串。
pub fn date_ymd(value: i64) -> String {
    if value < 10_000_000 {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02}",
        value / 10_000,
        (value / 100) % 100,
        value % 100
    )
}

/// 开行区间展示：`2026-01-01 ~ 2026-12-31`、单日、或 `2026-01-01 起`。
pub fn date_window(from: i64, to: i64) -> String {
    if to >= 2099_0101 {
        format!("{} 起", date_ymd(from))
    } else if from == to {
        date_ymd(from)
    } else {
        format!("{} ~ {}", date_ymd(from), date_ymd(to))
    }
}

/// `周一` … `周日`。
pub fn weekday_cn(date: Date) -> &'static str {
    use jiff::civil::Weekday;
    match date.weekday() {
        Weekday::Monday => "周一",
        Weekday::Tuesday => "周二",
        Weekday::Wednesday => "周三",
        Weekday::Thursday => "周四",
        Weekday::Friday => "周五",
        Weekday::Saturday => "周六",
        Weekday::Sunday => "周日",
    }
}

/// 今天（本地时区）。
pub fn today() -> Date {
    jiff::Zoned::now().date()
}

/// 解析用户输入的日期：`2026-11-08`（ISO）或 `20261108`（数据里的写法）。
pub fn parse_date(input: &str) -> Result<Date, String> {
    let text = input.trim();
    if text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit()) {
        let year: i16 = text[0..4]
            .parse()
            .map_err(|_| format!("无效日期：{input}"))?;
        let month: i8 = text[4..6]
            .parse()
            .map_err(|_| format!("无效日期：{input}"))?;
        let day: i8 = text[6..8]
            .parse()
            .map_err(|_| format!("无效日期：{input}"))?;
        return Date::new(year, month, day)
            .map_err(|_| format!("无效日期 {input}（例：2026-11-08 或 20261108）"));
    }
    text.parse::<Date>()
        .map_err(|_| format!("无效日期 {input}（例：2026-11-08 或 20261108）"))
}

/// 把分钟数格式化成 `1小时46分` / `2天3小时05分`。
pub fn duration_cn(minutes: i64) -> String {
    let minutes = minutes.max(0);
    let days = minutes / 1440;
    let hours = (minutes % 1440) / 60;
    let mins = minutes % 60;

    if days > 0 {
        format!("{days}天{hours}小时{mins:02}分")
    } else if hours > 0 {
        format!("{hours}小时{mins:02}分")
    } else {
        format!("{mins}分")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ymd_and_back() {
        let date = Date::new(2026, 10, 28).unwrap();
        assert_eq!(ymd(date), 20261028);
        assert_eq!(date_ymd(20261028), "2026-10-28");
        assert_eq!(date_ymd(0), "");
        assert_eq!(weekday_cn(date), "周三");
    }

    #[test]
    fn origin_date_handles_month_boundary() {
        let date = Date::new(2026, 3, 1).unwrap();
        assert_eq!(origin_ymd(date, 1), 20260301);
        assert_eq!(origin_ymd(date, 2), 20260228);
        assert_eq!(origin_ymd(date, 3), 20260227);
    }

    #[test]
    fn date_window_formats_ranges() {
        assert_eq!(date_window(20261105, 20261105), "2026-11-05");
        assert_eq!(date_window(20261105, 20261120), "2026-11-05 ~ 2026-11-20");
        assert_eq!(date_window(20261105, 20991231), "2026-11-05 起");
    }

    #[test]
    fn parse_date_accepts_both_forms() {
        let expected = Date::new(2026, 11, 8).unwrap();
        assert_eq!(parse_date("2026-11-08").unwrap(), expected);
        assert_eq!(parse_date("20261108").unwrap(), expected);
        assert_eq!(parse_date(" 20261108 ").unwrap(), expected);
        assert!(parse_date("2026-13-01").is_err());
        assert!(parse_date("2026/11/08").is_err());
        assert!(parse_date("").is_err());
    }

    #[test]
    fn duration_formats() {
        assert_eq!(duration_cn(106), "1小时46分");
        assert_eq!(duration_cn(45), "45分");
        assert_eq!(duration_cn(1440 + 125), "1天2小时05分");
        assert_eq!(duration_cn(-5), "0分");
    }
}
