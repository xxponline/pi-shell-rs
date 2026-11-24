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
/// 固定浅色主题模式（spec: 独立窗口浅色模式固定）。
///
/// 功能: 把全局主题显式置为 Light，不安装系统外观订阅；
/// 输入: cx — gpui App 上下文（须已执行 `gpui_kit::init`）；
/// 输出: 无返回；副作用为全局 `Theme.mode = Light`，此后系统外观
///       变化不影响本窗口配色（完整主题系统为后续 change）。
fn pin_light_theme(cx: &mut gpui_kit::App) {
    gpui_kit::component::theme::Theme::change(
        gpui_kit::component::theme::ThemeMode::Light,
        None,
        cx,
    );
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
        // 启动即浅色（spec: 独立窗口浅色模式固定；不装外观订阅）。
        pin_light_theme(cx);
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
    use super::pin_light_theme;
    use gpui_kit::component::theme::{Theme, ThemeMode};

    /// pin_light_theme 必须把主题强制为 Light（spec: 启动即浅色）。
    ///
    /// 功能: 先翻成 Dark 模拟被外观带偏，再 pin，断言回 Light；
    /// 输入: 测试上下文（`gpui_kit::init` 已执行）；
    /// 输出: 断言通过（mode 由 Dark 复位为 Light）。
    #[gpui_kit::test]
    fn pin_light_theme_forces_light_mode(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| Theme::change(ThemeMode::Dark, None, cx));
        cx.update(|cx| assert_eq!(Theme::global(cx).mode, ThemeMode::Dark, "前置：先处于 Dark"));
        cx.update(pin_light_theme);
        cx.update(|cx| assert_eq!(Theme::global(cx).mode, ThemeMode::Light, "pin 后必须 Light"));
    }
}
