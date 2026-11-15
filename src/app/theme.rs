//! 可注入配色结构 [`TuiTheme`]（宿主配色注入 design D1/D5）。
//!
//! 全仓 ~140 处颜色中只有本结构收录的 7 处是 RGB 直通、主题相关
//! （工具卡三态面板、滚动条 track/thumb、syntect 主题名）；其余命名
//! ANSI 色走 0-15 索引、由宿主渲染调色板解释，天生主题无关，不入本结构。
//!
//! 主题状态的唯一真相在宿主：本仓不读系统外观、不内置浅色知识，只接受
//! 注入（"宿主说啥我画啥"）。`Default` = pi dark.json 深底现值——
//! 独立 TUI 与未注入的 embed 路径输出与注入前逐字节一致。

use ratatui::style::Color;

/// 宿主可注入的完整配色（完整值结构体，非覆盖补丁）。
///
/// 字段语义见各字段注释；默认值即 pi dark.json 深底现值。
/// 推论：未来加颜色旋钮 = 结构体加字段 + 默认值，单仓扩 API，
/// 两仓（宿主 GUI）不需同步改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiTheme {
    /// 工具卡运行中（Pending/Running）面板背景色；默认运行灰。
    pub tool_running_bg: Color,
    /// 工具卡失败（Ended + is_error）面板背景色；默认失败暗红。
    pub tool_error_bg: Color,
    /// 工具卡成功（Ended）面板背景色；默认成功墨绿。
    pub tool_success_bg: Color,
    /// 消息区滚动条轨道（`│`）前景色。
    pub scrollbar_track: Color,
    /// 消息区滚动条拖动条（`┃`/`█`）前景色。
    pub scrollbar_thumb: Color,
    /// syntect 语法高亮主题名（内置主题集内的键，如
    /// `"base16-ocean.dark"`）；无效名回退默认主题、不 panic。
    pub syntax_theme: String,
}

impl Default for TuiTheme {
    /// 默认配色 = pi dark.json 深底现值（design D2：默认路径字节级不变）。
    ///
    /// 输入: 无
    /// 输出: 工具卡三态为 okhsl 转 RGB 现值、滚动条引用 wrap 模块现有
    ///       const（默认值的唯一定义处）、语法主题为 `"base16-ocean.dark"`
    fn default() -> Self {
        Self {
            tool_running_bg: Color::Rgb(52, 56, 58),
            tool_error_bg: Color::Rgb(91, 40, 42),
            tool_success_bg: Color::Rgb(37, 65, 49),
            scrollbar_track: crate::app::wrap::SCROLLBAR_TRACK_COLOR,
            scrollbar_thumb: crate::app::wrap::SCROLLBAR_THUMB_COLOR,
            syntax_theme: "base16-ocean.dark".to_string(),
        }
    }
}
