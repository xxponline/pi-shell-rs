//! 独立窗口运行形态（spec: 独立窗口运行形态）。
//!
//! 创建 gpui-kit 窗口并内嵌 [`crate::gpui_agent::terminal::GPUIAgentTerminal`]；
//! 关闭窗口时退出会话并回收 pi 子进程（`pi-shell-gpui` bin 的入口逻辑）。
//!
//! 生命周期：探活成功 → 开窗（组件接管会话）→ 关窗/退出 → 组件实体 Drop
//! 与 `on_app_quit` 钩子双路调用幂等 `shutdown`，pi 必被回收。

use gpui_kit::*;

use crate::gpui_agent::actor::RpcSession;
use crate::gpui_agent::terminal::GPUIAgentTerminal;

/// 独立窗口入口：拉起会话 → 开窗内嵌组件 → 阻塞运行至退出。
///
/// 功能: 解析 pi 二进制、spawn + `get_state` 探活（失败打印可读错误并
///       以退出码 1 结束，不悬挂不崩溃——spec: 启动探活与错误提示），
///       然后进入 gpui 事件循环（`QuitMode::LastWindowClosed`：关窗即退，
///       由组件的退出钩子/实体 Drop 回收 pi）；
/// 输入: 无（`PI_BIN` 环境变量覆盖二进制路径）；
/// 输出: 无返回值；正常退出码 0，启动失败退出码 1（stderr 中文可读错误）。
/// pi `scrollbarTrack` 预计算（light.json okhsl → sRGB，task1.4）。
const SCROLLBAR_TRACK_LIGHT: &str = "#e1e3e4";
/// pi `scrollbarThumb` 预计算（light.json）。
const SCROLLBAR_THUMB_LIGHT: &str = "#96a0a4";
/// pi `scrollbarTrack` 预计算（dark.json）。
const SCROLLBAR_TRACK_DARK: &str = "#484e52";
/// pi `scrollbarThumb` 预计算（dark.json）。
const SCROLLBAR_THUMB_DARK: &str = "#97a0a5";

/// 滚动条 token 按 mode 覆写为 pi 值（task1.4；spec: 消息区滚动条 pi 同源）。
///
/// 功能: 把全局 Theme 的 light/dark 两份 `ThemeConfig.colors` 的
///       `scrollbar.background` / `scrollbar.thumb.background` /
///       `scrollbar.thumb.hover.background` 三个字段全部换成 pi
///       `scrollbarTrack/Thumb` 预计算 hex（pi 无 hover token → hover 取
///       thumb 值；kit 默认主题显式给了 hover，不写会残留默认灰），再触发
///       `Theme::change` 重解析；
/// 输入: cx — gpui App（须已 `gpui_kit::init` 且全局 Theme 已创建；
///       未创建时直接跳过——首次 `Theme::change` 后再调即可）；
/// 输出: 无返回；副作用为全局主题配置与解析色（track/thumb/hover）更新。
pub(crate) fn apply_pi_scrollbar_tokens(cx: &mut gpui_kit::App) {
    use gpui_kit::component::theme::{Theme, ThemeConfig};
    use std::rc::Rc;
    if !cx.has_global::<Theme>() {
        return;
    }
    let (light, dark, mode) = {
        let t = Theme::global(cx);
        (t.light_theme.clone(), t.dark_theme.clone(), t.mode)
    };
    let patch = |cfg: Rc<ThemeConfig>, track: &str, thumb: &str| -> Rc<ThemeConfig> {
        let mut c = cfg.as_ref().clone();
        c.colors.scrollbar = Some(track.into());
        c.colors.scrollbar_thumb = Some(thumb.into());
        c.colors.scrollbar_thumb_hover = Some(thumb.into());
        Rc::new(c)
    };
    {
        let t = Theme::global_mut(cx);
        t.light_theme = patch(light, SCROLLBAR_TRACK_LIGHT, SCROLLBAR_THUMB_LIGHT);
        t.dark_theme = patch(dark, SCROLLBAR_TRACK_DARK, SCROLLBAR_THUMB_DARK);
    }
    // reload_mode=true：换配置后重应用（`Theme::change` 文档承诺的通路）。
    Theme::change(mode, None, cx);
}

