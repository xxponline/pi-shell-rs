//! pi-shell-rs: minimal ratatui front-end for `pi --mode rpc`.

use pi_shell_rs::app::App;

fn main() {
    if let Err(e) = App::new().run() {
        eprintln!("pi-shell-rs: {e}");
        std::process::exit(1);
    }
}
