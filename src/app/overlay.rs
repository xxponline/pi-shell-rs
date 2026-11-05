//! overlay 弹层框架：浮层状态、纯键处理、渲染（选择器 + 扩展对话 4 形态）。
//!
//! 焦点约定：浮层打开时所有非 ctrl+c 按键先交 [`overlay_key`]，返回
//! [`OverlayOutcome`] 决定关闭/应用/继续编辑；未消费的语义由 App 侧忽略，
//! 保证主界面键位表零改动。渲染为纯函数，TestBackend 可测。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;

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
    /// 会话选择器（/resume）。
    SessionSelect {
        items: Vec<crate::app::sessions::SessionItem>,
        cursor: usize,
        loading: bool,
        seq: u64,
        /// ctrl+d 两段式删除确认态。
        confirming: bool,
    },
    /// 会话树（/tree）。
    Tree {
        /// get_tree 的原始节点数组。
        tree: Vec<serde_json::Value>,
        leaf_id: Option<String>,
        /// 当前可见行（fold+filter 后重建）。
        rows: Vec<crate::app::tree::TreeRow>,
        cursor: usize,
        folded: std::collections::HashSet<String>,
        filter: crate::app::tree::FilterMode,
        loading: bool,
        seq: u64,
    },
    /// 会话信息面板（/session）。
    Info {
        title: String,
        lines: Vec<String>,
        loading: bool,
        seq: u64,
    },
    /// /import 二次确认（目标路径 + 光标行：0=导入 1=取消）。
    ImportConfirm {
        path: String,
        cursor: usize,
    },
    /// 只读模型范围浮层（/scoped-models；无写入能力）。
    ScopedModels {
        models: Vec<String>,
        loading: bool,
        seq: u64,
        cursor: usize,
    },
    /// 简化设置浮层（/settings；两开关 + 只读行）。
    Settings {
        cursor: usize,
        loading: bool,
        seq: u64,
        /// autoCompact 开关（get_state 回填）。
        auto_compaction: Option<bool>,
        /// autoRetry 开关（RPC 无 getter，首次显示 —，切换后本地跟踪）。
        auto_retry: Option<bool>,
        /// 当前模型（只读）。
        model: Option<String>,
    },
    /// 信任决策浮层（/trust；current=当前 cwd 的信任记录）。
    Trust {
        cursor: usize,
        current: Option<bool>,
    },
    /// 键位表浮层（/hotkeys；cursor 为滚动位置）。
    Hotkeys {
        cursor: usize,
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
    /// 会话选择器：对第 index 项执行删除（两段式确认的第二段）。
    Delete(usize),
    /// 应用第 index 项但**不关闭浮层**（设置类反复切换）。
    Toggle(usize),
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
        Overlay::Tree {
            tree,
            leaf_id,
            rows,
            cursor,
            folded,
            filter,
            loading,
            ..
        } => {
            if *loading {
                return match key.code {
                    KeyCode::Esc => OverlayOutcome::Close { reply: None },
                    _ => OverlayOutcome::Handled,
                };
            }
            let rebuild = |tree: &Vec<serde_json::Value>,
                           leaf: &Option<String>,
                           folded: &std::collections::HashSet<String>,
                           filter: crate::app::tree::FilterMode,
                           rows: &mut Vec<crate::app::tree::TreeRow>| {
                *rows = crate::app::tree::flatten_tree(tree, leaf.as_deref(), folded, filter);
            };
            match key.code {
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    OverlayOutcome::Handled
                }
                KeyCode::Down => {
                    if *cursor + 1 < rows.len() {
                        *cursor += 1;
                    }
                    OverlayOutcome::Handled
                }
                KeyCode::Enter if !rows.is_empty() => OverlayOutcome::Apply(*cursor),
                // ctrl+o：过滤模式循环
                KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    *filter = filter.next();
                    rebuild(tree, leaf_id, folded, *filter, rows);
                    OverlayOutcome::Handled
                }
                // ← 折叠选中子树；→ 展开
                KeyCode::Left => {
                    if let Some(row) = rows.get(*cursor)
                        && row.has_children
                    {
                        folded.insert(row.id.clone());
                        rebuild(tree, leaf_id, folded, *filter, rows);
                        if *cursor >= rows.len() {
                            *cursor = rows.len().saturating_sub(1);
                        }
                    }
                    OverlayOutcome::Handled
                }
                KeyCode::Right => {
                    if let Some(row) = rows.get(*cursor)
                        && folded.remove(&row.id)
                    {
                        rebuild(tree, leaf_id, folded, *filter, rows);
                    }
                    OverlayOutcome::Handled
                }
                KeyCode::Esc => OverlayOutcome::Close { reply: None },
                _ => OverlayOutcome::Handled,
            }
        }
        Overlay::ImportConfirm { cursor, .. } => match key.code {
            KeyCode::Esc => OverlayOutcome::Close { reply: None },
            KeyCode::Enter => OverlayOutcome::Apply(*cursor),
            KeyCode::Up | KeyCode::Down => {
                *cursor = 1 - *cursor;
                OverlayOutcome::Handled
            }
            _ => OverlayOutcome::Handled,
        },
        Overlay::ScopedModels { cursor, loading, .. } => {
            if *loading {
                return match key.code {
                    KeyCode::Esc => OverlayOutcome::Close { reply: None },
                    _ => OverlayOutcome::Handled,
                };
            }
            match key.code {
                KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Close { reply: None },
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    OverlayOutcome::Handled
                }
                KeyCode::Down => {
                    *cursor = cursor.saturating_add(1);
                    OverlayOutcome::Handled
                }
                _ => OverlayOutcome::Handled,
            }
        }
        Overlay::Settings {
            cursor, loading, ..
        } => {
            if *loading {
                return match key.code {
                    KeyCode::Esc => OverlayOutcome::Close { reply: None },
                    _ => OverlayOutcome::Handled,
                };
            }
            match key.code {
                KeyCode::Esc => OverlayOutcome::Close { reply: None },
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    OverlayOutcome::Handled
                }
                KeyCode::Down => {
                    *cursor = (*cursor + 1).min(1); // 两行可切换（模型行只读）
                    OverlayOutcome::Handled
                }
                KeyCode::Enter => OverlayOutcome::Toggle(*cursor),
                _ => OverlayOutcome::Handled,
            }
        }
        Overlay::Trust { cursor, .. } => match key.code {
            KeyCode::Esc => OverlayOutcome::Close { reply: None },
            KeyCode::Enter => OverlayOutcome::Apply(*cursor),
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
                OverlayOutcome::Handled
            }
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(2);
                OverlayOutcome::Handled
            }
            _ => OverlayOutcome::Handled,
        },
        Overlay::Hotkeys { cursor } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Close { reply: None },
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
                OverlayOutcome::Handled
            }
            KeyCode::Down => {
                *cursor = cursor.saturating_add(1);
                OverlayOutcome::Handled
            }
            _ => OverlayOutcome::Handled,
        },
        Overlay::Info { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Close { reply: None },
            _ => OverlayOutcome::Handled,
        },
        Overlay::SessionSelect {
            items, cursor, loading, confirming, ..
        } => {
            if *loading {
                // 加载中仅允许 Esc 退出等待
                return match key.code {
                    KeyCode::Esc => OverlayOutcome::Close { reply: None },
                    _ => OverlayOutcome::Handled,
                };
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
                KeyCode::Enter if !items.is_empty() && !*confirming => {
                    OverlayOutcome::Apply(*cursor)
                }
                KeyCode::Char('d')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && !items.is_empty() =>
                {
                    if *confirming {
                        OverlayOutcome::Delete(*cursor)
                    } else {
                        *confirming = true;
                        OverlayOutcome::Handled
                    }
                }
                KeyCode::Esc if *confirming => {
                    // Esc 先退出确认态，再按才关窗
                    *confirming = false;
                    OverlayOutcome::Handled
                }
                KeyCode::Esc => OverlayOutcome::Close { reply: None },
                _ => OverlayOutcome::Handled,
            }
        }
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
/// 键位表数据：(分组标题, 键位, 说明)——分组行为 Some(标题)，普通行为 None。
/// 对照当前实现的真实绑定（key_intent / edit_key / 补全菜单）。
pub fn hotkeys_rows() -> Vec<(Option<&'static str>, &'static str, &'static str)> {
    vec![
        (Some("提交与中断"), "", ""),
        (None, "enter", "发送（流式中发送排队消息）"),
        (None, "alt+enter", "流式中排队 follow_up；空闲等同 enter"),
        (None, "esc", "流式中止；浮层/补全菜单关闭"),
        (None, "alt+up", "取回全部排队消息"),
        (None, "ctrl+c", "单击清空输入；500ms 内双击退出"),
        (None, "ctrl+d", "空输入退出；非空向后删字符"),
        (Some("编辑（Emacs）"), "", ""),
        (None, "shift+enter / ctrl+j", "换行"),
        (None, "ctrl+b / ctrl+f", "光标左/右"),
        (None, "ctrl+a / ctrl+e", "行首 / 行尾"),
        (None, "alt+b / alt+f", "词左 / 词右"),
        (None, "←→↑↓ / home / end", "光标移动"),
        (None, "ctrl+w / alt+backspace", "删词（后向）"),
        (None, "alt+d", "删词（前向）"),
        (None, "ctrl+u / ctrl+k", "删至行首 / 行尾"),
        (None, "ctrl+y / alt+y", "yank / yank-pop"),
        (None, "ctrl+-", "撤销"),
        (Some("会话与模型"), "", ""),
        (None, "ctrl+l", "模型选择器"),
        (None, "ctrl+p / shift+ctrl+p", "模型循环 下一个/上一个"),
        (None, "alt+t", "thinking 级别选择器"),
        (None, "shift+tab", "thinking 级别循环"),
        (None, "ctrl+t", "thinking 块展开/折叠"),
        (None, "ctrl+o", "工具输出展开/折叠"),
        (Some("斜杠命令与补全"), "", ""),
        (None, "/ + 输入", "弹出命令补全菜单"),
        (None, "↑↓ / pageup / pagedown", "菜单导航"),
        (None, "tab", "应用选中补全"),
        (None, "enter", "补全并立即提交"),
        (None, "esc", "只关菜单（不清输入）"),
        (None, "命令一览", "/resume /tree /new /clone /session /compact /name /copy /export /import /changelog /hotkeys /settings /scoped-models /trust /reload /model /thinking /fork /quit（详见 Docs/slash-commands.md）"),
    ]
}

