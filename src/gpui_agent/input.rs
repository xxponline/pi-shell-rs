//! 输入翻译层（design D5）：gpui 键盘/滚轮/鼠标 → 现有输入语义载荷。
//!
//! 载荷 = `crossterm::event::KeyEvent`（`UiEvent::Key` 载荷）/ 有符号行数
//! （`UiEvent::Wheel` 载荷）/ 0-based cell 坐标（`UiEvent::Click` 载荷）——
//! 组件接线时包成 `UiEvent`，编辑器/滚动/补全模块**零改动**复用。
//!
//! 与宿主 AgentTerminal 的翻译差异（spec: GPUIAgentTerminal 要与当前功能
//! 语义对齐）：宿主对齐"终端转发"语义——Ctrl/Cmd 组合直接不转发；本组件
//! 对齐 **ratatui 编辑器**语义——ctrl+c 清空、alt+enter follow_up、
//! ctrl+w 删词、alt+←→ 词级移动全靠修饰键组合，故修饰键必须完整保留。
//! 特殊键映射表的"思路"抄宿主 `keystroke_to_keyevents`（key_char 优先 +
//! 单字符兜底），并补齐 ratatui 侧需要的 escape/home/end/delete/F 键。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use gpui_kit::{Keystroke, Modifiers, Pixels, ScrollDelta};

/// gpui 修饰键 → crossterm 位标志。
///
/// 输入: m — gpui `Modifiers`（control/alt/shift/platform/function）；
/// 输出: `KeyModifiers` 位集；`platform` 映射为 `SUPER`（crossterm 无
///       CMD/Cmd 概念，SUPER 是最近语义）；`function`（Fn 键状态）无
///       对应位，忽略——F1-F12 走 key 名映射本身。
pub fn modifiers_to_crossterm(m: &Modifiers) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if m.control {
        out |= KeyModifiers::CONTROL;
    }
    if m.alt {
        out |= KeyModifiers::ALT;
    }
    if m.shift {
        out |= KeyModifiers::SHIFT;
    }
    if m.platform {
        out |= KeyModifiers::SUPER;
    }
    out
}

/// 特殊键名 → crossterm `KeyCode`（ratatui App 所需全集）。
///
/// 输入: key — gpui 键名（`Keystroke::key`）；
/// 输出: `Some(code)` 或 `None`（非特殊键，走可打印路径）。
fn special_key(key: &str) -> Option<KeyCode> {
    match key {
        "enter" | "return" => Some(KeyCode::Enter),
        "escape" => Some(KeyCode::Esc),
        "backspace" => Some(KeyCode::Backspace),
        "delete" => Some(KeyCode::Delete),
        "tab" => Some(KeyCode::Tab),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        // F1-F12（ratatui 侧 key_intent 消费 F(n)）
        "f1" => Some(KeyCode::F(1)),
        "f2" => Some(KeyCode::F(2)),
        "f3" => Some(KeyCode::F(3)),
        "f4" => Some(KeyCode::F(4)),
        "f5" => Some(KeyCode::F(5)),
        "f6" => Some(KeyCode::F(6)),
        "f7" => Some(KeyCode::F(7)),
        "f8" => Some(KeyCode::F(8)),
        "f9" => Some(KeyCode::F(9)),
        "f10" => Some(KeyCode::F(10)),
        "f11" => Some(KeyCode::F(11)),
        "f12" => Some(KeyCode::F(12)),
        _ => None,
    }
}

