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
