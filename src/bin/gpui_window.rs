//! `pi-shell-gpui` 独立窗口入口（spec: 独立窗口运行形态）。
//!
//! 功能：以 `gpui-support` 特性构建的独立窗口形态——拉起会话并内嵌
//! GPUIAgentTerminal；输入: 无（命令行可后续扩展 pi_bin/cwd 注入）；
//! 输出: 窗口进程，关闭窗口即退出并回收 pi 子进程。
#![cfg(feature = "gpui-support")]

fn main() {
    // 任务 3.1 接线：开窗 + 内嵌组件 + 关闭回收（占位，随任务推进填充）。
    pi_shell_rs::gpui_agent::window::run();
}
