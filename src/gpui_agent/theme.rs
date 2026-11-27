//! 独立窗口主题 mode 状态与 system 解析（spec: 独立窗口主题三模式；
//! design D1/D3）。
//!
//! mode 是窗口内存态（A3 决策：不持久化、不读写 pi `settings.json`），
//! 默认 `System`；生效值经 [`resolved`] 解析为 kit `ThemeMode` 后交给
//! `Theme::change`（内建控件）与本仓 palette 双表（render 入口已接，
//! task1.2）。

use gpui_kit::WindowAppearance;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::{App, Global};

/// 用户可选的三态设置（窗口内存态，重启回默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ThemeSetting {
    /// 跟随操作系统外观（默认）。
    #[default]
    System,
    /// 固定浅色（task3 /settings 选择行构造；在此之前仅测试构造）。
    #[allow(dead_code)]
    Light,
    /// 固定深色（同上）。
    #[allow(dead_code)]
    Dark,
}

impl ThemeSetting {
    /// 循环到下一个取值（/settings Theme 行 enter；system → light → dark → system）。
    ///
    /// 输入: 无（读 self）；输出: 下一个设置
    pub(crate) fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    /// 行内展示名（小写，与 pi 设置选择器同名）。
    ///
    /// 输入: 无（读 self）；输出: "system" / "light" / "dark"
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// 设置 + 当前外观 → 生效模式（纯函数，无 TTY 可测）。
///
/// 输入: setting — 三态设置；appearance — 窗口/OS 当前外观（gpui 四变体，
///       `Vibrant*` 为 macOS vibrancy，按明暗族归类）；
/// 输出: kit `ThemeMode`——`System` 按外观落 Light/Dark，固定态无视外观
pub(crate) fn resolved(setting: ThemeSetting, appearance: WindowAppearance) -> ThemeMode {
    match setting {
        ThemeSetting::Light => ThemeMode::Light,
        ThemeSetting::Dark => ThemeMode::Dark,
        ThemeSetting::System => match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        },
    }
}

/// 窗口主题设置状态（gpui Global；内存态，重启回默认 System）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ThemeState {
    /// 当前三态设置。
    pub(crate) setting: ThemeSetting,
}

impl Global for ThemeState {}

impl ThemeState {
    /// 读当前状态（未初始化时回退默认 `System`——测试与早启动路径安全）。
    ///
    /// 输入: cx — gpui App 上下文；
    /// 输出: 当前状态副本（缺省 = `ThemeState::default()`）
    pub(crate) fn get(cx: &App) -> Self {
        cx.try_global::<ThemeState>().copied().unwrap_or_default()
    }

    /// 写入设置（只改内存态、不落盘；生效应用由调用方走 [`apply`] 与
    /// 重绘——design D1：本函数无副作用于主题解析）。
    ///
    /// 输入: cx — gpui App 上下文；setting — 新的三态设置；
    /// 输出: 无返回；全局状态更新
    pub(crate) fn set(cx: &mut App, setting: ThemeSetting) {
        cx.set_global(ThemeState { setting });
    }
}

/// 启动主题初始化：写默认设置（内存态，A3 不落盘）并按当前外观应用。
///
/// 输入: cx — gpui App（须已 `gpui_kit::init`）；
/// 输出: 无返回；全局 `ThemeState` = 默认（System）+ kit 主题 = 解析值
pub(crate) fn init_theme(cx: &mut App) {
    ThemeState::set(cx, ThemeSetting::default());
    apply(cx);
}

/// 把当前设置应用到全局主题（启动与设置变更共用入口，task2.2）。
///
/// 输入: cx — gpui App 上下文；
/// 输出: 无返回；`Theme::change(resolved(设置, cx.window_appearance()))`
pub(crate) fn apply(cx: &mut App) {
    let setting = ThemeState::get(cx).setting;
    apply_mode(setting, cx.window_appearance(), cx);
}

/// 外观变化回调（`Window::observe_window_appearance` 注册体，task2.2）。
///
/// 输入: window — 变化后的窗口（读缓存外观）；cx — gpui App 上下文；
/// 输出: 无返回；仅 System 设置下重解析，固定态不动作（幂等）
pub(crate) fn on_appearance_changed(window: &mut gpui_kit::Window, cx: &mut App) {
    on_appearance_changed_with(window.appearance(), cx);
}

/// 外观变化处理的可注入决策体（测试注入翻转；逻辑与回调一致）。
///
/// 输入: appearance — 变化后的外观（测试可注入任意值）；cx — gpui App；
/// 输出: 无返回；System → 重解析应用；固定 Light/Dark → 直接返回不动作
pub(crate) fn on_appearance_changed_with(appearance: WindowAppearance, cx: &mut App) {
    if ThemeState::get(cx).setting != ThemeSetting::System {
        return;
    }
    apply_mode(ThemeSetting::System, appearance, cx);
}

/// 外观订阅保活句柄（gpui Global；应用生命周期存活，drop = 退订，
/// task2.2——`Subscription` 非 Copy，故独立 Global 不进 `ThemeState`；
/// 字段从不读：RAII 保活即其全部职责）。
#[allow(dead_code)]
pub(crate) struct AppearanceSubscription(pub(crate) gpui_kit::Subscription);

impl Global for AppearanceSubscription {}

/// 解析 + `Theme::change`（三处接线共用的核心）。
///
/// 输入: setting — 三态设置；appearance — 当前外观；cx — gpui App；
/// 输出: 无返回；kit 全局主题切到解析后的 Light/Dark
fn apply_mode(setting: ThemeSetting, appearance: WindowAppearance, cx: &mut App) {
    Theme::change(resolved(setting, appearance), None, cx);
}

