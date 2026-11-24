//! GPUI 浮层渲染（spec: 悬浮层渲染；design D2）。
//!
//! 状态与按键 100% 在共享层（`App.overlay` / `overlay_key`），本模块只读
//! 状态直绘：全窗遮罩 + 居中面板，样式取主题 token（独立窗口固定 Light）。
//! 逐变体内容按 TUI `app/overlay.rs::render_overlay` 行文对照填充；
//! 标题后缀、loading/空态、光标行高亮均与 TUI 同源。

use std::path::Path;

use gpui_kit::base::TestTextExt;
use gpui_kit::component::theme::ActiveTheme;
use gpui_kit::*;

use crate::app::overlay::{Overlay, hotkeys_rows, truncate_text};
use crate::app::App as AppState;

/// 面板宽度占窗比（对齐 TUI `draw_list` 的 70%）。
const PANEL_WIDTH_PCT: f32 = 70.0;

/// 面板高度上限占窗比（长内容不溢出窗口）。
const PANEL_MAX_H_PCT: f32 = 0.9;

/// 列表浮层标题的导航提示（对齐 TUI `draw_list` 标题后缀）。
const NAV_HINT: &str = "(↑/↓ 导航 · Enter 选择 · Esc 取消)";

/// 百分比居中盒（GPUI 版 `centered_rect`，纯函数可单测）。
///
/// 功能: 在给定宽高中按百分比取居中块；
/// 输入: percent_x/percent_y — 1-100（越界夹取到 1..=100）；
///       width/height — 父区像素；
/// 输出: `(left, top, box_w, box_h)`；width/height 为 0 时盒退化为 0。
pub(crate) fn centered_box(
    percent_x: f32,
    percent_y: f32,
    width: f32,
    height: f32,
) -> (f32, f32, f32, f32) {
    let w = width * percent_x.clamp(1.0, 100.0) / 100.0;
    let h = height * percent_y.clamp(1.0, 100.0) / 100.0;
    ((width - w) / 2.0, (height - h) / 2.0, w, h)
}

/// 面板内一行文本（带观测锚点，供无头快照断言）。
///
/// 功能: 组一行带内边距的文本行；
/// 输入: text — 行文本；color — 文字色（主题 token）；
/// 输出: Div 行元素（test_text 记录原文）。
fn row(text: impl Into<String>, color: Hsla) -> Div {
    let text = text.into();
    div()
        .px_3()
        .py_1()
        .text_sm()
        .text_color(color)
        .child(div().child(text.clone()).test_text(text))
}

/// loading 行（弱化）。
///
/// 功能: 组 "loading…" 行；输入: cx — App（主题 token）；输出: Div 行。
fn loading_row(cx: &App) -> Div {
    row("loading…", cx.theme().muted_foreground)
}

/// 弱化说明行。
///
/// 功能: 组 muted 色说明行；输入: text — 说明文本；cx — App；输出: Div 行。
fn note_row(text: impl Into<String>, cx: &App) -> Div {
    row(text, cx.theme().muted_foreground)
}

/// 可选行：前缀 + 文本（选中行加 list_active 底）。
///
/// 功能: 组带光标高亮的选项行；
/// 输入: label — 完整行文本（含 "> "/"  " 前缀）；selected — 是否选中；
///       cx — App（主题 token）；
/// 输出: Div 行（test_text 记录 label）。
fn opt_row(label: String, selected: bool, cx: &App) -> Div {
    let mut r = div().px_3().py_1().text_sm().flex().items_center();
    if selected {
        r = r.bg(cx.theme().list_active);
    }
    r.child(div().child(label.clone()).test_text(label))
}

/// 遮罩 + 居中面板 chrome。
///
/// 功能: 全窗遮罩（`theme.overlay` 色）+ flex 居中的面板（标题行 + 内容行）；
/// 输入: title — 标题；rows — 面板内容行；size — 整窗像素；
///       cx — App（主题 token 来源）；
/// 输出: 覆盖层 Div（绝对定位铺满父容器）。
fn chrome(title: String, rows: Vec<Div>, size: Size<Pixels>, cx: &App) -> Div {
    let vw = f32::from(size.width);
    let vh = f32::from(size.height);
    // 宽度取百分比居中盒的宽（高度由 flex 居中 + 内容自适应，仅设上限）。
    let (_, _, panel_w, _) = centered_box(PANEL_WIDTH_PCT, 100.0, vw, vh);
    let panel_max_h = vh * PANEL_MAX_H_PCT;
    div()
        .absolute()
        .inset_0()
        .occlude() // 阻断后方鼠标与滚轮命中（spec: 浮层下滚轮忽略）
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().overlay)
        .child(
            div()
                .flex()
                .flex_col()
                .w(px(panel_w))
                .max_h(px(panel_max_h))
                .overflow_hidden()
                .bg(cx.theme().popover)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(title.clone())
                        .test_text(title),
                )
                .children(rows),
        )
}