pub fn render_overlay(frame: &mut Frame, overlay: &Overlay, current_session: Option<&str>) {
    let area = frame.area();
    match overlay {
        Overlay::Tree {
            rows,
            cursor,
            filter,
            loading,
            ..
        } => {
            let title = format!(
                " Tree (filter:{}) Enter=分支 ←=折叠 →=展开 ",
                filter.label()
            );
            let lines: Vec<Line> = if *loading {
                vec![Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else if rows.is_empty() {
                vec![Line::from(Span::styled(
                    "(空树)",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                rows.iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let selected = i == *cursor;
                        let indent = "  ".repeat(r.depth);
                        let marker = if r.is_leaf {
                            Span::styled("● ", Style::default().fg(Color::Green))
                        } else if r.has_children {
                            Span::styled("▾ ", Style::default().fg(Color::DarkGray))
                        } else {
                            Span::raw("  ")
                        };
                        let mut spans = vec![
                            Span::styled(
                                if selected { "> " } else { "  " },
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::raw(indent),
                            marker,
                            Span::styled(
                                r.text.clone(),
                                if selected {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                        ];
                        if r.is_leaf {
                            spans.push(Span::styled(
                                " ◀ 当前",
                                Style::default().fg(Color::Green),
                            ));
                        }
                        Line::from(spans)
                    })
                    .collect()
            };
            draw_list(frame, area, &title, lines, rows.len().min(16) as u16 + 2);
        }
        Overlay::ImportConfirm { path, cursor } => {
            let name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let options = ["✓ 导入并切换", "取消"];
            let inner_h = options.len() as u16 + 2;
            let width = (area.width.saturating_mul(60) / 100).max(46);
            let rect = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(inner_h) / 2,
                width,
                height: inner_h,
            };
            frame.render_widget(Clear, rect);
            let lines: Vec<Line> = options
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let style = if i == *cursor {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(format!("  {opt}"), style))
                })
                .collect();
            frame.render_widget(
                Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(format!(
                    " Import {name}（替换当前会话） "
                ))),
                rect,
            );
        }
        Overlay::ScopedModels {
            models,
            loading,
            cursor,
            ..
        } => {
            let mut rows: Vec<Line> = Vec::new();
            if *loading {
                rows.push(Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                )));
            } else if models.is_empty() {
                rows.push(Line::from(Span::styled(
                    "(无可用模型)",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                rows.push(Line::from(Span::styled(
                    "当前会话模型见状态行；本列表只读（写入需 pi /settings，本 TUI 未实现）",
                    Style::default().fg(Color::DarkGray),
                )));
                for (i, m) in models.iter().enumerate() {
                    let selected = i == *cursor;
                    let style = if selected {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    rows.push(Line::from(Span::styled(format!("  {m}"), style)));
                }
            }
            let inner_h = (rows.len() as u16 + 2)
                .min(area.height.saturating_sub(2))
                .max(6);
            let width = (area.width.saturating_mul(60) / 100).max(44);
            let rect = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(inner_h) / 2,
                width,
                height: inner_h,
            };
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(rows).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Scoped Models（只读） "),
                ),
                rect,
            );
        }
        Overlay::Settings {
            cursor,
            loading,
            auto_compaction,
            auto_retry,
            model,
            ..
        } => {
            let rows: Vec<Line> = if *loading {
                vec![Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                let onoff = |v: &Option<bool>| match v {
                    Some(true) => "on".to_string(),
                    Some(false) => "off".to_string(),
                    None => "—".to_string(),
                };
                vec![
                    Line::from(Span::styled(
                        format!("  autoCompact        {}", onoff(auto_compaction)),
                        if *cursor == 0 {
                            Style::default().fg(Color::Black).bg(Color::Cyan)
                        } else {
                            Style::default()
                        },
                    )),
                    Line::from(Span::styled(
                        format!("  autoRetry          {}", onoff(auto_retry)),
                        if *cursor == 1 {
                            Style::default().fg(Color::Black).bg(Color::Cyan)
                        } else {
                            Style::default()
                        },
                    )),
                    Line::from(Span::styled(
                        format!(
                            "  model              {}",
                            model.as_deref().unwrap_or("—")
                        ),
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::from(Span::styled(
                        "  enter 切换开关 · esc 关闭（简化版，详见 Docs/slash-commands.md）",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]
            };
            let inner_h = (rows.len() as u16 + 2).max(5);
            let width = (area.width.saturating_mul(70) / 100).max(50);
            let rect = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(inner_h) / 2,
                width,
                height: inner_h,
            };
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(rows).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Settings（简化） "),
                ),
                rect,
            );
        }
        Overlay::Trust { cursor, current } => {
            let state = match current {
                Some(true) => "已信任",
                Some(false) => "已拒绝",
                None => "未记录",
            };
            let options = ["✓ 信任本项目", "× 清除信任记录", "Esc 关闭"];
            let inner_h = (options.len() as u16 + 2).max(5);
            let width = (area.width.saturating_mul(50) / 100).max(34);
            let rect = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(inner_h) / 2,
                width,
                height: inner_h,
            };
            frame.render_widget(Clear, rect);
            let lines: Vec<Line> = options
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let selected = i == *cursor && i < 2;
                    let style = if selected {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else if i == 2 {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(format!("  {opt}"), style))
                })
                .collect();
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Trust（当前: {state}） ")),
                ),
                rect,
            );
        }
        Overlay::Hotkeys { cursor } => {
            let rows = hotkeys_rows();
            let inner_h = (rows.len() as u16 + 3)
                .min(area.height.saturating_sub(2))
                .max(6);
            let width = area.width.saturating_mul(80) / 100;
            let rect = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(inner_h) / 2,
                width,
                height: inner_h,
            };
            frame.render_widget(Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Hotkeys  ↑↓ 滚动 ");
            // 滚动窗口（选中行可见）+ 高亮
            let visible_h = rect.height.saturating_sub(2) as usize;
            let start = cursor.saturating_sub(visible_h.saturating_sub(1))
                .min(rows.len().saturating_sub(visible_h));
            let lines: Vec<Line> = rows[start..]
                .iter()
                .take(visible_h)
                .enumerate()
                .map(|(i, (group, key, desc))| {
                    let selected = start + i == *cursor;
                    let style = if selected {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else if group.is_some() {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    };
                    let text = match group {
                        Some(g) => format!("── {g} ──"),
                        None => format!("  {key:<16}{desc}"),
                    };
                    Line::from(Span::styled(text, style))
                })
                .collect();
            frame.render_widget(
                Paragraph::new(lines).block(block),
                rect,
            );
        }
        Overlay::Info {
            title,
            lines,
            loading,
            ..
        } => {
            let mut display: Vec<Line> = if *loading {
                vec![Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                lines
                    .iter()
                    .map(|l| Line::from(l.clone()))
                    .collect()
            };
            display.push(Line::from(Span::styled(
                "",
                Style::default(),
            )));
            display.push(Line::from(Span::styled(
                "Esc 关闭",
                Style::default().fg(Color::DarkGray),
            )));
            // 按行数精确定位（非百分比）：内容 + 上下边框
            let inner_h = (display.len() as u16 + 2)
                .min(area.height.saturating_sub(2))
                .max(4);
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
                .title(format!(" {title} "));
            frame.render_widget(Paragraph::new(display).block(block), rect);
        }
        Overlay::SessionSelect {
            items,
            cursor,
            loading,
            confirming,
            ..
        } => {
            let title = if *confirming {
                " Resume · ⚠ 再按 ctrl+d 确认删除，Esc 取消 ".to_string()
            } else {
                " Resume (Enter 切换 · ctrl+d 删除) ".to_string()
            };
            let now = std::time::SystemTime::now();
            let lines: Vec<Line> = if *loading {
                vec![Line::from(Span::styled(
                    "loading…",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else if items.is_empty() {
                vec![Line::from(Span::styled(
                    "(无会话)",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let selected = i == *cursor;
                        let label = it
                            .name
                            .clone()
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| {
                                if it.first_message.is_empty() {
                                    "(空会话)".to_string()
                                } else {
                                    it.first_message.clone()
                                }
                            });
                        let mut spans = vec![
                            Span::styled(
                                if selected { "> " } else { "  " },
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::styled(
                                truncate_text(&label, 52),
                                if selected {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                            Span::styled(
                                format!("  {}", crate::app::sessions::rel_time(it.modified, now)),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ];
                        // 当前会话标记
                        if current_session
                            .map(|c| Path::new(c) == it.path.as_path())
                            .unwrap_or(false)
                        {
                            spans.insert(
                                0,
                                Span::styled("• ", Style::default().fg(Color::Green)),
                            );
                        }
                        Line::from(spans)
                    })
                    .collect()
            };
            draw_list(frame, area, &title, lines, items.len().min(12) as u16 + 2);
        }
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

/// 字符安全截断（浮层行内标签用）。
///
/// 输入: text — 原文；max — 最大字符数
/// 输出: 截断串（超长以 … 结尾）
fn truncate_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let taken: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{taken}…")
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
        terminal.draw(|frame| render_overlay(frame, &overlay, None)).unwrap();
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
        terminal.draw(|frame| render_overlay(frame, &loading, None)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out2 = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out2.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(out2.contains("loading"), "loading 行必须可见: {out2}");
    }

    #[test]
    fn session_select_keys_and_two_stage_delete() {
        use crate::app::sessions::SessionItem;
        use std::path::PathBuf;
        use std::time::SystemTime;
        let mk = |n: &str| SessionItem {
            path: PathBuf::from(format!("/s/{n}.jsonl")),
            name: Some(n.to_string()),
            first_message: String::new(),
            modified: Some(SystemTime::now()),
            cwd: String::new(),
        };
        let mut o = Overlay::SessionSelect {
            items: vec![mk("one"), mk("two"), mk("three")],
            cursor: 0,
            loading: false,
            seq: 1,
            confirming: false,
        };
        // 导航
        assert_eq!(overlay_key(&mut o, key(KeyCode::Down)), OverlayOutcome::Handled);
        assert_eq!(overlay_key(&mut o, key(KeyCode::Down)), OverlayOutcome::Handled);
        assert_eq!(overlay_key(&mut o, key(KeyCode::Down)), OverlayOutcome::Handled); // 底部 clamp
        // Enter → Apply(2)
        assert_eq!(overlay_key(&mut o, key(KeyCode::Enter)), OverlayOutcome::Apply(2));
        // ctrl+d 第一次 → 确认态（Handled，不关窗）
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(overlay_key(&mut o, ctrl_d), OverlayOutcome::Handled);
        let confirming = matches!(&o, Overlay::SessionSelect { confirming: true, .. });
        assert!(confirming, "进入确认态");
        // Esc 先退确认态
        assert_eq!(overlay_key(&mut o, key(KeyCode::Esc)), OverlayOutcome::Handled);
        let confirming = matches!(&o, Overlay::SessionSelect { confirming: true, .. });
        assert!(!confirming, "Esc 退出确认态");
        // 再次 ctrl+d 两段 → 确认态 → Delete
        overlay_key(&mut o, ctrl_d);
        assert_eq!(overlay_key(&mut o, ctrl_d), OverlayOutcome::Delete(2));
        // 确认态下 Enter 不切换（防误触）
        overlay_key(&mut o, ctrl_d); // 重新进确认态？Delete 后 confirming 仍在？——Delete 分支不改状态，
        // 确认态下 Enter：
        let mut o2 = Overlay::SessionSelect {
            items: vec![mk("a")],
            cursor: 0,
            loading: false,
            seq: 1,
            confirming: true,
        };
        assert_eq!(overlay_key(&mut o2, key(KeyCode::Enter)), OverlayOutcome::Handled);
        // loading 态全部按键忽略
        let mut o3 = Overlay::SessionSelect {
            items: vec![],
            cursor: 0,
            loading: true,
            seq: 1,
            confirming: false,
        };
        assert_eq!(overlay_key(&mut o3, key(KeyCode::Enter)), OverlayOutcome::Handled, "loading 忽略导航/确认");
        assert_eq!(
            overlay_key(&mut o3, key(KeyCode::Esc)),
            OverlayOutcome::Close { reply: None },
            "loading 中 Esc 可退出等待"
        );
        // 正常 Esc 关窗
        let mut o4 = Overlay::SessionSelect {
            items: vec![mk("a")],
            cursor: 0,
            loading: false,
            seq: 1,
            confirming: false,
        };
        assert_eq!(overlay_key(&mut o4, key(KeyCode::Esc)), OverlayOutcome::Close { reply: None });
    }

    #[test]
    fn tree_keys_nav_filter_fold_and_fork() {
        use crate::app::tree::FilterMode;
        use std::collections::HashSet;
        // 复用 tree 测试的小树
        let tree: Vec<serde_json::Value> = vec![serde_json::json!({
            "entry": {"type":"message","id":"root","parentId":null,
                "message":{"role":"user","content":[{"type":"text","text":"q"}]}},
            "children": [
                {"entry":{"type":"message","id":"a1","parentId":"root",
                    "message":{"role":"assistant","content":[{"type":"text","text":"a"}]}},
                 "children":[]},
                {"entry":{"type":"message","id":"b1","parentId":"root",
                    "message":{"role":"user","content":[{"type":"text","text":"b"}]}},
                 "children":[]}
            ]
        })];
        let mk = |filter: FilterMode| Overlay::Tree {
            tree: tree.clone(),
            leaf_id: Some("b1".to_string()),
            rows: crate::app::tree::flatten_tree(&tree, Some("b1"), &HashSet::new(), filter),
            cursor: 0,
            folded: HashSet::new(),
            filter,
            loading: false,
            seq: 1,
        };
        let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;

        let mut o = mk(FilterMode::Default);
        // 导航
        assert_eq!(overlay_key(&mut o, key(KeyCode::Down)), OverlayOutcome::Handled);
        match &o { Overlay::Tree { cursor, .. } => assert_eq!(*cursor, 1), _ => unreachable!() }
        // Enter → Apply（fork）
        assert_eq!(overlay_key(&mut o, key(KeyCode::Enter)), OverlayOutcome::Apply(1));
        // ctrl+o 循环过滤
        overlay_key(&mut o, ctrl_o);
        match &o { Overlay::Tree { filter, .. } => assert_eq!(*filter, FilterMode::NoTools), _ => unreachable!() }
        overlay_key(&mut o, ctrl_o);
        overlay_key(&mut o, ctrl_o);
        overlay_key(&mut o, ctrl_o);
        overlay_key(&mut o, ctrl_o); // 转一圈回 default
        match &o { Overlay::Tree { filter, .. } => assert_eq!(*filter, FilterMode::Default), _ => unreachable!() }
        // ← 折叠 root（cursor 回 0；root 有 children，本测试树的 a1/b1 是叶子）
        overlay_key(&mut o, key(KeyCode::Up)); // cursor 1→0
        overlay_key(&mut o, key(KeyCode::Left));
        match &o { Overlay::Tree { rows, folded, cursor, .. } => {
            assert!(folded.contains("root"), "root 被折叠");
            assert_eq!(*cursor, 0);
            let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
            assert_eq!(ids, vec!["root"], "子树被跳过: {ids:?}");
        }, _ => unreachable!() }
        // → 展开恢复
        overlay_key(&mut o, key(KeyCode::Right));
        match &o { Overlay::Tree { rows, .. } => {
            let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
            assert_eq!(ids, vec!["root", "a1", "b1"], "展开恢复: {ids:?}");
        }, _ => unreachable!() }
        // Esc 关闭
        assert_eq!(overlay_key(&mut o, key(KeyCode::Esc)), OverlayOutcome::Close { reply: None });
        // loading 忽略所有键
        let mut ld = mk(FilterMode::Default);
        match &mut ld { Overlay::Tree { loading, .. } => *loading = true, _ => unreachable!() }
        assert_eq!(overlay_key(&mut ld, key(KeyCode::Enter)), OverlayOutcome::Handled);

        // alt 键不干扰（未绑定 → Handled）
        let _ = alt; let _ = ctrl;
    }

    #[test]
    fn info_overlay_closes_on_esc_or_enter() {
        let mut o = Overlay::Info {
            title: " Session Info ".into(),
            lines: vec!["名称: x".into()],
            loading: false,
            seq: 1,
        };
        assert_eq!(overlay_key(&mut o, key(KeyCode::Esc)), OverlayOutcome::Close { reply: None });
        let mut o2 = Overlay::Info {
            title: "t".into(),
            lines: vec![],
            loading: false,
            seq: 1,
        };
        assert_eq!(overlay_key(&mut o2, key(KeyCode::Enter)), OverlayOutcome::Close { reply: None });
        // loading 也响应 Esc（用户应能退出等待）
        let mut o3 = Overlay::Info { title: "t".into(), lines: vec![], loading: true, seq: 1 };
        assert_eq!(overlay_key(&mut o3, key(KeyCode::Esc)), OverlayOutcome::Close { reply: None });
    }
}
