//! 中文字体与基础排版设置。
//!
//! 正文中文本来依赖 eframe 的 `system_fonts`：缺字时按需从系统字体里查找
//! （Android 上没有 fontconfig，这条路径找不到东西，全靠下面的候选目录）。
//! 这里再把思源黑体 / 苹方插到 `Proportional` 字体链最前面，让中英文正文
//! 优先用它们；另外注册一个真正的粗体字族，供详情页车次号等少量文字使用。

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontFamily};

/// 正文字体候选，按优先级排列：思源黑体 → 苹方。
///
/// 第二项是 TTC/OTC 集合内的 PostScript 名（`None` 表示文件只有单款字形，
/// 直接用 index 0）。都找不到时保留 egui 默认字体链 + 系统字体回退。
const REGULAR_CANDIDATES: &[(&str, Option<&str>)] = &[
    ("SourceHanSansCN-Regular.otf", None),
    ("SourceHanSansSC-Regular.otf", None),
    ("SourceHanSansCN-Normal.otf", None),
    ("NotoSansCJKsc-Regular.otf", None),
    ("NotoSansCJK-Regular.ttc", Some("NotoSansCJKsc-Regular")),
    ("PingFang.ttc", Some("PingFangSC-Regular")),
];

/// 粗体候选，按优先级排列：思源黑体 → 苹方 → 微软雅黑 → 等线 → 黑体。
///
/// 苹方没有 Bold，Semibold 是它最粗的一档。
const BOLD_CANDIDATES: &[(&str, Option<&str>)] = &[
    // Android 随 APK 带一份思源黑体 Bold 的子集（系统里只有 Regular），
    // 由平台解到应用目录，见 `Platform::font_dirs`。
    ("llt-bold.otf", None),
    ("SourceHanSansCN-Bold.otf", None),
    ("SourceHanSansSC-Bold.otf", None),
    ("SourceHanSansCN-Heavy.otf", None),
    ("PingFang.ttc", Some("PingFangSC-Semibold")),
    ("msyhbd.ttc", None),
    ("Dengb.ttf", None),
    ("simhei.ttf", None),
    ("NotoSansCJK-Bold.ttc", Some("NotoSansCJKsc-Bold")),
];

const BOLD_FAMILY_NAME: &str = "llt-bold";

/// egui 的 `RichText::strong()` 只改颜色；真正加粗需要单独的粗体字体族。
pub fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD_FAMILY_NAME.into())
}

/// 正文中文交给 eframe 的 `system_fonts` 按需回退；
/// 这里把思源黑体 / 苹方插到正文字体链最前，并额外注册一个真正的粗体字族。
pub fn install(ctx: &egui::Context, extra_dirs: &[PathBuf]) {
    let mut fonts = egui::FontDefinitions::default();

    // 正文族：思源黑体 / 苹方优先，其余（Ubuntu-Light、egui-icons）沿用默认链。
    // Monospace 保持 egui 默认的 Hack：时刻、里程等列要等宽对齐。
    if let Some((name, data)) = load_first(REGULAR_CANDIDATES, extra_dirs) {
        fonts.font_data.insert(name.clone(), Arc::new(data));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, name);
    }

    // 粗体族：粗体字体优先，其余沿用正文字体链；找不到粗体时退化为普通字体。
    let mut bold_chain = Vec::new();
    if let Some((name, data)) = load_first(BOLD_CANDIDATES, extra_dirs) {
        fonts.font_data.insert(name.clone(), Arc::new(data));
        bold_chain.push(name);
    }
    if let Some(proportional) = fonts.families.get(&FontFamily::Proportional) {
        bold_chain.extend(proportional.iter().cloned());
    }
    fonts.families.insert(bold_family(), bold_chain.clone());

    // 等宽族也要能显示汉字：egui 默认的 Monospace 只有 Hack（纯 ASCII），
    // 数据版本、路径这类等宽文字里的中文会变成豆腐块。把中文字体追加到它后面，
    // ASCII 仍然走 Hack（等宽对齐），汉字回退到中文字体。
    if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
        for name in &bold_chain {
            if !mono.contains(name) {
                mono.push(name.clone());
            }
        }
    }

    ctx.set_fonts(fonts);
}

/// 候选字体可能所在的目录（按平台）。
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "windows")]
    {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            // 用户自己"右键安装"的字体落在这里，而不是 C:\Windows\Fonts
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
    }

    #[cfg(target_os = "android")]
    {
        // Android 没有 fontconfig，也没有 /usr/share/fonts；系统字体全在 /system/fonts。
        // 中文是 NotoSansCJK-Regular.ttc（集合里选 NotoSansCJKsc-Regular），
        // 少数精简系统只有 NotoSansCJKsc-Regular.otf，两个候选都命中同一套字形。
        dirs.push(PathBuf::from("/system/fonts"));
        dirs.push(PathBuf::from("/product/fonts"));
    }

    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        not(target_os = "android")
    ))]
    {
        for dir in [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/usr/share/fonts/opentype/noto",
            "/usr/share/fonts/truetype/noto",
            "/usr/share/fonts/noto-cjk",
            "/usr/share/fonts/opentype/source-han-sans",
            "/usr/share/fonts/opentype/adobe-source-han-sans",
        ] {
            dirs.push(PathBuf::from(dir));
        }
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join(".local/share/fonts"));
        }
    }

    dirs
}

