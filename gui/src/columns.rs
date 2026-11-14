//! 表格自定义列：四种表各自「能显示哪些列」。
//!
//! 声明顺序就是显示顺序——只让用户挑显示哪些，不做拖动排序（手机上也拖不动）。
//! 配置里存的是列名（`status` / `code` / `from` …，见 [`key`]），这里负责
//! 字符串 ↔ 枚举、表头文案，以及这一列的自然宽度；渲染在 `app.rs`。
//!
//! 宽度是「窄屏用的自然宽度」：表格按它排初始列宽，窗口更宽时 `grow > 0` 的列
//! （站名、区段这类文本）分掉多余空间，时刻/数字列保持不变，这样上下两行的时刻
//! 才能对齐着比。用户还可以拖动列分隔线微调；除状态列外都可以拖。

/// 站站结果表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OdColumn {
    Status,
    Code,
    From,
    Depart,
    To,
    Arrive,
    Duration,
    Km,
    Kind,
    Segment,
    Date,
}

/// 车次结果表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrainColumn {
    Status,
    Code,
    Kind,
    Route,
    Km,
    Days,
    Date,
}

/// 车站结果表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StationColumn {
    Status,
    Code,
    Station,
    Arrive,
    Depart,
    Dwell,
    Day,
    Km,
    Kind,
    Route,
}

/// 车次详情里的停站表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopColumn {
    Station,
    Arrive,
    Depart,
    Dwell,
    Km,
    Day,
    /// 均速：本站里程 ÷ 从始发开车到本站到达的时间；首站没有意义，显示 `---`。
    Speed,
}

/// 一列的静态描述：所有表共用这一份结构，渲染时按它排宽度、定对齐。
#[derive(Debug, Clone, Copy)]
pub struct Spec<C> {
    pub column: C,
    /// 表头文案。
    pub head: &'static str,
    /// 右对齐（时刻、里程、天数这类数字）。
    pub right: bool,
    /// 自然宽度。
    pub width: f32,
    /// 压缩下限：窗口变窄时不低于它，再窄就靠外层横向滚动。
    pub min: f32,
    /// `1` = 参与「多余宽度」分配。没有可长宽的列时，多余宽度全给最后一列。
    pub grow: u8,
}

const fn spec<C>(
    column: C,
    head: &'static str,
    right: bool,
    width: f32,
    min: f32,
    grow: u8,
) -> Spec<C> {
    Spec {
        column,
        head,
        right,
        width,
        min,
        grow,
    }
}

impl OdColumn {
    pub const ALL: &'static [Self] = &[
        Self::Status,
        Self::Code,
        Self::From,
        Self::Depart,
        Self::To,
        Self::Arrive,
        Self::Duration,
        Self::Km,
        Self::Kind,
        Self::Segment,
        Self::Date,
    ];

    pub fn spec(self) -> Spec<Self> {
        match self {
            // 「状态」两个汉字要 26 点，再小表头就被截成「…」。
            Self::Status => spec(self, "状态", false, 34.0, 34.0, 0),
            Self::Code => spec(self, "车次", false, 84.0, 78.0, 0),
            Self::From => spec(self, "出发站", false, 84.0, 74.0, 1),
            Self::Depart => spec(self, "出发时间", false, 68.0, 64.0, 0),
            Self::To => spec(self, "到达站", false, 84.0, 74.0, 1),
            Self::Arrive => spec(self, "到达时间", false, 68.0, 64.0, 0),
            // 「18小时46分」「1天10小时05分」最长要 104 点，再窄就省略号收尾。
            Self::Duration => spec(self, "历时", false, 112.0, 104.0, 0),
            Self::Km => spec(self, "里程", true, 82.0, 74.0, 0),
            Self::Kind => spec(self, "类型", false, 88.0, 80.0, 0),
            Self::Segment => spec(self, "区段", false, 150.0, 100.0, 1),
            Self::Date => spec(self, "开行日期", false, 168.0, 150.0, 0),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Code => "code",
            Self::From => "from",
            Self::Depart => "depart",
            Self::To => "to",
            Self::Arrive => "arrive",
            Self::Duration => "duration",
            Self::Km => "km",
            Self::Kind => "kind",
            Self::Segment => "segment",
            Self::Date => "date",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == key)
    }

    /// 状态列只有状态点，拖宽只会留白；其余列都可以拖动分隔线调整。
    pub fn resizable(self) -> bool {
        !matches!(self, Self::Status)
    }

    /// 按宽度自动挑列（用户没自定义时用）：窄屏留六列基本盘，宽了再加详情。
    pub fn auto(width: f32) -> Vec<Self> {
        let mut columns = vec![
            Self::Status,
            Self::Code,
            Self::From,
            Self::Depart,
            Self::To,
            Self::Arrive,
        ];
        // 宽屏才加这几列：手机上加进来就得横向滑两屏才能看全时刻。
        if width >= 560.0 {
            columns.push(Self::Duration);
        }
        if width >= 840.0 {
            columns.extend([Self::Km, Self::Kind, Self::Segment]);
        }
        columns
    }
}