/// gpui 键击 → crossterm 键事件（通常 0/1 个；`key_char` 多字符时逐字）。
///
/// 映射思路抄宿主 `keystroke_to_keyevents`：特殊键表 → key_char 优先
/// （平台合成的实际字符，如 shift+a → "A"）→ space → 单字符兜底；
/// 与宿主的差异是**保留 ctrl/alt 修饰键**（见模块 doc）与 shift+tab →
/// `BackTab` 特判（ratatui `key_intent` 以 `BackTab` 识别 thinking 循环）；
/// **Cmd/Win（platform）组合一丢弃**——ratatui 无 cmd 键位语义，放行会
/// 退化成插入字符（cmd+c → 插 'c'），与宿主 "Cmd 组合不转发" 一致。
///
/// 输入: ks — gpui 键击（键名 + 修饰键 + 可选合成字符）；
/// 输出: 等价 crossterm `KeyEvent` 列表（无对应键返回空 vec）。
pub fn keystroke_to_keyevents(ks: &Keystroke) -> Vec<KeyEvent> {
    // Cmd/Win 组合一律不进 ratatui 语义域（见函数 doc）。
    if ks.modifiers.platform {
        return vec![];
    }
    let mods = modifiers_to_crossterm(&ks.modifiers);
    // shift+tab：gpui 给 key="tab"+shift；ratatui 以 BackTab 识别（spec:
    // 双击 Esc 之外的 thinking 循环键）。
    if ks.key == "tab" && ks.modifiers.shift {
        return vec![KeyEvent::new(KeyCode::BackTab, mods)];
    }
    if let Some(code) = special_key(&ks.key) {
        return vec![KeyEvent::new(code, mods)];
    }
    // 可打印字符：优先平台给出的合成字符（"A"/"ß"/"…"）。
    if let Some(text) = ks.key_char.as_deref()
        && !text.is_empty()
    {
        return text
            .chars()
            .map(|ch| KeyEvent::new(KeyCode::Char(ch), mods))
            .collect();
    }
    if ks.key == "space" {
        return vec![KeyEvent::new(KeyCode::Char(' '), mods)];
    }
    if ks.key.chars().count() == 1
        && let Some(ch) = ks.key.chars().next()
    {
        return vec![KeyEvent::new(KeyCode::Char(ch), mods)];
    }
    vec![]
}

