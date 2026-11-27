//! 最小 SGR 解析：把带 ANSI 转义的状态文本转成 ratatui 样式片段（纯函数）。
//!
//! 背景：扩展经 `setStatus` 上报的 `statusText` 内嵌 ANSI 序列（pi-tui 是
//! 字符串透传架构，序列原样进终端由终端着色）；ratatui 是结构体架构，颜色
//! 属于 `Style`，只滤掉 `0x1b` 字节会把 `[38;2;...m` 等可打印残留显示出来。
//! 本模块做反向转换：`ANSI -> Vec<Span>`，序列边界语义对齐 pi-tui 的
//! `ansiCodeLength`（CSI 扫至终止字节、OSC/APC 扫至 BEL 或 ST）。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// 转义序列类别（内部判别）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqKind {
    /// CSI SGR（终止符 `m`）——需解析参数并更新样式。
    Sgr,
    /// 其余序列（未知 CSI / OSC / APC / 裸 ESC）——整段跳过。
    Other,
}

/// 把可能内嵌 ANSI 转义序列的字符串解析为样式化片段序列。
///
/// 功能：识别 SGR 序列并映射为 ratatui `Style`（真彩/256 色/标准 8-16 色、
///      bold/dim/italic/underline 及各 reset）；未知 CSI/OSC/APC 序列整段
///      跳过不显示；U+200B 零宽空格剔除；任意输入不 panic。
/// 输入: text — 待解析文本（可含转义序列，任意字节内容）；base — 初始样式，
///      也是 `ESC[m` / `ESC[0m` 复位的目标样式。
/// 输出: `Vec<Span<'static>>`——可见文本按样式变化切段；转义序列残留绝不
///       出现在片段内容中；序列缺终止符（截断于序列中点）时丢弃从 ESC 起
///       的剩余部分，以杜绝残留。
pub(crate) fn parse_ansi_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = base;
    let mut buf = String::new();
    let mut i = 0usize;
    while i < text.len() {
        let byte = text.as_bytes()[i];
        if byte == 0x1b {
            match classify_escape(text, i) {
                Some((len, kind)) => {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    if kind == SeqKind::Sgr {
                        // 序列形如 ESC [ <params> m：len 至少 3，参数区间为
                        // [i+2, i+len-1)（两端均为 ASCII，切片必在字符边界）。
                        let params = &text[i + 2..i + len - 1];
                        apply_sgr(&mut style, base, params);
                    }
                    i += len;
                }
                None => {
                    // 截断序列（缺终止符）：丢弃从 ESC 起的剩余部分，杜绝残留。
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    break;
                }
            }
        } else {
            // 可见字符按完整 char 前进（多字节 UTF-8 安全）。
            let ch = text[i..].chars().next().expect("i 必在字符边界且小于 len");
            i += ch.len_utf8();
            if ch != '\u{200b}' {
                buf.push(ch);
            }
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    spans
}

/// 识别从 `start`（必须是 `0x1b`）起的转义序列边界。
///
/// 功能：按 pi-tui `ansiCodeLength` 语义判定序列类别与长度；CSI 扫描至首个
///      终止字节（0x40..=0x7E，覆盖 pi 认可的 m/G/K/H/J 并扩展到 h/l 等
///      其余合法终止符——pi 集合外的序列在 pi 处会残留，在此被正确跳过）；
///      OSC/APC 扫描至 BEL(0x07) 或 ST(ESC \)；裸 ESC（含结尾）仅跳过自身。
/// 输入: text — 完整字符串；start — `0x1b` 的字节下标（调用方保证）。
/// 输出: `Some((序列字节长, 类别))`；CSI/OSC/APC 缺终止符（截断）时 `None`，
///       调用方应丢弃剩余文本。
fn classify_escape(text: &str, start: usize) -> Option<(usize, SeqKind)> {
    let bytes = text.as_bytes();
    match bytes.get(start + 1) {
        Some(b'[') => {
            let mut j = start + 2;
            while j < bytes.len() {
                let c = bytes[j];
                // UTF-8 多字节字节均 >= 0x80，不会误中 0x40..=0x7E 区间，
                // 终止位置必在字符边界。
                if (0x40..=0x7e).contains(&c) {
                    let kind = if c == b'm' { SeqKind::Sgr } else { SeqKind::Other };
                    return Some((j + 1 - start, kind));
                }
                j += 1;
            }
            None
        }
        Some(b']') | Some(b'_') => {
            let mut j = start + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return Some((j + 1 - start, SeqKind::Other));
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return Some((j + 2 - start, SeqKind::Other));
                }
                j += 1;
            }
            None
        }
        // 裸 ESC（ESC 结尾，或 ESC + 非 [ ] _ 字符）：仅跳过 ESC 自身，
        // 后续字符按可见文本处理（对齐 pi：length 0 时只跳过 ESC 字节）。
        _ => Some((1, SeqKind::Other)),
    }
}

