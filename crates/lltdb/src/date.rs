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

/// 某一天对这趟车意味着什么（详情页日历的配色就靠它）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// 开行。
    Running,
    /// 开行区间还没开始。
    NotYet,
    /// 开行区间已经结束。
    Expired,
    /// 区间内，但按开行周期这天停开（例如隔日开行的另一天）。
    Stopped,
}

impl RunState {
    /// 中文短标签，界面与 CLI 共用。
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "开行",
            Self::NotYet => "未开行",
            Self::Expired => "已过期",
            Self::Stopped => "规则停开",
        }
    }
}

/// 一天开不开行。
///
/// `period` 天一轮、`mask` 的第 `k` 位为 1 表示第 `k` 天开行；`period <= 1` 或
/// `mask` 为 0（老数据没有 `t.rule`）时按「区间内天天开行」算。判定顺序与
/// `docs/llt-format-reverse-engineering.md` §12 一致：先看区间，再看周期。
pub fn run_state(date_from: i64, date_to: i64, date: i64, period: i64, mask: i64) -> RunState {
    if date < date_from {
        return RunState::NotYet;
    }
    if date > date_to {
        return RunState::Expired;
    }
    let period = period.max(1);
    if period > 1 && mask != 0 {
        let offset = days_between(date_from, date) % period;
        if mask & (1 << offset) == 0 {
            return RunState::Stopped;
        }
    }
    RunState::Running
}

/// 两个 `YYYYMMDD` 之间的天数差（`to - from`）。
pub fn days_between(from: i64, to: i64) -> i64 {
    match (to_date(from), to_date(to)) {
        // 两端都是午夜，直接按天数差算，不牵扯时区。
        (Some(from), Some(to)) => to.duration_since(from).as_secs() / 86_400,
        _ => 0,
    }
}

fn to_date(value: i64) -> Option<Date> {
    if value < 10_000_000 {
        return None;
    }
    Date::new(
        (value / 10_000) as i16,
        ((value / 100) % 100) as i8,
        (value % 100) as i8,
    )
    .ok()
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
    fn run_state_follows_period_and_range() {
        // G531：ruleIndex=89 -> t.rule=(7, 0b1111)，隔日之外的「周一周二」停开。
        let from = 20261001;
        let to = 20261124;
        // 区间外。
        assert_eq!(run_state(from, to, 20260930, 7, 0b1111), RunState::NotYet);
        assert_eq!(run_state(from, to, 20261125, 7, 0b1111), RunState::Expired);
        // 区间内按 period 取模：第 0/1/2/3 天开行，第 4/5/6 天停开。
        for (offset, expected) in [
            (0, RunState::Running),
            (1, RunState::Running),
            (2, RunState::Running),
            (3, RunState::Running),
            (4, RunState::Stopped),
            (5, RunState::Stopped),
            (6, RunState::Stopped),
            (7, RunState::Running),
        ] {
            let date = add_days(from, offset);
            assert_eq!(
                run_state(from, to, date, 7, 0b1111),
                expected,
                "offset={offset}"
            );
        }
        // period=1 或没有规则表时，区间内天天开行。
        assert_eq!(run_state(from, to, from + 1, 1, 0), RunState::Running);
        assert_eq!(run_state(from, to, from + 1, 7, 0), RunState::Running);
    }

    fn add_days(from: i64, days: i64) -> i64 {
        let date = to_date(from).unwrap();
        let shifted = date.checked_add(jiff::Span::new().days(days)).unwrap();
        ymd(shifted)
    }

    #[test]
    fn duration_formats() {
        assert_eq!(duration_cn(106), "1小时46分");
        assert_eq!(duration_cn(45), "45分");
        assert_eq!(duration_cn(1440 + 125), "1天2小时05分");
        assert_eq!(duration_cn(-5), "0分");
    }
}
