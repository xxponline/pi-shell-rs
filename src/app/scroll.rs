//! 消息区滚动状态（纯逻辑，无终端依赖）。
//!
//! 背景见 `openspec/changes/add-message-scrollback/design.md` D1：采用
//! "换行后绝对行号 + 贴底跟随标志"，与 pi `ScrollView` 的
//! `scrollTop + followingEnd` 同构——绝对行号在内容增长时不动即钉住，
//! 正是规格"上滚后新内容不抢位"所需；"距底偏移量"方案会随增长滑动，已否决。

/// 整页滚动的上下文重叠行数（对齐 pi `PAGE_SCROLL_OVERLAP`：翻页后保留
/// 上下文，避免整屏换血）。
pub const PAGE_SCROLL_OVERLAP: usize = 4;

/// 消息区滚动状态：视口首行（换行后绝对行号）+ 贴底跟随标志。
///
/// 字段：
/// - `top` — 当前视口首行在**换行后**行序列中的绝对行号；`following` 时
///   渲染端每帧重算为 `len - height`，此值仅在钉住时生效（每帧被 absorb
///   同步为实际显示首行）。
/// - `following` — 贴底跟随标志；true 时新内容保持视口贴底，false 时视口
///   钉在原内容上不被新内容抢占。
/// - `viewport` — 上次渲染的消息区高度（页长计算用；渲染端每帧回写），
///   resize 后下一帧自纠，首帧前为 0（页长退化为 1 行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatScroll {
    /// 视口首行（换行后绝对行号）。
    pub top: usize,
    /// 贴底跟随标志。
    pub following: bool,
    /// 上次渲染的消息区高度（页长计算用）。
    pub viewport: u16,
}

impl Default for ChatScroll {
    /// 新会话默认贴底跟随（top=0、viewport 尚未渲染为 0）；
    /// `ViewUi` 的 derive(Default) 依赖此语义。
    fn default() -> Self {
        Self {
            top: 0,
            following: true,
            viewport: 0,
        }
    }
}

impl ChatScroll {
    /// 整页滚动的行数：视口高减去上下文重叠行，至少 1 行。
    ///
    /// 输入: 无（读 self.viewport）
    /// 输出: 页长（行数）；viewport 未渲染（0）或不足重叠行数时退化为 1
    pub fn page(&self) -> usize {
        (self.viewport as usize)
            .saturating_sub(PAGE_SCROLL_OVERLAP)
            .max(1)
    }

    /// 向上翻一页（看更早内容）：打断贴底跟随并饱和上移。
    ///
    /// 输入: 无
    /// 输出: 副作用为 following=false、top 减一页（不低于 0）；
    ///       已在顶部时保持 top=0 但跟随仍被打断
    pub fn page_up(&mut self) {
        self.following = false;
        self.top = self.top.saturating_sub(self.page());
    }

    /// 向下翻一页（看更新内容）：饱和下移。
    ///
    /// 输入: 无
    /// 输出: 副作用为 top 加一页（饱和防溢出）；不改 following——
    ///       是否已到底、是否恢复跟随由渲染端 absorb 判定
    pub fn page_down(&mut self) {
        self.top = self.top.saturating_add(self.page());
    }

    /// 吸收渲染端反馈：回写视口首行并据"是否贴底"恢复/维持跟随。
    ///
    /// 输入:
    ///   effective_top — 渲染端 clamp 后实际使用的视口首行
    ///   at_bottom     — 该首行是否已在内容底部（含内容不足一屏）
    /// 输出: 副作用为 top=effective_top、following=at_bottom；
    ///       贴底即恢复跟随——覆盖规格"滚回底部恢复跟随"与
    ///       "内容骤变 clamp 到底恢复跟随"两条路径
    pub fn absorb(&mut self, effective_top: usize, at_bottom: bool) {
        self.top = effective_top;
        self.following = at_bottom;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 任务 1.1：default 即贴底跟随（新会话/测试构造的默认语义）。
    #[test]
    fn default_follows_bottom() {
        let s = ChatScroll::default();
        assert!(s.following, "默认应贴底跟随");
        assert_eq!(s.top, 0);
        assert_eq!(s.viewport, 0);
    }

    /// 任务 1.2：页长 = viewport − 重叠行，viewport 不足时退化为 1。
    #[test]
    fn page_subtracts_overlap_and_floors_at_one() {
        let mut s = ChatScroll {
            viewport: 20,
            ..Default::default()
        };
        assert_eq!(s.page(), 20 - PAGE_SCROLL_OVERLAP, "页长应减去 4 行重叠");

        s.viewport = 3;
        assert_eq!(s.page(), 1, "视口不足重叠行数时页长至少 1");

        s.viewport = 0;
        assert_eq!(s.page(), 1, "首帧前（viewport=0）页长退化为 1");
    }

    /// 任务 1.2：page_up 打断跟随并饱和上移；已在顶部不越界。
    #[test]
    fn page_up_breaks_follow_and_saturates_at_top() {
        let mut s = ChatScroll {
            top: 10,
            following: true,
            viewport: 20,
        };
        s.page_up();
        assert!(!s.following, "上滚应打断贴底跟随");
        assert_eq!(s.top, 0, "10 < 页长 16，饱和到顶部");

        s.top = 3;
        s.page_up();
        assert_eq!(s.top, 0, "顶部饱和不越界");
        assert!(!s.following);
    }

    /// 任务 1.2：page_down 饱和下移、不改 following（恢复交给 absorb）。
    #[test]
    fn page_down_saturates_and_keeps_following_flag() {
        let mut s = ChatScroll {
            top: 10,
            following: false,
            viewport: 20,
        };
        s.page_down();
        assert_eq!(s.top, 10 + 16, "top 加一页");
        assert!(!s.following, "page_down 本身不改 following");

        s.top = usize::MAX;
        s.page_down();
        assert_eq!(s.top, usize::MAX, "饱和防溢出");
    }

    /// 任务 1.2：absorb 贴底恢复跟随、中段维持钉住。
    #[test]
    fn absorb_restores_follow_at_bottom_and_keeps_pin_midway() {
        let mut s = ChatScroll {
            top: 500,
            following: false,
            viewport: 20,
        };
        // 中段：维持钉住
        s.absorb(120, false);
        assert_eq!(s.top, 120, "回写实际显示首行");
        assert!(!s.following, "中段不恢复跟随");

        // 滚回/骤变 clamp 到底：恢复跟随
        s.absorb(180, true);
        assert_eq!(s.top, 180);
        assert!(s.following, "贴底即恢复跟随");
    }
}
