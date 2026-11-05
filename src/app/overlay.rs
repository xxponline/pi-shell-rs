//! overlay 弹层框架：浮层状态、纯键处理、渲染（选择器 + 扩展对话 4 形态）。
//!
//! 焦点约定：浮层打开时所有非 ctrl+c 按键先交 [`overlay_key`]，返回
//! [`OverlayOutcome`] 决定关闭/应用/继续编辑；未消费的语义由 App 侧忽略，
//! 保证主界面键位表零改动。渲染为纯函数，TestBackend 可测。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use serde_json::Value;

/// 一条可选模型（来自 get_available_models 的 Model 摘要）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelItem {
    pub provider: String,
    pub id: String,
    pub name: String,
}

/// 浮层形态（单槽 + App 侧队列串行）。
#[derive(Debug, Clone)]
pub enum Overlay {
    /// model 选择器（loading 时 items 为空）。
    ModelSelect {
        items: Vec<ModelItem>,
        loading: bool,
        cursor: usize,
        seq: u64,
    },
    /// thinking 级别选择器。
    ThinkingSelect {
        levels: Vec<String>,
        loading: bool,
        cursor: usize,
        seq: u64,
    },
    /// 扩展 select 请求。
    ExtSelect {
        id: String,
        title: String,
        options: Vec<String>,
        cursor: usize,
    },
    /// 扩展 confirm 请求（按钮：true=确认 false=取消）。
    ExtConfirm {
        id: String,
        title: String,
        message: String,
        confirm_selected: bool,
    },
    /// 扩展 input 请求（单行缓冲）。
    ExtInput {
        id: String,
        title: String,
        placeholder: String,
        buffer: String,
    },
    /// 扩展 editor 请求（多行编辑，row/col 为字符光标）。
    ExtEditor {
        id: String,
        title: String,
        lines: Vec<String>,
        row: usize,
        col: usize,
    },
}

/// 一次浮层按键的结果。
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayOutcome {
    /// 按键已被浮层消费（含无实际效果的情况）。
    Handled,
    /// 关闭浮层；扩展对话携带回复载荷（None = 选择器取消，不回发）。
    Close { reply: Option<Value> },
    /// 选择器：应用第 index 项。
    Apply(usize),
}

/// 居中矩形：按百分比在给定区域内取中心块。
///
/// 输入: percent_x/percent_y — 相对宽高百分比(1-100)；area — 父区域
/// 输出: 居中的 Rect（至少 1x1）
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}

/// 浮层键处理（纯函数）。
///
/// 输入: o — 可变浮层状态；key — 原始按键
/// 输出: OverlayOutcome——Handled(继续存在) / Close(关闭,可带回复) / Apply(选择器确认)
pub fn overlay_key(o: &mut Overlay, key: KeyEvent) -> OverlayOutcome {
    // ctrl+c 由 App 全局处理，不进此函数；这里兜底忽略其它修饰键组合。
    match o {
        Overlay::ModelSelect {
            items, loading, cursor, ..
        } => {
            if *loading {
                return OverlayOutcome::Handled;
            }
            match key.code {
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    OverlayOutcome::Handled
                }
                KeyCode::Down => {
                    if *cursor + 1 < items.len() {
                        *cursor += 1;
                    }
                    OverlayOutcome::Handled
                }
                KeyCode::Enter if !items.is_empty() => OverlayOutcome::Apply(*cursor),
                KeyCode::Esc => OverlayOutcome::Close { reply: None },
                _ => OverlayOutcome::Handled,
            }
        }
        Overlay::ThinkingSelect {
            levels,
            loading,
            cursor,
            ..
        } => {
            if *loading {
                return OverlayOutcome::Handled;
            }
            match key.code {
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    OverlayOutcome::Handled
                }
                KeyCode::Down => {
                    if *cursor + 1 < levels.len() {
                        *cursor += 1;
                    }
                    OverlayOutcome::Handled
                }
                KeyCode::Enter if !levels.is_empty() => OverlayOutcome::Apply(*cursor),
                KeyCode::Esc => OverlayOutcome::Close { reply: None },
                _ => OverlayOutcome::Handled,
            }
        }
        Overlay::ExtSelect {
            id,
            options,
            cursor,
            ..
        } => match key.code {
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
                OverlayOutcome::Handled
            }
            KeyCode::Down => {
                if *cursor + 1 < options.len() {
                    *cursor += 1;
                }
                OverlayOutcome::Handled
            }
            KeyCode::Enter => {
                let value = options.get(*cursor).cloned().unwrap_or_default();
                OverlayOutcome::Close {
                    reply: Some(reply_value(id, Value::String(value))),
                }
            }
            KeyCode::Esc => OverlayOutcome::Close {
                reply: Some(cancelled_reply(id)),
            },
            _ => OverlayOutcome::Handled,
        },
        Overlay::ExtConfirm {
            id,
            confirm_selected,
            ..
        } => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                *confirm_selected = !*confirm_selected;
                OverlayOutcome::Handled
            }
            KeyCode::Enter => OverlayOutcome::Close {
                reply: Some(if *confirm_selected {
                    reply_confirmed(id, true)
                } else {
                    reply_confirmed(id, false)
                }),
            },
            KeyCode::Esc => OverlayOutcome::Close {
                reply: Some(cancelled_reply(id)),
            },
            _ => OverlayOutcome::Handled,
        },
        Overlay::ExtInput { id, buffer, .. } => match key.code {
            KeyCode::Char(c) => {
                buffer.push(c);
                OverlayOutcome::Handled
            }
            KeyCode::Backspace => {
                buffer.pop();
                OverlayOutcome::Handled
            }
            KeyCode::Enter => OverlayOutcome::Close {
                reply: Some(reply_value(id, Value::String(buffer.clone()))),
            },
            KeyCode::Esc => OverlayOutcome::Close {
                reply: Some(cancelled_reply(id)),
            },
            _ => OverlayOutcome::Handled,
        },
        Overlay::ExtEditor {
            id,
            lines,
            row,
            col,
            ..
        } => editor_key(id, lines, row, col, key),
    }
}