/// 浮层元素入口：overlay 打开时返回覆盖层，否则 None。
///
/// 功能: 读 `app.overlay` 直绘（只读，不改共享状态）；
/// 输入: app — 共享状态；size — 整窗像素；cx — App（主题 token）；
/// 输出: Some(覆盖元素)；浮层未打开时 None。
pub(crate) fn overlay_layer(app: &AppState, size: Size<Pixels>, cx: &App) -> Option<AnyElement> {
    let overlay = app.overlay.as_ref()?;
    let (title, rows) = describe(overlay, app, cx);
    Some(chrome(title, rows, size, cx).into_any_element())
}

/// 变体 → (标题, 内容行)，行文对照 TUI `render_overlay`。
///
/// 功能: 把共享浮层状态翻译成面板内容（14 变体全量）；
/// 输入: overlay — 打开中的浮层；app — 共享状态（当前会话标记等）；
///       cx — App（主题 token）；
/// 输出: (标题, 行)。
fn describe(overlay: &Overlay, app: &AppState, cx: &App) -> (String, Vec<Div>) {
    match overlay {
        Overlay::ModelSelect {
            items,
            loading,
            cursor,
            ..
        } => {
            let title = format!(" Model (ctrl+l) {NAV_HINT} ");
            let rows = if *loading {
                vec![loading_row(cx)]
            } else if items.is_empty() {
                vec![note_row("(no models)", cx)]
            } else {
                items
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let label = format!(
                            "{}{}  ({})",
                            if i == *cursor { "> " } else { "  " },
                            m.name,
                            m.provider
                        );
                        opt_row(label, i == *cursor, cx)
                    })
                    .collect()
            };
            (title, rows)
        }
        Overlay::ThinkingSelect {
            levels,
            loading,
            cursor,
            ..
        } => {
            let title = format!(" Thinking level (alt+t) {NAV_HINT} ");
            let rows = if *loading {
                vec![loading_row(cx)]
            } else {
                levels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let label = format!("{}{l}", if i == *cursor { "> " } else { "  " });
                        opt_row(label, i == *cursor, cx)
                    })
                    .collect()
            };
            (title, rows)
        }
        Overlay::SessionSelect {
            items,
            cursor,
            loading,
            confirming,
            ..
        } => {
            let title = if *confirming {
                format!(" Resume · ⚠ 再按 ctrl+d 确认删除，Esc 取消 {NAV_HINT} ")
            } else {
                format!(" Resume (Enter 切换 · ctrl+d 删除) {NAV_HINT} ")
            };
            let rows = if *loading {
                vec![loading_row(cx)]
            } else if items.is_empty() {
                vec![note_row("(无会话)", cx)]
            } else {
                let now = std::time::SystemTime::now();
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
                        let current = app
                            .session_file
                            .as_deref()
                            .map(|c| Path::new(c) == it.path.as_path())
                            .unwrap_or(false);
                        let prefix = format!(
                            "{}{}",
                            if current { "• " } else { "" },
                            if selected { "> " } else { "  " }
                        );
                        let text = format!(
                            "{prefix}{}  {}",
                            truncate_text(&label, 52),
                            crate::app::sessions::rel_time(it.modified, now)
                        );
                        opt_row(text, selected, cx)
                    })
                    .collect()
            };
            (title, rows)
        }
        Overlay::ScopedModels {
            models,
            loading,
            cursor,
            ..
        } => {
            let title = " Scoped Models（只读） ".to_string();
            let rows = if *loading {
                vec![loading_row(cx)]
            } else if models.is_empty() {
                vec![note_row("(无可用模型)", cx)]
            } else {
                let mut rows = vec![note_row(
                    "当前会话模型见状态行；本列表只读（写入需 pi /settings，本 TUI 未实现）",
                    cx,
                )];
                for (i, m) in models.iter().enumerate() {
                    rows.push(opt_row(format!("  {m}"), i == *cursor, cx));
                }
                rows
            };
            (title, rows)
        }
        Overlay::Info {
            title,
            lines,
            loading,
            ..
        } => {
            let mut rows = if *loading {
                vec![loading_row(cx)]
            } else {
                lines
                    .iter()
                    .map(|l| row(l.clone(), cx.theme().foreground))
                    .collect()
            };
            rows.push(row("", cx.theme().muted_foreground));
            rows.push(row("Esc 关闭", cx.theme().muted_foreground));
            (title.clone(), rows)
        }
        Overlay::Tree {
            rows: tree_rows,
            cursor,
            filter,
            search,
            loading,
            ..
        } => {
            let mut title = format!(" Tree (filter:{}) ", filter.label());
            if !search.is_empty() {
                title.push_str(&format!("· search:\"{search}\" ({} 命中) ", tree_rows.len()));
            }
            title.push_str("Enter=分支 ←=折叠 →=展开 ");
            let rows = if *loading {
                vec![loading_row(cx)]
            } else if tree_rows.is_empty() && !search.is_empty() {
                vec![row(
                    "无匹配 — backspace 逐字恢复，清空后显示全部",
                    cx.theme().warning,
                )]
            } else if tree_rows.is_empty() {
                vec![note_row("(空树)", cx)]
            } else {
                tree_rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let selected = i == *cursor;
                        let indent = "  ".repeat(r.depth);
                        let marker = if r.is_leaf {
                            "● "
                        } else if r.has_children {
                            "▾ "
                        } else {
                            "  "
                        };
                        let mut text = format!(
                            "{}{indent}{marker}{}",
                            if selected { "> " } else { "  " },
                            r.text
                        );
                        if r.is_leaf {
                            text.push_str(" ◀ 当前");
                        }
                        opt_row(text, selected, cx)
                    })
                    .collect()
            };
            (title, rows)
        }
        Overlay::Hotkeys { cursor } => {
            // TUI 按可视高度开窗滚动；GPUI 无固定行高开窗，全量渲染由面板
            // max_h 裁剪（spec 只要求键位表可见，↑↓ 移动高亮即达）。
            let title = " Hotkeys  ↑↓ 滚动 ".to_string();
            let rows = hotkeys_rows()
                .iter()
                .enumerate()
                .map(|(i, (group, key, desc))| match group {
                    Some(g) => row(format!("── {g} ──"), cx.theme().warning),
                    None => opt_row(
                        format!("  {key:<16}{desc}"),
                        i == *cursor,
                        cx,
                    ),
                })
                .collect();
            (title, rows)
        }
        Overlay::Settings {
            cursor,
            loading,
            auto_compaction,
            auto_retry,
            double_esc,
            model,
            ..
        } => {
            let title = " Settings（简化） ".to_string();
            let rows = if *loading {
                vec![loading_row(cx)]
            } else {
                let onoff = |v: &Option<bool>| match v {
                    Some(true) => "on".to_string(),
                    Some(false) => "off".to_string(),
                    None => "—".to_string(),
                };
                vec![
                    opt_row(
                        format!("  autoCompact        {}", onoff(auto_compaction)),
                        *cursor == 0,
                        cx,
                    ),
                    opt_row(
                        format!("  autoRetry          {}", onoff(auto_retry)),
                        *cursor == 1,
                        cx,
                    ),
                    opt_row(
                        format!(
                            "  doubleEsc          {}",
                            double_esc.as_deref().unwrap_or("—")
                        ),
                        *cursor == 2,
                        cx,
                    ),
                    row(
                        format!("  model              {}", model.as_deref().unwrap_or("—")),
                        cx.theme().muted_foreground,
                    ),
                    note_row(
                        "  enter 切换开关 · esc 关闭（简化版，详见 Docs/slash-commands.md）",
                        cx,
                    ),
                ]
            };
            (title, rows)
        }
        Overlay::Trust { cursor, current } => {
            let state = match current {
                Some(true) => "已信任",
                Some(false) => "已拒绝",
                None => "未记录",
            };
            let title = format!(" Trust（当前: {state}） ");
            let options = ["✓ 信任本项目", "× 清除信任记录", "Esc 关闭"];
            let rows = options
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    if i == 2 {
                        note_row(format!("  {opt}"), cx)
                    } else {
                        opt_row(format!("  {opt}"), i == *cursor, cx)
                    }
                })
                .collect();
            (title, rows)
        }
        Overlay::ImportConfirm { path, cursor } => {
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let title = format!(" Import {name}（替换当前会话） ");
            let options = ["✓ 导入并切换", "取消"];
            let rows = options
                .iter()
                .enumerate()
                .map(|(i, opt)| opt_row(format!("  {opt}"), i == *cursor, cx))
                .collect();
            (title, rows)
        }
        Overlay::ExtSelect {
            title,
            options,
            cursor,
            ..
        } => {
            let title = format!(" {title} {NAV_HINT} ");
            let rows = options
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let label = format!("{}{o}", if i == *cursor { "> " } else { "  " });
                    opt_row(label, i == *cursor, cx)
                })
                .collect();
            (title, rows)
        }
        Overlay::ExtConfirm {
            title,
            message,
            confirm_selected,
            ..
        } => {
            let title = format!(" {title} ");
            let btn = |selected: bool, label: &str, cx: &App| -> Div {
                let label = format!(" {label} ");
                let mut b = div().px_2().py_1().text_sm();
                if selected {
                    b = b.bg(cx.theme().list_active);
                }
                b.child(div().child(label.clone()).test_text(label))
            };
            let rows = vec![
                row(message.clone(), cx.theme().foreground),
                row("", cx.theme().muted_foreground),
                div()
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .child(btn(*confirm_selected, "确认", cx))
                    .child(btn(!*confirm_selected, "取消", cx)),
                note_row("←/→/Tab 切换 · Enter 确定 · Esc 取消", cx),
            ];
            (title, rows)
        }
        Overlay::ExtInput {
            title,
            placeholder,
            buffer,
            ..
        } => {
            let title = format!(" {title} (Enter 提交 · Esc 取消) ");
            let rows = if buffer.is_empty() {
                vec![note_row(placeholder.clone(), cx)]
            } else {
                vec![row(buffer.clone(), cx.theme().foreground)]
            };
            (title, rows)
        }
        Overlay::ExtEditor {
            title,
            lines,
            row: edit_row,
            col,
            ..
        } => {
            let title = format!(" {title} (Enter 提交 · Alt+Enter 换行 · Esc 取消) ");
            let rows = lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    if i == *edit_row {
                        // 当前行按字符光标切分，光标处插入高亮块（对照 TUI）。
                        let byte = crate::app::overlay::char_byte_idx(line, *col);
                        let (left, right) = line.split_at(byte);
                        let full = format!("{left}▏{right}");
                        div()
                            .px_3()
                            .py_1()
                            .text_sm()
                            .flex()
                            .child(div().child(left.to_string()).test_text(full))
                            .child(div().bg(cx.theme().list_active).child("▏".to_string()))
                            .child(div().child(right.to_string()))
                    } else {
                        row(line.clone(), cx.theme().foreground)
                    }
                })
                .collect();
            (title, rows)
        }
    }
}

#[cfg(test)]
mod tests {
    // 显式导入（不 glob）：避免 `gpui_kit::*` 遮蔽 `#[test]`（kit lib.rs 注释）。
    use super::centered_box;

    /// 居中盒几何：50%×50% 于 100×40 → (25,10,50,20)。
    ///
    /// 功能: 校验百分比换算与居中；
    /// 输入: 50/50 与100×40；
    /// 输出: 断言通过（四元组精确相等）。
    #[test]
    fn centered_box_is_centered() {
        assert_eq!(
            centered_box(50.0, 50.0, 100.0, 40.0),
            (25.0, 10.0, 50.0, 20.0)
        );
    }

    /// 越界百分比夹取到 1..=100。
    ///
    /// 功能: 校验 clamp 语义；
    /// 输入: 200% / -3% 于 100×40；
    /// 输出: 断言通过（x 满宽贴左、y 取 1%）。
    #[test]
    fn centered_box_clamps_percent() {
        let (l, t, w, h) = centered_box(200.0, -3.0, 100.0, 40.0);
        assert_eq!((l, w), (0.0, 100.0));
        assert!((t - 19.8).abs() < f32::EPSILON);
        assert!((h - 0.4).abs() < f32::EPSILON);
    }
}