/// 循环三态并应用（/settings Theme 行；task3.1——只改内存态，A3 不写文件）。
///
/// 输入: cx — gpui App 上下文；
/// 输出: 无返回；`ThemeState` 循环到下一值 + kit 主题按新设置应用
pub(crate) fn cycle_and_apply(cx: &mut App) {
    let next = ThemeState::get(cx).setting.next();
    ThemeState::set(cx, next);
    apply(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认 = System（design D1/A3：不持久化，重启回默认）。
    #[test]
    fn default_setting_is_system() {
        assert_eq!(ThemeState::default().setting, ThemeSetting::System);
        assert_eq!(ThemeSetting::default(), ThemeSetting::System);
    }

    /// `resolved` 解析矩阵：3 设置 × 4 外观（Vibrant 按明暗族归类）。
    #[test]
    fn resolved_matrix_covers_three_settings_and_four_appearances() {
        use WindowAppearance::*;
        for (s, a, want) in [
            (ThemeSetting::System, Light, ThemeMode::Light),
            (ThemeSetting::System, VibrantLight, ThemeMode::Light),
            (ThemeSetting::System, Dark, ThemeMode::Dark),
            (ThemeSetting::System, VibrantDark, ThemeMode::Dark),
            (ThemeSetting::Light, Dark, ThemeMode::Light),
            (ThemeSetting::Light, VibrantDark, ThemeMode::Light),
            (ThemeSetting::Dark, Light, ThemeMode::Dark),
            (ThemeSetting::Dark, VibrantLight, ThemeMode::Dark),
        ] {
            assert_eq!(resolved(s, a), want, "{s:?} × {a:?}");
        }
    }

    /// 循环顺序与展示名（task3.1）：system → light → dark → system。
    #[test]
    fn next_cycles_system_light_dark_and_labels_match() {
        let mut s = ThemeSetting::System;
        let mut seen = vec![s.label()];
        for _ in 0..3 {
            s = s.next();
            seen.push(s.label());
        }
        assert_eq!(seen, ["system", "light", "dark", "system"]);
        assert_eq!(ThemeSetting::System.label(), "system");
    }

    /// Global 读写：set 后 get 返回新值，未初始化 get 回退默认。
    ///
    /// 功能: 未初始化断言回退 → set(Dark) → get 断言往返；
    /// 输入: 测试上下文（`gpui_kit::init` 不依赖——Global 与主题无关）；
    /// 输出: 断言通过。
    #[gpui_kit::test]
    fn state_roundtrip_and_default_fallback(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(
                ThemeState::get(cx).setting,
                ThemeSetting::System,
                "未初始化应回退默认"
            );
            ThemeState::set(cx, ThemeSetting::Dark);
            assert_eq!(ThemeState::get(cx).setting, ThemeSetting::Dark, "set/get 往返");
        });
    }

    /// 外观变化决策（task2.2，可注入）：System 跟随注入值，固定态不动作。
    ///
    /// 功能: System 下注入 Dark/VibrantLight 断言 mode 跟随；切 Dark 设置
    ///       后 apply，再注入 Light 断言 mode 不变（幂等不动作）；
    /// 输入: 测试上下文；
    /// 输出: 断言通过。
    #[gpui_kit::test]
    fn appearance_change_follows_system_and_ignores_fixed(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::theme::Theme;
        cx.update(gpui_kit::init);
        // System：注入外观翻转 → mode 跟随。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::System);
            on_appearance_changed_with(WindowAppearance::Dark, cx);
            assert_eq!(Theme::global(cx).mode, ThemeMode::Dark, "System 跟随 Dark");
            on_appearance_changed_with(WindowAppearance::VibrantLight, cx);
            assert_eq!(Theme::global(cx).mode, ThemeMode::Light, "System 跟随 VibrantLight");
        });
        // 固定 Dark：注入外观翻转 → 不动作（mode 保持 Dark）。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::Dark);
            apply(cx);
            assert_eq!(Theme::global(cx).mode, ThemeMode::Dark, "前置：固定 Dark");
            on_appearance_changed_with(WindowAppearance::Light, cx);
            assert_eq!(
                Theme::global(cx).mode,
                ThemeMode::Dark,
                "固定态注入外观翻转不改 mode"
            );
        });
        // 固定 Light：注入 Dark → 不动作。
        cx.update(|cx| {
            ThemeState::set(cx, ThemeSetting::Light);
            apply(cx);
            on_appearance_changed_with(WindowAppearance::Dark, cx);
            assert_eq!(
                Theme::global(cx).mode,
                ThemeMode::Light,
                "固定 Light 不受注入影响"
            );
        });
    }

    /// 启动初始化（task2.1/2.2）：默认设置 = System，mode = 解析值。
    ///
    /// 功能: init_theme 后断言设置为默认、kit mode 等于 resolved(默认, 外观)；
    /// 输入: 测试上下文；
    /// 输出: 断言通过。
    #[gpui_kit::test]
    fn init_theme_applies_default_system(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::theme::Theme;
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            init_theme(cx);
            assert_eq!(ThemeState::get(cx).setting, ThemeSetting::System, "默认 System");
            let expect = resolved(ThemeSetting::default(), cx.window_appearance());
            assert_eq!(Theme::global(cx).mode, expect, "mode = 解析(默认, 当前外观)");
        });
    }
}