/// editor 形态的按键处理（Enter 提交、alt+enter 换行、Backspace 行首回并）。
///
/// 输入: id — 请求 id；lines/row/col — 缓冲与字符光标；key — 原始按键
/// 输出: OverlayOutcome
fn editor_key(
    id: &str,
    lines: &mut Vec<String>,
    row: &mut usize,
    col: &mut usize,
    key: KeyEvent,
) -> OverlayOutcome {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char(c) if !alt => {
            if *row >= lines.len() {
                lines.push(String::new());
            }
            let line = &mut lines[*row];
            let byte_idx = char_byte_idx(line, *col);
            line.insert(byte_idx, c);
            *col += 1;
            OverlayOutcome::Handled
        }
        KeyCode::Enter if alt => {
            // 换行：在光标处切分当前行。
            let old = std::mem::take(lines.get_mut(*row).unwrap_or(&mut String::new()));
            let byte_idx = char_byte_idx(&old, *col);
            let (left, right) = old.split_at(byte_idx);
            let (left, right) = (left.to_string(), right.to_string());
            lines[*row] = left;
            lines.insert(*row + 1, right);
            *row += 1;
            *col = 0;
            OverlayOutcome::Handled
        }
        KeyCode::Enter => {
            let text = lines.join("\n");
            OverlayOutcome::Close {
                reply: Some(reply_value(id, Value::String(text))),
            }
        }
        KeyCode::Esc => OverlayOutcome::Close {
            reply: Some(cancelled_reply(id)),
        },
        KeyCode::Backspace => {
            if *col > 0 {
                if *row >= lines.len() {
                    lines.push(String::new());
                }
                let line = &mut lines[*row];
                let byte_idx = char_byte_idx(line, *col - 1);
                let end = char_byte_idx(line, *col);
                line.replace_range(byte_idx..end, "");
                *col -= 1;
            } else if *row > 0 {
                // 行首回并上一行。
                let cur = lines.remove(*row);
                *row -= 1;
                *col = lines[*row].chars().count();
                lines[*row].push_str(&cur);
            }
            OverlayOutcome::Handled
        }
        KeyCode::Left => {
            if *col > 0 {
                *col -= 1;
            } else if *row > 0 {
                *row -= 1;
                *col = lines[*row].chars().count();
            }
            OverlayOutcome::Handled
        }
        KeyCode::Right => {
            let len = lines.get(*row).map(|l| l.chars().count()).unwrap_or(0);
            if *col < len {
                *col += 1;
            } else if *row + 1 < lines.len() {
                *row += 1;
                *col = 0;
            }
            OverlayOutcome::Handled
        }
        KeyCode::Up => {
            if *row > 0 {
                *row -= 1;
                let len = lines.get(*row).map(|l| l.chars().count()).unwrap_or(0);
                *col = (*col).min(len);
            }
            OverlayOutcome::Handled
        }
        KeyCode::Down => {
            if *row + 1 < lines.len() {
                *row += 1;
                let len = lines.get(*row).map(|l| l.chars().count()).unwrap_or(0);
                *col = (*col).min(len);
            }
            OverlayOutcome::Handled
        }
        _ => OverlayOutcome::Handled,
    }
}