/// IME commit 文本 → 逐字符键事件（对应 embed `inject_text`：无修饰符）。
///
/// 输入: text — IME 提交的整段文本（可含多字符/换行）；
/// 输出: 每字符一个 `Char` 键事件序列。
pub fn text_to_keyevents(text: &str) -> Vec<KeyEvent> {
    text.chars()
        .map(|ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
        .collect()
}

/// 滚轮像素/行累积器：把连续 delta 凑成整行再发（余数跨事件携带）。
///
/// 语义抄宿主 `WheelAccumulator`：`line_h` 变化（字号/度量变更）时余数
/// 作废；`line_h` 未测量（0）拒绝一切输入（防御）。
#[derive(Debug, Default, Clone, Copy)]
pub struct WheelAccum {
    /// 未凑成整行的余数（行单位，与整数部分同号）。
    remainder: f32,
    /// 当前行高（px）；0 = 尚未测量。
    line_h: f32,
}

impl WheelAccum {
    /// 同步行高；变化即清零余数（spec: Remainder resets on line height change）。
    ///
    /// 输入: line_h — 新行高（px）；
    /// 输出: 无返回值；副作用为行高更新 + 余数清零（若变化）。
    fn set_line_h(&mut self, line_h: f32) {
        if self.line_h != line_h {
            self.line_h = line_h;
            self.remainder = 0.0;
        }
    }

    /// 累积一次输入（行单位），返回本次可发送的整数行（向零取整）。
    ///
    /// 输入: delta_lines — 本次增量（行，已翻号）；
    /// 输出: 整数行（0 = 未凑满；非有限值/行高非法返回 0）。
    fn add(&mut self, delta_lines: f32) -> i16 {
        if !delta_lines.is_finite() || !self.line_h.is_finite() || self.line_h <= 0.0 {
            return 0;
        }
        self.remainder += delta_lines;
        let whole = self.remainder.trunc();
        self.remainder -= whole;
        whole.clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }
}

/// 滚轮事件 → 本次应发送的行数（`None` = 未凑满整行，余数已携带）。
///
/// 语义抄宿主 `wheel_lines_for`（spec: 像素累积 / Lines 直通 / Alt 倍率）：
/// gpui ScrollDelta 的 y 符号与 pi 行数约定**相反**——金标准是 pi standalone
/// 的 crossterm 路径（ScrollUp → -1 = 上滚看更早），此处把 gpui delta 翻转
/// 成同一约定；Alt 持有时整行数 × [`crate::app::scroll::ALT_WHEEL_MULTIPLIER`]。
///
/// 输入: acc — 累积器（跨事件携带余数）；delta — gpui 滚动增量；
///       line_h — 当前行高（px，0 = 拒绝）；alt — Alt 是否按住；
/// 输出: `Some(行数)`（负=上滚看更早）或 `None`（未凑满）。
pub fn wheel_lines(
    acc: &mut WheelAccum,
    delta: &ScrollDelta,
    line_h: f32,
    alt: bool,
) -> Option<i16> {
    acc.set_line_h(line_h);
    let delta_lines = match delta {
        ScrollDelta::Pixels(p) => -f32::from(p.y) / line_h,
        ScrollDelta::Lines(l) => -l.y,
    };
    let n = acc.add(delta_lines);
    if n == 0 {
        return None;
    }
    Some(if alt {
        n.saturating_mul(crate::app::scroll::ALT_WHEEL_MULTIPLIER as i16)
    } else {
        n
    })
}

/// 点击像素坐标 → 网格 0-based cell 坐标（`UiEvent::Click` 载荷）。
///
/// 输入: x/y — 点击位置（px，窗口坐标）；origin — 网格左上角（px）；
///       cell_w/cell_h — 单元格尺寸（px，非正返回 (0,0) 防御）；
/// 输出: `(col, row)`，负坐标截断为 0（越界判定归渲染面——spec:
///       Clicks outside the grid area 由组件按网格 bounds 拒绝）。
pub fn cell_coords(x: f32, y: f32, origin: (f32, f32), cell_w: f32, cell_h: f32) -> (u16, u16) {
    let col = if cell_w > 0. { (x - origin.0) / cell_w } else { 0. };
    let row = if cell_h > 0. { (y - origin.1) / cell_h } else { 0. };
    (
        col.floor().clamp(0., u16::MAX as f32) as u16,
        row.floor().clamp(0., u16::MAX as f32) as u16,
    )
}

/// 便捷重载：从 gpui `Pixels` 坐标换算 cell 坐标。
///
/// 输入: x/y — 点击位置（`Pixels`）；origin — 网格左上角（`Pixels` 元组）；
///       cell — 单元格尺寸（`Pixels` 元组）；
/// 输出: `(col, row)`（同 [`cell_coords`]）。
pub fn cell_coords_px(
    x: Pixels,
    y: Pixels,
    origin: (Pixels, Pixels),
    cell: (Pixels, Pixels),
) -> (u16, u16) {
    cell_coords(
        f32::from(x),
        f32::from(y),
        (f32::from(origin.0), f32::from(origin.1)),
        f32::from(cell.0),
        f32::from(cell.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// 构造 gpui 键击（显式字段，不依赖 parse 的内部约定）。
    fn ks(key: &str, key_char: Option<&str>, m: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: m,
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
        }
    }

    fn mods(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        }
    }

    /// 特殊键映射全集（对照 `event.rs` 既有测试口径：构造输入 → 断言 KeyEvent）。
    ///
    /// 功能: 锁定特殊键表；输入: 无；输出: 断言通过。
    #[test]
    fn special_keys_map_to_crossterm_codes() {
        for (gpui_key, expected) in [
            ("enter", KeyCode::Enter),
            ("return", KeyCode::Enter),
            ("escape", KeyCode::Esc),
            ("backspace", KeyCode::Backspace),
            ("delete", KeyCode::Delete),
            ("tab", KeyCode::Tab),
            ("up", KeyCode::Up),
            ("down", KeyCode::Down),
            ("left", KeyCode::Left),
            ("right", KeyCode::Right),
            ("home", KeyCode::Home),
            ("end", KeyCode::End),
            ("pageup", KeyCode::PageUp),
            ("pagedown", KeyCode::PageDown),
            ("f5", KeyCode::F(5)),
            ("f12", KeyCode::F(12)),
        ] {
            assert_eq!(
                keystroke_to_keyevents(&ks(gpui_key, None, Modifiers::default())),
                vec![KeyEvent::new(expected, KeyModifiers::NONE)],
                "特殊键 {gpui_key} 映射"
            );
        }
    }

    /// 修饰键组合完整保留（spec: 与 ratatui 编辑器语义对齐——与宿主
    /// "Ctrl 不转发"语义的差异点）。
    ///
    /// 功能: 锁定 ctrl/alt/shift → crossterm 位标志、platform 组合丢弃；
    /// 输入: 构造组合键击；输出: 断言通过（ctrl+c 清空 / alt+enter
    ///       follow_up / alt+← 词移 / cmd 组合丢弃 / shift+tab → BackTab
    ///       / ctrl+alt 叠加 五类代表）。
    #[test]
    fn modifier_combines_are_preserved() {
        // ctrl+c → Char('c') + CONTROL（ratatui：清空输入）
        assert_eq!(
            keystroke_to_keyevents(&ks("c", None, mods(true, false, false, false))),
            vec![KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)]
        );
        // alt+enter → Enter + ALT（ratatui：follow_up 队列）
        assert_eq!(
            keystroke_to_keyevents(&ks("enter", None, mods(false, true, false, false))),
            vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)]
        );
        // alt+left → Left + ALT（ratatui：词级移动）
        assert_eq!(
            keystroke_to_keyevents(&ks("left", None, mods(false, true, false, false))),
            vec![KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)]
        );
        // platform（Cmd/Win）→ 丢弃：ratatui 无 cmd 语义，放行会误插字符
        assert_eq!(
            keystroke_to_keyevents(&ks("s", Some("s"), mods(false, false, false, true))),
            vec![]
        );
        // 但位映射本身仍保留（platform → SUPER，crossterm 最近语义）
        assert_eq!(
            modifiers_to_crossterm(&mods(false, false, false, true)),
            KeyModifiers::SUPER
        );
        // shift+tab → BackTab + SHIFT（ratatui：thinking 循环）
        assert_eq!(
            keystroke_to_keyevents(&ks("tab", None, mods(false, false, true, false))),
            vec![KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)]
        );
        // 组合叠加：ctrl+alt+x → 两位或
        assert_eq!(
            keystroke_to_keyevents(&ks("x", None, mods(true, true, false, false))),
            vec![KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )]
        );
    }

    /// key_char 优先 + space/单字符兜底（宿主映射思路）。
    ///
    /// 功能: 锁定可打印字符三条路径；输入: 构造键击；输出: 断言通过。
    #[test]
    fn printable_chars_prefer_key_char_then_fallback() {
        // key_char 优先：shift+a 平台合成 "A"（带 SHIFT）
        assert_eq!(
            keystroke_to_keyevents(&ks("a", Some("A"), mods(false, false, true, false))),
            vec![KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)]
        );
        // key_char 多字符逐字（IME 合成串）
        assert_eq!(
            keystroke_to_keyevents(&ks("ab", Some("ab"), Modifiers::default())),
            vec![
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)
            ]
        );
        // space → Char(' ')
        assert_eq!(
            keystroke_to_keyevents(&ks("space", None, Modifiers::default())),
            vec![KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)]
        );
        // 单字符 key 兜底（key_char 缺失）
        assert_eq!(
            keystroke_to_keyevents(&ks("q", None, Modifiers::default())),
            vec![KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)]
        );
        // 无对应 → 空
        assert!(keystroke_to_keyevents(&ks("deadend", None, Modifiers::default())).is_empty());
    }

    /// IME commit 逐字符注入（对应 embed `inject_text` 语义）。
    ///
    /// 功能: 锁定 IME 文本 → 键事件；输入: 多字符文本；输出: 断言通过。
    #[test]
    fn ime_commit_text_maps_char_by_char() {
        assert_eq!(
            text_to_keyevents("你好"),
            vec![
                KeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('好'), KeyModifiers::NONE)
            ]
        );
        assert!(text_to_keyevents("").is_empty());
    }

    /// 滚轮换算（抄宿主 `wheel_lines_for` 用例：像素累积 / 符号翻转 /
    /// Line 直通 / Alt 倍率 / 行高变化清零 / 未测量拒绝）。
    ///
    /// 功能: 锁定 spec「像素累积 / Line 直通 / Alt 倍率」三条语义；
    /// 输入: 构造 ScrollDelta 序列；输出: 断言通过（None = 未凑满）。
    #[test]
    fn wheel_accumulates_pixels_and_applies_alt_multiplier() {
        use gpui_kit::{Point, px, point};

        let pix = |y: f32| ScrollDelta::Pixels(point(px(0.), px(y)));
        let lines = |y: f32| ScrollDelta::Lines(Point { x: 0.0, y });

        let mut acc = WheelAccum::default();
        // 行高未测量（0）→ 拒绝一切输入（防御）
        assert_eq!(wheel_lines(&mut acc, &pix(42.), 0.0, false), None);

        // 像素上滑 0.4 行/次（gpui +y → 翻号为 -0.4）：前两次 None（余数携带），
        // 第三次 -1.2 → Some(-1)（负 = 上滚看更早，与 crossterm ScrollUp 同约定）
        assert_eq!(wheel_lines(&mut acc, &pix(8.4), 21.0, false), None);
        assert_eq!(wheel_lines(&mut acc, &pix(8.4), 21.0, false), None);
        assert_eq!(wheel_lines(&mut acc, &pix(8.4), 21.0, false), Some(-1));

        // Alt 倍率：一行 gpui → -1 → ×5 保号
        assert_eq!(
            wheel_lines(&mut acc, &pix(21.0), 21.0, true),
            Some(-(crate::app::scroll::ALT_WHEEL_MULTIPLIER as i16))
        );

        // Lines 直通：+0.3 → -0.3；余数继续携带
        assert_eq!(wheel_lines(&mut acc, &lines(0.3), 21.0, false), None);

        // 行高变化 → 余数清零（spec: Remainder resets on line height change）
        assert_eq!(wheel_lines(&mut acc, &lines(0.2), 24.0, false), None);
        assert_eq!(
            wheel_lines(&mut acc, &lines(0.7), 24.0, false),
            None,
            "换行高后余数从零起算 (-0.7 单独未凑满)"
        );
        assert_eq!(wheel_lines(&mut acc, &lines(0.5), 24.0, false), Some(-1));

        // 大额下滚：gpui +3 行 → 翻号 -3（正 = 下滚看更新对应 gpui 负 y）
        assert_eq!(wheel_lines(&mut acc, &lines(3.0), 24.0, false), Some(-3));
        // 反向 +3：前步余数 -0.4 携带 → 3-0.4=2.6 → 截 2（余数跨事件携带）
        assert_eq!(wheel_lines(&mut acc, &lines(-3.0), 24.0, false), Some(2));
    }

    /// 点击像素 → cell 坐标（floor 换算 + 负值截断 + 防御）。
    ///
    /// 功能: 锁定 `cell_coords` 换算口径；输入: 构造坐标；输出: 断言通过。
    #[test]
    fn cell_coords_floor_and_clamp() {
        // (17, 33) ÷ (8, 16) 原点 (1, 1) → col=(17-1)/8=2, row=(33-1)/16=2
        assert_eq!(cell_coords(17., 33., (1., 1.), 8., 16.), (2, 2));
        // 边界内最大：刚好不到下一格 → 不进位
        assert_eq!(cell_coords(16.9, 31.9, (1., 1.), 8., 16.), (1, 1));
        // 负坐标（网格左上外）→ 截断 0
        assert_eq!(cell_coords(-5., -5., (1., 1.), 8., 16.), (0, 0));
        // cell 尺寸非法 → (0,0) 防御
        assert_eq!(cell_coords(100., 100., (0., 0.), 0., 0.), (0, 0));
        // 超大值 → u16 饱和不 panic
        assert_eq!(cell_coords(f32::MAX, f32::MAX, (0., 0.), 8., 16.), (u16::MAX, u16::MAX));
    }

    /// Pixels 便捷重载与 f32 版一致。
    ///
    /// 功能: 锁定 `cell_coords_px` 委托正确；输入: 构造 Pixels 坐标；
    /// 输出: 断言通过。
    #[test]
    fn cell_coords_px_matches_f32_version() {
        use gpui_kit::px;
        assert_eq!(
            cell_coords_px(px(17.), px(33.), (px(1.), px(1.)), (px(8.), px(16.))),
            cell_coords(17., 33., (1., 1.), 8., 16.)
        );
    }
}
