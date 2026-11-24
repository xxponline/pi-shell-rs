//! pi-shell-rs library: headless RPC connection, session state, and the TUI
//! app for driving `pi --mode rpc`. The app module is the embeddable core:
//! the standalone bin is a thin harness over it, and host applications (e.g.
//! a GUI shell) can embed it with an injected input source and a virtual
//! terminal backend.


// Allow in-crate paths like `pi_shell_rs::rpc::...` (the app module was
// originally written in the bin crate, where this name resolved to the lib).
extern crate self as pi_shell_rs;

pub mod app;
pub mod rpc;
pub mod session;

/// GPUI 构建模式（feature `gpui-support`，design D4）：核心薄层 RPC 路由
/// （kameo actor）与 GPUIAgentTerminal 组件。特性关闭时整棵树编译期不存在。
#[cfg(feature = "gpui-support")]
pub mod gpui_agent;