/// 字符索引 → 字节偏移（char 安全）。
///
/// 输入: s — 行文本；char_idx — 字符序号（越界取末尾）
/// 输出: 可用于 insert/split_at 的字节偏移
fn char_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// 构造 `value` 型回复载荷。
fn reply_value(id: &str, value: Value) -> Value {
    serde_json::json!({"type": "extension_ui_response", "id": id, "value": value})
}

/// 构造 `confirmed` 型回复载荷。
fn reply_confirmed(id: &str, confirmed: bool) -> Value {
    serde_json::json!({"type": "extension_ui_response", "id": id, "confirmed": confirmed})
}

/// 构造 `cancelled` 回复载荷。
fn cancelled_reply(id: &str) -> Value {
    serde_json::json!({"type": "extension_ui_response", "id": id, "cancelled": true})
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

/// 在帧上绘制当前浮层（Clear 打底 + 居中窗口）。
///
/// 输入: frame — ratatui 帧；overlay — 浮层状态
/// 输出: 无返回值；副作用为在帧最上层绘制
pub fn render_overlay(frame: &mut Frame, overlay: &Overlay) {
    let area = frame.area();
    match overlay {
        Overlay::ModelSelect {
            items,
            loading,
            cursor,
            ..
        } => {
            let title = " Model (ctrl+l) ";
            let lines: Vec<Line> = if *loading {
                vec![Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else if items.is_empty() {
                vec![Line::from(Span::styled(
                    "(no models)",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                items
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let selected = i == *cursor;
                        Line::from(vec![
                            Span::styled(
                                if selected { "> " } else { "  " },
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::styled(
                                m.name.clone(),
                                if selected {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                            Span::styled(
                                format!("  ({})", m.provider),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ])
                    })
                    .collect()
            };
            draw_list(frame, area, title, lines, items.len().min(12) as u16 + 2);
        }
        Overlay::ThinkingSelect {
            levels,
            loading,
            cursor,
            ..
        } => {
            let title = " Thinking level (alt+t) ";
            let lines: Vec<Line> = if *loading {
                vec![Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                levels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let selected = i == *cursor;
                        Line::from(vec![
                            Span::styled(
                                if selected { "> " } else { "  " },
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::styled(
                                l.clone(),
                                if selected {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                        ])
                    })
                    .collect()
            };
            draw_list(frame, area, title, lines, levels.len().min(10) as u16 + 2);
        }
        Overlay::ExtSelect {
            title,
            options,
            cursor,
            ..
        } => {
            let lines: Vec<Line> = options
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let selected = i == *cursor;
                    Line::from(vec![
                        Span::styled(
                            if selected { "> " } else { "  " },
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::styled(
                            o.clone(),
                            if selected {
                                Style::default().add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            },
                        ),
                    ])
                })
                .collect();
            draw_list(
                frame,
                area,
                &format!(" {title} "),
                lines,
                options.len().min(12) as u16 + 2,
            );
        }
        Overlay::ExtConfirm {
            title,
            message,
            confirm_selected,
            ..
        } => {
            let rect = centered_rect(60, 30, area);
            frame.render_widget(Clear, rect);
            let btn = |selected: bool, label: &str| -> Span<'static> {
                Span::styled(
                    format!(" {label} "),
                    if selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                )
            };
            let text = vec![
                Line::from(Span::styled(
                    message.clone(),
                    Style::default(),
                )),
                Line::from(""),
                Line::from(vec![
                    btn(*confirm_selected, "确认"),
                    Span::raw("  "),
                    btn(!*confirm_selected, "取消"),
                ]),
                Line::from(Span::styled(
                    "←/→/Tab 切换 · Enter 确定 · Esc 取消",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "));
            frame.render_widget(Paragraph::new(text).block(block), rect);
        }
        Overlay::ExtInput {
            title,
            placeholder,
            buffer,
            ..
        } => {
            let rect = centered_rect(70, 20, area);
            frame.render_widget(Clear, rect);
            let (content, style) = if buffer.is_empty() {
                (
                    Line::from(Span::styled(
                        placeholder.clone(),
                        Style::default().fg(Color::DarkGray),
                    )),
                    Style::default(),
                )
            } else {
                (Line::from(buffer.clone()), Style::default())
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} (Enter 提交 · Esc 取消) "));
            frame.render_widget(
                Paragraph::new(vec![content]).block(block).style(style),
                rect,
            );
        }
        Overlay::ExtEditor {
            title,
            lines,
            row,
            col,
            ..
        } => {
            let rect = centered_rect(80, 70, area);
            frame.render_widget(Clear, rect);
            let mut display: Vec<Line> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if i == *row {
                    // 当前行：按字符光标切分，光标处插入反色块。
                    let byte = char_byte_idx(line, *col);
                    let (left, right) = line.split_at(byte);
                    display.push(Line::from(vec![
                        Span::raw(left.to_string()),
                        Span::styled(
                            "▏",
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan),
                        ),
                        Span::raw(right.to_string()),
                    ]));
                } else {
                    display.push(Line::from(line.clone()));
                }
            }
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} (Enter 提交 · Alt+Enter 换行 · Esc 取消) "));
            frame.render_widget(Paragraph::new(display).block(block), rect);
        }
    }
}

/// 绘制通用列表浮层（标题 + 行 + 边框），按**行数**精确定位（非百分比）。
///
/// 输入: frame — ratatui 帧；area — 父区域；title — 标题；
///       lines — 列表内容行；rows — 内容行数（不含边框）
/// 输出: 无返回值；副作用为绘制居中浮层
fn draw_list(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, rows: u16) {
    let inner_h = rows.saturating_add(2).min(area.height).max(3);
    let width = area.width.saturating_mul(70) / 100;
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(inner_h) / 2,
        width,
        height: inner_h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} (↑/↓ 导航 · Enter 选择 · Esc 取消) "));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn centered_rect_is_centered() {
        let area = Rect::new(0, 0, 100, 40);
        let r = centered_rect(50, 50, area);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 20);
        assert_eq!(r.x, 25);
        assert_eq!(r.y, 10);
    }

    #[test]
    fn model_select_navigation_and_apply() {
        let mut o = Overlay::ModelSelect {
            items: vec![
                ModelItem { provider: "a".into(), id: "m1".into(), name: "Model One".into() },
                ModelItem { provider: "b".into(), id: "m2".into(), name: "Model Two".into() },
            ],
            loading: false,
            cursor: 0,
            seq: 1,
        };
        assert_eq!(overlay_key(&mut o, key(KeyCode::Down)), OverlayOutcome::Handled);
        assert_eq!(overlay_key(&mut o, key(KeyCode::Enter)), OverlayOutcome::Apply(1));
        // Esc → Close 无回复
        assert_eq!(
            overlay_key(&mut o, key(KeyCode::Esc)),
            OverlayOutcome::Close { reply: None }
        );
        // loading 时按键全部忽略
        let mut loading = Overlay::ModelSelect {
            items: vec![], loading: true, cursor: 0, seq: 1,
        };
        assert_eq!(overlay_key(&mut loading, key(KeyCode::Enter)), OverlayOutcome::Handled);
    }

    #[test]
    fn ext_select_replies_with_option_and_cancel() {
        let mut o = Overlay::ExtSelect {
            id: "r1".into(),
            title: "pick".into(),
            options: vec!["a".into(), "b".into()],
            cursor: 0,
        };
        overlay_key(&mut o, key(KeyCode::Down));
        let out = overlay_key(&mut o, key(KeyCode::Enter));
        match out {
            OverlayOutcome::Close { reply: Some(v) } => {
                assert_eq!(v["value"], "b");
                assert_eq!(v["id"], "r1");
                assert_eq!(v["type"], "extension_ui_response");
            }
            other => panic!("{other:?}"),
        }
        let out = overlay_key(&mut o, key(KeyCode::Esc));
        match out {
            OverlayOutcome::Close { reply: Some(v) } => assert_eq!(v["cancelled"], true),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ext_confirm_toggles_and_replies() {
        let mut o = Overlay::ExtConfirm {
            id: "c1".into(),
            title: "t".into(),
            message: "sure?".into(),
            confirm_selected: true,
        };
        let out = overlay_key(&mut o, key(KeyCode::Enter));
        assert!(matches!(out, OverlayOutcome::Close { reply: Some(v) } if v["confirmed"] == true));
        overlay_key(&mut o, key(KeyCode::Right));
        let out = overlay_key(&mut o, key(KeyCode::Enter));
        assert!(matches!(out, OverlayOutcome::Close { reply: Some(v) } if v["confirmed"] == false));
        // Esc → cancelled
        let mut o2 = Overlay::ExtConfirm {
            id: "c2".into(), title: "t".into(), message: "m".into(), confirm_selected: true,
        };
        let out = overlay_key(&mut o2, key(KeyCode::Esc));
        assert!(matches!(out, OverlayOutcome::Close { reply: Some(v) } if v["cancelled"] == true));
    }

    #[test]
    fn ext_input_edits_and_submits() {
        let mut o = Overlay::ExtInput {
            id: "i1".into(),
            title: "in".into(),
            placeholder: "type…".into(),
            buffer: String::new(),
        };
        for c in "hi".chars() {
            overlay_key(&mut o, key(KeyCode::Char(c)));
        }
        overlay_key(&mut o, key(KeyCode::Backspace));
        overlay_key(&mut o, key(KeyCode::Char('y')));
        let out = overlay_key(&mut o, key(KeyCode::Enter));
        assert!(matches!(out, OverlayOutcome::Close { reply: Some(v) } if v["value"] == "hy"));
    }

    #[test]
    fn ext_editor_multiline_edit_and_submit() {
        let mut o = Overlay::ExtEditor {
            id: "e1".into(),
            title: "ed".into(),
            lines: vec!["line1".into(), "line2".into()],
            row: 0,
            col: 5,
        };
        // 光标行尾插入
        overlay_key(&mut o, key(KeyCode::Char('!')));
        // alt+enter 换行 → 行1 切成 "line1!" 与 ""
        overlay_key(&mut o, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        match &o {
            Overlay::ExtEditor { lines, row, col, .. } => {
                assert_eq!(lines, &vec!["line1!".to_string(), String::new(), "line2".to_string()]);
                assert_eq!((*row, *col), (1, 0));
            }
            _ => unreachable!(),
        }
        // 行首 Backspace 回并上一行
        overlay_key(&mut o, key(KeyCode::Backspace));
        match &o {
            Overlay::ExtEditor { lines, row, col, .. } => {
                assert_eq!(lines, &vec!["line1!".to_string(), "line2".to_string()]);
                assert_eq!((*row, *col), (0, 6));
            }
            _ => unreachable!(),
        }
        // 方向键
        overlay_key(&mut o, key(KeyCode::Down));
        match &o {
            Overlay::ExtEditor { row, col, .. } => {
                // row 0→1；col 6 超出 "line2"(5) 被 clamp 到 5
                assert_eq!((*row, *col), (1, 5));
            }
            _ => unreachable!(),
        }
        // 提交全文
        let out = overlay_key(&mut o, key(KeyCode::Enter));
        match out {
            OverlayOutcome::Close { reply: Some(v) } => {
                assert_eq!(v["value"], "line1!\nline2");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ext_editor_empty_prefill_and_cancel() {
        let mut o = Overlay::ExtEditor {
            id: "e2".into(),
            title: "ed".into(),
            lines: vec![String::new()],
            row: 0,
            col: 0,
        };
        overlay_key(&mut o, key(KeyCode::Char('x')));
        let out = overlay_key(&mut o, key(KeyCode::Esc));
        assert!(matches!(out, OverlayOutcome::Close { reply: Some(v) } if v["cancelled"] == true));
    }

    #[test]
    fn model_selector_content_visible_in_frame() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let overlay = Overlay::ModelSelect {
            items: vec![
                ModelItem { provider: "p1".into(), id: "a".into(), name: "Alpha Model".into() },
                ModelItem { provider: "p2".into(), id: "b".into(), name: "Beta Model".into() },
            ],
            loading: false,
            cursor: 1,
            seq: 1,
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render_overlay(frame, &overlay)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        assert!(out.contains("Model (ctrl+l)"), "标题: {out}");
        assert!(out.contains("Alpha Model"), "条目1: {out}");
        assert!(out.contains("Beta Model"), "条目2: {out}");
        assert!(out.contains("> "), "光标: {out}");

        // loading 态内容行也要可见
        let loading = Overlay::ModelSelect { items: vec![], loading: true, cursor: 0, seq: 1 };
        terminal.draw(|frame| render_overlay(frame, &loading)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out2 = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out2.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(out2.contains("loading"), "loading 行必须可见: {out2}");
    }
}
