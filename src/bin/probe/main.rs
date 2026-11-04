//! pi RPC 探针：驱动 `pi --mode rpc`，抓取 JSONL 事件流并归档为 fixture。

mod artifact;
mod classify;
mod scenario;
mod scenarios;

use std::path::PathBuf;
use std::time::Duration;

use pi_rust_tui::rpc::{resolve_binary, Transport};
use serde_json::json;

use crate::scenario::RunOutcome;

const HELP: &str = "\
probe - pi RPC protocol probe

USAGE:
    probe [OPTIONS]

OPTIONS:
    --list                 List built-in scenarios and exit
    --scenario <name>      Run one built-in scenario
    --all                  Run all built-in scenarios
    --out <dir>            Output root (default: probe-out)
    --timeout-ms <ms>      Default per-step timeout (default: 60000)
    --snapshot             Also write a tracked, truncated snapshot copy
    --ext <path>           Extra pi arg repeated: passed as `-e <path>` (fixture extension)
    -h, --help             Print this help
";

struct Cli {
    list: bool,
    scenario: Option<String>,
    all: bool,
    out: PathBuf,
    timeout_ms: u64,
    snapshot: bool,
    exts: Vec<String>,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            list: false,
            scenario: None,
            all: false,
            out: PathBuf::from("probe-out"),
            timeout_ms: 60_000,
            snapshot: false,
            exts: Vec::new(),
        }
    }
}

fn parse_args(argv: Vec<String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--list" => cli.list = true,
            "--all" => cli.all = true,
            "--snapshot" => cli.snapshot = true,
            "--scenario" => {
                i += 1;
                cli.scenario = Some(argv.get(i).ok_or("--scenario needs a value")?.clone());
            }
            "--out" => {
                i += 1;
                cli.out = PathBuf::from(argv.get(i).ok_or("--out needs a value")?);
            }
            "--timeout-ms" => {
                i += 1;
                cli.timeout_ms = argv
                    .get(i)
                    .ok_or("--timeout-ms needs a value")?
                    .parse()
                    .map_err(|_| "invalid --timeout-ms")?;
            }
            "--ext" => {
                i += 1;
                cli.exts.push(argv.get(i).ok_or("--ext needs a value")?.clone());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(cli)
}

fn main() {
    let cli = match parse_args(std::env::args().skip(1).collect()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    if cli.list {
        for s in scenarios::all() {
            println!("{:<14} {}", s.name, s.description);
        }
        return;
    }

    let selected: Vec<_> = if cli.all {
        scenarios::all()
    } else if let Some(name) = &cli.scenario {
        match scenarios::by_name(name) {
            Some(s) => vec![s],
            None => {
                eprintln!("error: unknown scenario '{name}'");
                std::process::exit(2);
            }
        }
    } else {
        eprintln!("error: provide --scenario <name>, --all, or --list\n\n{HELP}");
        std::process::exit(2);
    };

    let binary = resolve_binary();
    let extra_args = {
        let mut v = Vec::new();
        for e in &cli.exts {
            v.push("-e".to_string());
            v.push(e.clone());
        }
        v
    };

    let mut failures = 0;
    let total = selected.len();
    for (idx, sc) in selected.iter().enumerate() {
        eprintln!("[{}/{}] scenario: {}", idx + 1, total, sc.name);
        match run_one(sc, &binary, &extra_args, &cli) {
            Ok(true) => eprintln!("    ok: {}", sc.name),
            Ok(false) => {
                eprintln!("    FAIL: {}", sc.name);
                failures += 1;
            }
            Err(e) => {
                eprintln!("    ERROR: {e}");
                failures += 1;
            }
        }
    }

    if failures > 0 {
        eprintln!("{failures}/{total} scenario(s) failed");
        std::process::exit(1);
    }
}

/// Spawn, preflight, run one scenario, persist artifacts. Returns Ok(ok).
fn run_one(
    sc: &scenario::Scenario,
    binary: &str,
    extra_args: &[String],
    cli: &Cli,
) -> std::io::Result<bool> {
    let mut transport = Transport::spawn_rpc(binary, extra_args)?;

    // 5.2 preflight: get_state must return before any scenario steps.
    let preflight = scenario::Scenario {
        name: "preflight",
        description: "",
        steps: vec![scenario::Step::Send {
            command: json!({"type": "get_state"}),
            timeout: Duration::from_secs(15),
            expect_error: false,
        }],
    };
    let pf = scenario::run(&mut transport, &preflight, Duration::from_secs(15));
    if !pf.ok {
        transport.shutdown();
        eprintln!(
            "preflight get_state failed. Is pi logged in? Run the native `pi` once to complete auth (~/.pi/agent/auth.json). detail: {:?}",
            pf.report.failure
        );
        return Ok(false);
    }

    let outcome: RunOutcome =
        scenario::run(&mut transport, sc, Duration::from_millis(cli.timeout_ms));
    transport.shutdown();

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            // Simple sortable timestamp YYYYMMDD-HHMMSS-mmm
            let secs = d.as_secs();
            let millis = d.subsec_millis();
            format!("{secs}-{millis:03}")
        })
        .unwrap_or_else(|_| "unknown".into());

    let cfg = artifact::ArtifactConfig {
        out_root: &cli.out,
        scenario_name: sc.name,
        timestamp,
        pi_version: "unknown".into(),
        binary: binary.to_string(),
    };
    let paths = artifact::prepare_run(&cfg)?;
    artifact::write_run(&paths, &outcome, &cfg)?;
    eprintln!("    artifacts: {}", paths.run_dir.display());

    if cli.snapshot {
        let fixture_root = PathBuf::from("tests/fixtures/probe");
        artifact::write_snapshot(&fixture_root, sc.name, &outcome, 4000)?;
        eprintln!("    snapshot: {}/{}/snapshot", fixture_root.display(), sc.name);
    }

    Ok(outcome.ok)
}
