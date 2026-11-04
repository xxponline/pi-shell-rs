//! pi-rust-tui: minimal ratatui front-end for `pi --mode rpc`.

mod app;

use app::App;

fn main() {
    if let Err(e) = App::new().run() {
        eprintln!("pi-rust-tui: {e}");
        std::process::exit(1);
    }
}