pub fn run() {
    let binary = crate::rpc::resolve_binary();
    let session = match RpcSession::spawn(&binary, &[], None) {
        Ok(session) => session,
        Err(e) => {
            // 展示可读错误并干净结束（spec: pi 缺失 / 探活失败两 scenario）。
            eprintln!("{e}");
            eprintln!("请确认已安装 pi，并用原生 pi 完成登录（~/.pi/agent/auth.json）。");
            std::process::exit(1);
        }
    };

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        // 启动默认 System + 按当前外观应用（spec: 启动默认跟随系统外观；
        // A3 内存态不落盘），随后覆写滚动条 token（task1.4；Theme 已创建）。
        crate::gpui_agent::theme::init_theme(cx);
        apply_pi_scrollbar_tokens(cx);
        // 关窗即退（宿主仓实测：macOS 默认 Explicit 会进程残留）。
        cx.set_quit_mode(QuitMode::LastWindowClosed);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(
                size(px(1000.), px(640.)),
                cx,
            )),
            titlebar: Some(TitlebarOptions {
                title: Some("pi-shell".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        gpui_kit::open_window(options, cx, |window, cx| {
            // system 随 OS 外观热切换（task2.2）：订阅保活于 Global 至退出；
            // 固定 Light/Dark 在回调内自判不动作（幂等）。
            let sub =
                window.observe_window_appearance(crate::gpui_agent::theme::on_appearance_changed);
            cx.set_global(crate::gpui_agent::theme::AppearanceSubscription(sub));
            let terminal = cx.new(|cx| GPUIAgentTerminal::new(session, window, cx));
            // 开窗即聚焦输入窗（后续焦点由 gpui 焦点环/点击维护）。
            terminal.read(cx).input_focus().focus(window, cx);
            terminal
        })
        .expect("打开窗口失败");
    });
}

#[cfg(test)]
mod tests {
    // 显式导入（不 glob）：glob 会把 `gpui::*` 的 `test` 属性宏引进作用域，
    // gpui 测试宏展开发出的裸 `#[test]` 会解析到它 → 无限递归（kit lib.rs 注释）。
    use super::apply_pi_scrollbar_tokens;
    use gpui_kit::component::theme::{Theme, ThemeMode};

    /// pi 滚动条 token 覆写（task1.4）：两 mode 解析 track/thumb = pi 预计算，
    /// hover = thumb（pi 无 hover token），切回 mode 补丁仍在。
    ///
    /// 功能: pin 后打补丁 → 断言浅色解析值；切 Dark 再断言；切回 Light 复验；
    /// 输入: 测试上下文（`gpui_kit::init` 已执行）；
    /// 输出: 断言通过（track/thumb/hover 均为 pi 同源 RGB）。
    #[gpui_kit::test]
    fn pi_scrollbar_tokens_apply_per_mode(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(crate::gpui_agent::theme::init_theme);
        cx.update(apply_pi_scrollbar_tokens);
        let rgb = |h: gpui_kit::Hsla| {
            let c: gpui_kit::Rgba = h.into();
            (
                (c.r * 255.0).round() as u8,
                (c.g * 255.0).round() as u8,
                (c.b * 255.0).round() as u8,
            )
        };
        cx.update(|cx| {
            let t = Theme::global(cx);
            assert_eq!(rgb(t.scrollbar), (0xe1, 0xe3, 0xe4), "浅 track");
            assert_eq!(rgb(t.scrollbar_thumb), (0x96, 0xa0, 0xa4), "浅 thumb");
            assert_eq!(
                rgb(t.scrollbar_thumb_hover),
                (0x96, 0xa0, 0xa4),
                "浅 hover = thumb（pi 无 hover token）"
            );
            Theme::change(ThemeMode::Dark, None, cx);
            let t = Theme::global(cx);
            assert_eq!(rgb(t.scrollbar), (0x48, 0x4e, 0x52), "深 track");
            assert_eq!(rgb(t.scrollbar_thumb), (0x97, 0xa0, 0xa5), "深 thumb");
            Theme::change(ThemeMode::Light, None, cx);
            let t = Theme::global(cx);
            assert_eq!(rgb(t.scrollbar), (0xe1, 0xe3, 0xe4), "切回浅 track 补丁仍在");
        });
    }
}
