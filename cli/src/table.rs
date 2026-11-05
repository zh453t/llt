//! 终端表格输出：按**显示宽度**对齐，中文/全角字符占 2 列。
//!
//! `format!("{:<10}", "北京")` 按字符数补齐，中文会错位，所以这里统一用
//! `unicode-width` 计算宽度。

use unicode_width::UnicodeWidthStr;

pub struct Table {
    headers: Vec<&'static str>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: &[&'static str]) -> Self {
        Self {
            headers: headers.to_vec(),
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: Vec<String>) {
        debug_assert_eq!(row.len(), self.headers.len(), "表格列数不一致");
        self.rows.push(row);
    }

    pub fn print(&self) {
        let widths = self.widths();
        let header: Vec<String> = self.headers.iter().map(|text| (*text).to_owned()).collect();
        println!("{}", row_line(&header, &widths));
        let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
        println!("{}", row_line(&separator, &widths));
        for row in &self.rows {
            println!("{}", row_line(row, &widths));
        }
    }

    fn widths(&self) -> Vec<usize> {
        self.headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                self.rows
                    .iter()
                    .map(|row| width(&row[column]))
                    .chain(std::iter::once(width(header)))
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    }
}

fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn row_line(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| {
            let padding = width.saturating_sub(self::width(cell));
            format!("{cell}{}", " ".repeat(padding))
        })
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_count_cjk_as_two_columns() {
        let mut table = Table::new(&["车站", "里程"]);
        table.push(vec!["北京南".to_owned(), "1318".to_owned()]);
        table.push(vec!["广州白云".to_owned(), "4974".to_owned()]);
        // 北京南 = 6 列，广州白云 = 8 列；里程 = 4 列
        assert_eq!(table.widths(), vec![8, 4]);
    }

    #[test]
    fn row_pads_by_display_width() {
        let cells = vec!["北京南".to_owned(), "1318".to_owned()];
        let line = row_line(&cells, &[8, 4]);
        assert_eq!(line, "北京南    1318");
        // 8 列宽 + 2 空格分隔 + 4 列 = 14，且末尾不留空格
        assert!(!line.ends_with(' '));
    }

    #[test]
    fn short_values_are_padded() {
        let cells = vec!["甲".to_owned(), "1".to_owned()];
        assert_eq!(row_line(&cells, &[6, 3]), "甲      1");
    }
}