impl TrainColumn {
    pub const ALL: &'static [Self] = &[
        Self::Status,
        Self::Code,
        Self::Kind,
        Self::Route,
        Self::Km,
        Self::Days,
        Self::Date,
    ];

    pub fn spec(self) -> Spec<Self> {
        match self {
            Self::Status => spec(self, "状态", false, 34.0, 34.0, 0),
            Self::Code => spec(self, "车次", false, 94.0, 78.0, 0),
            Self::Kind => spec(self, "类型", false, 92.0, 80.0, 0),
            Self::Route => spec(self, "始发 → 终到", false, 150.0, 120.0, 1),
            Self::Km => spec(self, "里程", true, 84.0, 76.0, 0),
            Self::Days => spec(self, "天数", true, 58.0, 52.0, 0),
            Self::Date => spec(self, "开行日期", false, 168.0, 150.0, 0),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Code => "code",
            Self::Kind => "kind",
            Self::Route => "route",
            Self::Km => "km",
            Self::Days => "days",
            Self::Date => "date",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == key)
    }

    /// 状态列只有状态点，拖宽只会留白；其余列都可以拖动分隔线调整。
    pub fn resizable(self) -> bool {
        !matches!(self, Self::Status)
    }

    pub fn auto(width: f32) -> Vec<Self> {
        let mut columns = vec![Self::Status, Self::Code, Self::Route];
        if width >= 760.0 {
            columns.extend([Self::Kind, Self::Km, Self::Days]);
        }
        // 很窄时开行日期只放在车次的悬停提示里，避免硬裁掉。
        if width >= 560.0 {
            columns.push(Self::Date);
        }
        columns
    }
}

impl StationColumn {
    pub const ALL: &'static [Self] = &[
        Self::Status,
        Self::Code,
        Self::Station,
        Self::Arrive,
        Self::Depart,
        Self::Dwell,
        Self::Day,
        Self::Km,
        Self::Kind,
        Self::Route,
    ];

    pub fn spec(self) -> Spec<Self> {
        match self {
            Self::Status => spec(self, "状态", false, 34.0, 34.0, 0),
            Self::Code => spec(self, "车次", false, 96.0, 80.0, 0),
            Self::Station => spec(self, "车站", false, 86.0, 72.0, 1),
            Self::Arrive => spec(self, "到达", false, 76.0, 72.0, 0),
            Self::Depart => spec(self, "发车", false, 76.0, 72.0, 0),
            Self::Dwell => spec(self, "停留", true, 64.0, 56.0, 0),
            Self::Day => spec(self, "第几天", true, 58.0, 52.0, 0),
            Self::Km => spec(self, "本站里程", true, 82.0, 74.0, 0),
            Self::Kind => spec(self, "类型", false, 92.0, 80.0, 0),
            Self::Route => spec(self, "始发 → 终到", false, 110.0, 100.0, 1),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Code => "code",
            Self::Station => "station",
            Self::Arrive => "arrive",
            Self::Depart => "depart",
            Self::Dwell => "dwell",
            Self::Day => "day",
            Self::Km => "km",
            Self::Kind => "kind",
            Self::Route => "route",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == key)
    }

    /// 状态列只有状态点，拖宽只会留白；其余列都可以拖动分隔线调整。
    pub fn resizable(self) -> bool {
        !matches!(self, Self::Status)
    }

    /// 自动列：`show_station` = 一屏里可能有多个车站（同城多选），这时才要「车站」列。
    pub fn auto(width: f32, show_station: bool) -> Vec<Self> {
        let mut columns = vec![Self::Status, Self::Code];
        if show_station {
            columns.push(Self::Station);
        }
        columns.extend([Self::Arrive, Self::Depart]);
        if width >= 880.0 {
            columns.extend([Self::Dwell, Self::Day]);
        }
        if width >= 560.0 {
            columns.push(Self::Km);
        }
        if width >= 880.0 {
            columns.push(Self::Kind);
        }
        // 区段是「始发 → 终到」，窄屏上挤不下；有没有车站列影响阈值。
        if width >= if show_station { 620.0 } else { 430.0 } {
            columns.push(Self::Route);
        }
        columns
    }
}