/// 按优先级找第一个可用候选，返回 (字体名, 字体数据)。
fn load_first(
    candidates: &[(&str, Option<&str>)],
    extra_dirs: &[PathBuf],
) -> Option<(String, FontData)> {
    // 平台给的目录排在最前面：Android 随 APK 带的那份粗体优先级最高。
    let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();
    dirs.extend(font_dirs());
    for (file, wanted_face) in candidates {
        for dir in &dirs {
            let Ok(bytes) = std::fs::read(dir.join(file)) else {
                continue;
            };
            if bytes.is_empty() {
                continue;
            }
            let index = match wanted_face {
                None => 0,
                // 集合里没有想要的那款字形就换下一个候选，避免拿错语种/字重。
                Some(wanted) => match face_index(&bytes, wanted) {
                    Some(index) => index,
                    None => continue,
                },
            };
            return Some((
                (*file).to_owned(),
                FontData::from_blob(Arc::new(bytes), index),
            ));
        }
    }
    None
}

/// 在 TTC/OTC 集合里按 PostScript 名挑 face 序号；单字形文件返回 0。
fn face_index(bytes: &[u8], wanted: &str) -> Option<u32> {
    let wanted = normalize(wanted);
    let offsets: Vec<usize> = if bytes.starts_with(b"ttcf") {
        let count = read_u32(bytes, 8)? as usize;
        (0..count)
            .map(|i| read_u32(bytes, 12 + i * 4).map(|offset| offset as usize))
            .collect::<Option<_>>()?
    } else {
        vec![0]
    };
    offsets
        .iter()
        .enumerate()
        .find(|(_, offset)| face_has_name(bytes, **offset, &wanted))
        .map(|(index, _)| index as u32)
}

/// 某个 face 的 `name` 表里是否有包含 `wanted` 的族名/子族名。
fn face_has_name(bytes: &[u8], face_offset: usize, wanted: &str) -> bool {
    let Some(num_tables) = read_u16(bytes, face_offset + 4) else {
        return false;
    };
    for i in 0..num_tables as usize {
        let record = face_offset + 12 + i * 16;
        if bytes.get(record..record + 4) != Some(b"name") {
            continue;
        }
        let (Some(table), Some(_)) = (read_u32(bytes, record + 8), read_u32(bytes, record + 12))
        else {
            return false;
        };
        let table = table as usize;
        let (Some(count), Some(string_offset)) =
            (read_u16(bytes, table + 2), read_u16(bytes, table + 4))
        else {
            return false;
        };
        for j in 0..count as usize {
            let rec = table + 6 + j * 12;
            // nameID 2/17 是子族名，6 是 PostScript 名。
            let Some(name_id) = read_u16(bytes, rec + 6) else {
                break;
            };
            if !matches!(name_id, 2 | 6 | 17) {
                continue;
            }
            let (Some(platform), Some(len), Some(offset)) = (
                read_u16(bytes, rec),
                read_u16(bytes, rec + 8),
                read_u16(bytes, rec + 10),
            ) else {
                break;
            };
            let start = table + string_offset as usize + offset as usize;
            let Some(raw) = bytes.get(start..start + len as usize) else {
                continue;
            };
            let text = if platform == 1 {
                // Mac Roman，PostScript 名只含 ASCII。
                String::from_utf8_lossy(raw).into_owned()
            } else {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect();
                String::from_utf16_lossy(&units)
            };
            if normalize(&text).contains(wanted) {
                return true;
            }
        }
        return false;
    }
    false
}

/// 忽略大小写、空格与连字符，便于比较 "PingFang SC Semibold" 与 "PingFangSC-Semibold"。
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let pair = bytes.get(at..at + 2)?;
    Some(u16::from_be_bytes([pair[0], pair[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let quad = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ignores_spaces_and_case() {
        assert_eq!(normalize("PingFang SC Semibold"), "pingfangscsemibold");
        assert_eq!(normalize("PingFangSC-Semibold"), "pingfangscsemibold");
    }

    /// 用只含两款字形的 msyh.ttc / simsun.ttc 验证集合挑字形。
    #[test]
    fn picks_face_from_ttc() {
        let cases = [
            (r"C:\Windows\Fonts\msyh.ttc", "Microsoft YaHei UI", 1),
            (r"C:\Windows\Fonts\simsun.ttc", "NSimSun", 1),
        ];
        for (path, wanted, expected) in cases {
            let Ok(bytes) = std::fs::read(path) else {
                continue; // 非 Windows 或字体未安装
            };
            assert_eq!(face_index(&bytes, wanted), Some(expected), "{path}");
        }
    }
}