/// 应用一条 SGR 参数串到当前样式。
///
/// 功能：按 SGR 语义更新 `cur`——空参数/0 复位到 `base`；1/2/3/4 置
///      bold/dim/italic/underline；21/22/23/24 各自清除（22 同清 bold+dim）；
///      39/49 前景/背景回到 base；30-37/90-97/40-47/100-107 标准与亮色；
///      38/48 支持 `;5;n`（256 色）与 `;2;r;g;b`（真彩）子参数；未知码与
///      非数字参数无操作（对齐 pi 的 default 分支）。
/// 输入: cur — 待更新的当前样式；base — 复位目标；params — `ESC[` 与 `m`
///      之间的参数串（如 `"38;2;0;255;136"`、`""`）。
/// 输出: 无返回值；副作用为原地修改 `cur`。畸形参数不 panic。
fn apply_sgr(cur: &mut Style, base: Style, params: &str) {
    if params.is_empty() {
        *cur = base;
        return;
    }
    let parts: Vec<&str> = params.split(';').collect();
    let mut idx = 0usize;
    while idx < parts.len() {
        match parts[idx].parse::<u16>().ok() {
            Some(0) => *cur = base,
            Some(1) => cur.add_modifier |= Modifier::BOLD,
            Some(2) => cur.add_modifier |= Modifier::DIM,
            Some(3) => cur.add_modifier |= Modifier::ITALIC,
            Some(4) => cur.add_modifier |= Modifier::UNDERLINED,
            Some(21) => {
                cur.add_modifier.remove(Modifier::BOLD);
            }
            Some(22) => {
                cur.add_modifier.remove(Modifier::BOLD | Modifier::DIM);
            }
            Some(23) => {
                cur.add_modifier.remove(Modifier::ITALIC);
            }
            Some(24) => {
                cur.add_modifier.remove(Modifier::UNDERLINED);
            }
            Some(39) => cur.fg = base.fg,
            Some(49) => cur.bg = base.bg,
            Some(c @ 30..=37) => cur.fg = Some(basic_color(c - 30)),
            Some(c @ 90..=97) => cur.fg = Some(bright_color(c - 90)),
            Some(c @ 40..=47) => cur.bg = Some(basic_color(c - 40)),
            Some(c @ 100..=107) => cur.bg = Some(bright_color(c - 100)),
            Some(38) | Some(48) => {
                let is_fg = parts[idx] == "38";
                // 子参数（;5;n 或 ;2;r;g;b）完整合法时一并消费，否则只消费码位。
                idx += apply_extended_color(cur, &parts, idx, is_fg);
            }
            // 未知码（含 5 blink/7 inverse 等超出规格子集的码）与非数字：无操作。
            _ => {}
        }
        idx += 1;
    }
}

