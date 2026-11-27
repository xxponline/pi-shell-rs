//! GPUI 构建模式模块树（feature `gpui-support`，design D3/D4）。
//!
//! - [`actor`]：kameo 薄层 RPC 路由（拥有 RpcClient，ask 命令 / tell 事件扇出）
//! - [`terminal`]：GPUIAgentTerminal 组件（内建控件渲染三分区）
//! - [`overlay`]：浮层渲染（遮罩 + 居中面板，状态/按键在共享层）
//! - [`window`]：独立窗口运行形态（`pi-shell-gpui` bin 入口）
//!
//! 特性关闭时本文件及其子模块不参与编译，默认构建零 gpui/kameo 依赖。

pub mod actor;
pub mod input;
pub mod overlay;
pub mod scroll;
pub mod terminal;
pub mod theme;
pub mod window;