impl StopColumn {
    pub const ALL: &'static [Self] = &[
        Self::Station,
        Self::Arrive,
        Self::Depart,
        Self::Dwell,
        Self::Km,
        Self::Day,
        Self::Speed,
    ];

    pub fn spec(self) -> Spec<Self> {
        match self {
            Self::Station => spec(self, "车站", false, 88.0, 76.0, 1),
            Self::Arrive => spec(self, "到达", false, 66.0, 60.0, 0),
            Self::Depart => spec(self, "发车", false, 66.0, 60.0, 0),
            Self::Dwell => spec(self, "停留", true, 58.0, 52.0, 0),
            Self::Km => spec(self, "里程", true, 66.0, 60.0, 0),
            Self::Day => spec(self, "天", true, 58.0, 52.0, 0),
            Self::Speed => spec(self, "均速", true, 72.0, 64.0, 0),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Station => "station",
            Self::Arrive => "arrive",
            Self::Depart => "depart",
            Self::Dwell => "dwell",
            Self::Km => "km",
            Self::Day => "day",
            Self::Speed => "speed",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == key)
    }

    /// 停站表没有状态列：所有列都可以拖动分隔线调整。
    pub fn resizable(self) -> bool {
        matches!(
            self,
            Self::Station
                | Self::Arrive
                | Self::Depart
                | Self::Dwell
                | Self::Km
                | Self::Day
                | Self::Speed
        )
    }

    pub fn auto(width: f32) -> Vec<Self> {
        let mut columns = vec![Self::Station, Self::Arrive, Self::Depart];
        if width >= 380.0 {
            columns.push(Self::Km);
        }
        columns.push(Self::Day);
        columns
    }
}

/// 把配置里的字符串列表解析成列：认不出的键直接丢掉（手改配置写错一个词，
/// 不该让整份配置回退默认），按配置里的顺序保留、重复的只留第一次出现的。
///
/// 顺序就是显示顺序：设置页里拖动调整列序后写回配置，下次启动照这个顺序排。
/// [`ALL`] 只用来给「自动」和候选列表兜底，不再强制排序。
pub fn parse<C: Copy + PartialEq>(keys: &[String], from_key: fn(&str) -> Option<C>) -> Vec<C> {
    let mut picked: Vec<C> = Vec::new();
    for key in keys {
        if let Some(column) = from_key(key)
            && !picked.contains(&column)
        {
            picked.push(column);
        }
    }
    picked
}

/// 把列写回配置用的字符串列表。
pub fn keys<C>(columns: &[C], key: fn(C) -> &'static str) -> Vec<String>
where
    C: Copy,
{
    columns
        .iter()
        .copied()
        .map(key)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for column in OdColumn::ALL {
            assert_eq!(OdColumn::from_key(column.key()), Some(*column));
        }
        for column in TrainColumn::ALL {
            assert_eq!(TrainColumn::from_key(column.key()), Some(*column));
        }
        for column in StationColumn::ALL {
            assert_eq!(StationColumn::from_key(column.key()), Some(*column));
        }
        for column in StopColumn::ALL {
            assert_eq!(StopColumn::from_key(column.key()), Some(*column));
        }
    }

    #[test]
    fn parse_drops_unknown_and_keeps_config_order() {
        let keys = vec!["arrive".to_owned(), "nope".to_owned(), "status".to_owned()];
        let parsed = parse(&keys, OdColumn::from_key);
        assert_eq!(parsed, vec![OdColumn::Arrive, OdColumn::Status]);

        // 重复的只留第一次出现的位置。
        let keys = vec!["status".to_owned(), "code".to_owned(), "status".to_owned()];
        assert_eq!(
            parse(&keys, OdColumn::from_key),
            vec![OdColumn::Status, OdColumn::Code]
        );
    }

    #[test]
    fn auto_sets_grow_with_width() {
        assert_eq!(OdColumn::auto(340.0).len(), 6);
        assert!(OdColumn::auto(900.0).len() > 6);
        assert!(TrainColumn::auto(720.0).contains(&TrainColumn::Date));
        assert!(!StopColumn::auto(320.0).contains(&StopColumn::Km));
        assert!(StopColumn::auto(420.0).contains(&StopColumn::Km));
    }
}