/// 应用 38/48 扩展色（256 色或真彩）到当前样式。
///
/// 功能：`38;5;n` → `Color::Indexed(n)`；`38;2;r;g;b` → `Color::Rgb`；
///      按 is_fg 写入前景或背景；子参数缺失或越界（如 n>255）时无操作。
/// 输入: cur — 当前样式；parts — 按 `;` 切分的参数段；idx — 38/48 码位下标；
///      is_fg — true 写前景，false 写背景。
/// 输出: 实际消费的子参数个数（2 或 4；不合法时 0）——调用方据此前移游标。
fn apply_extended_color(cur: &mut Style, parts: &[&str], idx: usize, is_fg: bool) -> usize {
    let (color, skip) = match parts.get(idx + 1) {
        Some(&"5") => match parts.get(idx + 2).and_then(|s| s.parse::<u8>().ok()) {
            Some(n) => (Some(Color::Indexed(n)), 2),
            None => (None, 0),
        },
        Some(&"2") => {
            let rgb = parts.get(idx + 2..idx + 5).and_then(|p| {
                Some((
                    p[0].parse::<u8>().ok()?,
                    p[1].parse::<u8>().ok()?,
                    p[2].parse::<u8>().ok()?,
                ))
            });
            match rgb {
                Some((r, g, b)) => (Some(Color::Rgb(r, g, b)), 4),
                None => (None, 0),
            }
        }
        _ => (None, 0),
    };
    if let Some(color) = color {
        if is_fg {
            cur.fg = Some(color);
        } else {
            cur.bg = Some(color);
        }
    }
    skip
}

/// 标准 8 色映射（SGR 30-37 / 40-47 的低 3 位）。
///
/// 输入: n — 0..=7（超出范围按 7 处理，调用方区间已约束）。
/// 输出: 对应 ratatui `Color`（37 按 ratatui 命名为 Gray）。
fn basic_color(n: u16) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

/// 亮色映射（SGR 90-97 / 100-107 的低 3 位）。
///
/// 输入: n — 0..=7（超出范围按 7 处理，调用方区间已约束）。
/// 输出: 对应 ratatui 亮色 `Color`（97 按 ratatui 命名为 White）。
fn bright_color(n: u16) -> Color {
    match n {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼接片段可见文本（断言用）。
    fn visible(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn truecolor_sequence_becomes_rgb_span() {
        let spans = parse_ansi_spans("\x1b[38;2;0;255;136m25.0 tok/s\x1b[0m", Style::default());
        assert_eq!(visible(&spans), "25.0 tok/s");
        assert!(!visible(&spans).contains('\u{1b}'), "无 ESC 残留");
        assert!(!visible(&spans).contains("[38;2;"), "无可打印转义残留");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0, 255, 136)));
    }

    #[test]
    fn token_speed_plugin_text_renders_clean() {
        // 复刻 pi-token-speed 实际产出：theme dim 前缀 + 真彩数值 + ZWSP。
        let text = "\x1b[38;2;90;90;90m\x1b[2m⚡ TPS:\x1b[22;39m \x1b[38;2;0;255;136m25.0 tok/s\x1b[0m\u{200b}";
        let spans = parse_ansi_spans(text, Style::default());
        assert_eq!(visible(&spans), "⚡ TPS: 25.0 tok/s", "ZWSP 剔除、序列零残留");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(90, 90, 90)));
        assert!(spans[0].style.add_modifier.contains(Modifier::DIM));
        let value = spans.iter().find(|s| s.content.contains("25.0")).expect("数值段");
        assert_eq!(value.style.fg, Some(Color::Rgb(0, 255, 136)));
        assert!(
            !value.style.add_modifier.contains(Modifier::DIM),
            "22 已清 dim"
        );
    }

    #[test]
    fn unknown_csi_and_osc_sequences_skipped() {
        // 清屏 CSI（J 终止）：pi 集合内。
        assert_eq!(
            visible(&parse_ansi_spans("ab\x1b[2Jcd", Style::default())),
            "abcd"
        );
        // DECSET（?25h，h 不在 pi 的 m/G/K/H/J 集合）：扩展扫描正确跳过。
        assert_eq!(
            visible(&parse_ansi_spans("a\x1b[?25hb", Style::default())),
            "ab"
        );
        // OSC 8 超链接（BEL 与 ST 两种终止）。
        assert_eq!(
            visible(&parse_ansi_spans("a\x1b]8;;http://x\x07b\x1b]8;;\x1b\\c", Style::default())),
            "abc"
        );
        // APC 序列（ESC _ ... BEL）。
        assert_eq!(
            visible(&parse_ansi_spans("a\x1b_payload\x07b", Style::default())),
            "ab"
        );
    }

    #[test]
    fn malformed_inputs_never_panic() {
        let cases = [
            "",                  // 空串
            "plain",             // 纯文本
            "\x1b",              // 裸 ESC 结尾
            "\x1b[",             // CSI 截断
            "\x1b[38;2",         // 缺终止符（截断于序列中点）
            "\x1b[38;2;",        // 缺终止符 + 尾分号
            "\x1b]",             // OSC 截断
            "\x1b]8;;http",      // OSC 缺终止符
            "\x1b[m",            // 空参数
            "\x1b[0m",
            "\x1b[99999m",       // 超大未知码
            "\x1b[38;5;300m",    // 256 色索引越界
            "\x1b[38;2;1;2m",    // 真彩参数不足
            "\x1b[31;abc;4m",    // 混入非数字参数
            "\x1b[1;;2m",        // 空参数段
            "\x1b[38;2;0;255;136m", // 序列后无文本
            "你好\x1b[31m世界",  // 多字节夹序列
            "\x1b[你好m",        // CSI 参数含多字节
        ];
        for case in cases {
            let spans = parse_ansi_spans(case, Style::default());
            let vis = visible(&spans);
            assert!(
                !vis.contains('\u{1b}'),
                "输入 {case:?} 产生 ESC 残留: {vis:?}"
            );
        }
    }

    #[test]
    fn attribute_resets_are_surgical() {
        // 22 只清 bold+dim，不清 italic。
        let spans = parse_ansi_spans("\x1b[1m\x1b[2m\x1b[3mA\x1b[22mB", Style::default());
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "A");
        assert!(
            spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::DIM | Modifier::ITALIC)
        );
        assert_eq!(spans[1].content, "B");
        assert_eq!(spans[1].style.add_modifier, Modifier::ITALIC);
        // 23 只清 italic。
        let spans = parse_ansi_spans("\x1b[3mA\x1b[23mB", Style::default());
        assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
        assert!(spans[1].style.add_modifier.is_empty());
        // 1 只置 bold。
        let spans = parse_ansi_spans("\x1b[1mX", Style::default());
        assert_eq!(spans[0].style.add_modifier, Modifier::BOLD);
    }

    #[test]
    fn colors_and_resets() {
        // 标准色 + 39 回到 base。
        let spans = parse_ansi_spans("\x1b[31mred\x1b[39mplain", Style::default());
        assert_eq!(spans[0].style.fg, Some(Color::Red));
        assert_eq!(spans[1].content, "plain");
        assert_eq!(spans[1].style.fg, None, "39 回到 base.fg");
        // 亮色 90-97。
        assert_eq!(
            parse_ansi_spans("\x1b[92mgreen", Style::default())[0].style.fg,
            Some(Color::LightGreen)
        );
        // 256 色。
        assert_eq!(
            parse_ansi_spans("\x1b[38;5;208morange", Style::default())[0].style.fg,
            Some(Color::Indexed(208))
        );
        // 背景真彩。
        assert_eq!(
            parse_ansi_spans("\x1b[48;2;1;2;3mbg", Style::default())[0].style.bg,
            Some(Color::Rgb(1, 2, 3))
        );
        // 0 复位到 base（含 base 自带前景）。
        let base = Style::default().fg(Color::Magenta);
        let spans = parse_ansi_spans("\x1b[31mA\x1b[0mB", base);
        assert_eq!(spans[0].style.fg, Some(Color::Red));
        assert_eq!(spans[1].style.fg, Some(Color::Magenta), "0 回到 base");
    }
}
