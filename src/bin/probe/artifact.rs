//! On-disk run artifacts: transcript, stderr, summary, meta, and snapshots.
//!
//! Layout (under the output root, default `probe-out/`):
//!
//! ```text
//! <root>/<scenario>/runs/<timestamp>/transcript.jsonl
//! <root>/<scenario>/runs/<timestamp>/stderr.log
//! <root>/<scenario>/runs/<timestamp>/summary.json
//! <root>/<scenario>/runs/<timestamp>/meta.json
//! <root>/<scenario>/latest/                 (copy of the newest run)
//! ```
//!
//! Tracked test fixtures are produced separately into
//! `tests/fixtures/probe/<scenario>/snapshot/` by [`write_snapshot`], with large
//! string fields truncated and the cuts recorded in summary.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::scenario::{RunReport, RunOutcome};

/// Where one run's files live.
pub struct RunPaths {
    pub run_dir: PathBuf,
    pub transcript: PathBuf,
    pub stderr: PathBuf,
    pub summary: PathBuf,
    pub meta: PathBuf,
}

/// Configuration for one run's outputs.
pub struct ArtifactConfig<'a> {
    pub out_root: &'a Path,
    pub scenario_name: &'a str,
    pub timestamp: String,
    pub pi_version: String,
    pub binary: String,
}

/// Resolve (and create) the run directory paths.
pub fn prepare_run(cfg: &ArtifactConfig) -> std::io::Result<RunPaths> {
    let run_dir = cfg
        .out_root
        .join(cfg.scenario_name)
        .join("runs")
        .join(&cfg.timestamp);
    fs::create_dir_all(&run_dir)?;
    Ok(RunPaths {
        transcript: run_dir.join("transcript.jsonl"),
        stderr: run_dir.join("stderr.log"),
        summary: run_dir.join("summary.json"),
        meta: run_dir.join("meta.json"),
        run_dir,
    })
}

/// Write all artifacts for one run, then refresh the `latest/` copy.
pub fn write_run(
    paths: &RunPaths,
    outcome: &RunOutcome,
    cfg: &ArtifactConfig,
) -> std::io::Result<()> {
    // transcript
    {
        let f = fs::File::create(&paths.transcript)?;
        let mut w = std::io::BufWriter::new(f);
        crate::scenario::write_transcript(&outcome.report.records, &mut w)?;
        w.flush()?;
    }

    // stderr
    {
        let mut f = fs::File::create(&paths.stderr)?;
        for line in &outcome.report.stderr_lines {
            writeln!(f, "[{:>8?}] {}", line.at, line.text)?;
        }
    }

    // summary
    {
        let summary = build_summary(&outcome.report, outcome.ok);
        let f = fs::File::create(&paths.summary)?;
        serde_json::to_writer_pretty(f, &summary)?;
    }

    // meta
    {
        let meta = json!({
            "scenario": cfg.scenario_name,
            "timestamp": cfg.timestamp,
            "binary": cfg.binary,
            "piVersion": cfg.pi_version,
            "cwd": std::env::current_dir().ok().and_then(|p| p.to_str().map(String::from)),
            "gitSha": git_sha(),
        });
        let f = fs::File::create(&paths.meta)?;
        serde_json::to_writer_pretty(f, &meta)?;
    }

    refresh_latest(paths, cfg)?;
    Ok(())
}

fn build_summary(report: &RunReport, ok: bool) -> Value {
    let counts: BTreeMap<&String, &u32> = report.counts.iter().collect();
    let counts_json: BTreeMap<String, u32> =
        counts.into_iter().map(|(k, v)| (k.clone(), *v)).collect();

    let steps: Vec<Value> = report
        .step_results
        .iter()
        .map(|(idx, ok, detail)| {
            json!({"step": idx, "ok": ok, "detail": detail})
        })
        .collect();

    json!({
        "ok": ok,
        "failure": report.failure,
        "recordCount": report.records.len(),
        "counts": counts_json,
        "steps": steps,
        "truncations": [],
    })
}

fn refresh_latest(paths: &RunPaths, cfg: &ArtifactConfig) -> std::io::Result<()> {
    let latest = cfg
        .out_root
        .join(cfg.scenario_name)
        .join("latest");
    if latest.exists() {
        fs::remove_dir_all(&latest)?;
    }
    copy_dir_recursive(&paths.run_dir, &latest)?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn git_sha() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// Recursively truncate string leaves longer than `max_chars`, recording each cut
/// as a JSON-pointer-ish path. Returns the mutated value and cut descriptions.
pub fn truncate_value(value: Value, max_chars: usize) -> (Value, Vec<Value>) {
    let mut cuts = Vec::new();
    let v = truncate_inner(value, max_chars, String::new(), &mut cuts);
    (v, cuts)
}

fn truncate_inner(
    value: Value,
    max_chars: usize,
    path: String,
    cuts: &mut Vec<Value>,
) -> Value {
    match value {
        Value::String(s) if s.chars().count() > max_chars => {
            let original_len = s.chars().count();
            let kept: String = s.chars().take(max_chars).collect();
            cuts.push(json!({
                "path": if path.is_empty() { "/".to_string() } else { path.clone() },
                "originalChars": original_len,
                "truncatedTo": max_chars,
            }));
            Value::String(format!("{kept}…<truncated {original_len} chars>"))
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    let p = format!("{path}/{i}");
                    truncate_inner(item, max_chars, p, cuts)
                })
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, item)| {
                    let p = format!("{path}/{k}");
                    (k, truncate_inner(item, max_chars, p, cuts))
                })
                .collect(),
        ),
        other => other,
    }
}

/// Write a tracked snapshot of a run into `tests/fixtures/probe/<scenario>/snapshot/`,
/// truncating the transcript `raw` payloads over `max_chars` and recording cuts.
pub fn write_snapshot(
    fixture_root: &Path,
    scenario_name: &str,
    outcome: &RunOutcome,
    max_chars: usize,
) -> std::io::Result<()> {
    let dir = fixture_root.join(scenario_name).join("snapshot");
    fs::create_dir_all(&dir)?;

    // Truncated transcript.
    let mut total_cuts = Vec::new();
    {
        let f = fs::File::create(dir.join("transcript.jsonl"))?;
        let mut w = std::io::BufWriter::new(f);
        for r in &outcome.report.records {
            let (raw, cuts) = truncate_value(r.raw.clone(), max_chars);
            for c in cuts {
                total_cuts.push(json!({"seq": r.seq, "dir": format!("{:?}", r.dir).to_lowercase(), "cut": c}));
            }
            let wrapper = json!({
                "seq": r.seq,
                "ts_ms": r.at_ms,
                "dir": r.dir,
                "raw": raw,
            });
            writeln!(w, "{}", serde_json::to_string(&wrapper)?)?;
        }
        w.flush()?;
    }

    // Summary including truncation records.
    {
        let mut summary = build_summary(&outcome.report, outcome.ok);
        summary
            .as_object_mut()
            .unwrap()
            .insert("truncations".into(), Value::Array(total_cuts));
        let f = fs::File::create(dir.join("summary.json"))?;
        serde_json::to_writer_pretty(f, &summary)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncates_nested_long_strings_and_records_paths() {
        let input = json!({
            "small": "ok",
            "big": "x".repeat(50),
            "arr": [{ "tool": "y".repeat(30) }],
        });
        let (out, cuts) = truncate_value(input, 10);
        assert_eq!(out["small"], json!("ok"));
        assert!(out["big"].as_str().unwrap().contains("<truncated"));
        assert!(out["arr"][0]["tool"].as_str().unwrap().contains("<truncated"));
        let paths: Vec<String> =
            cuts.iter().map(|c| c["path"].as_str().unwrap().to_string()).collect();
        assert!(paths.iter().any(|p| p == "/big"), "paths: {paths:?}");
        assert!(paths.iter().any(|p| p == "/arr/0/tool"), "paths: {paths:?}");
        assert!(cuts.iter().all(|c| c["originalChars"].as_u64() == Some(30)
            || c["originalChars"].as_u64() == Some(50)));
    }

    #[test]
    fn writes_run_files_and_latest_copy() {
        let tmp = std::env::temp_dir().join(format!("probe-art-{:?}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let cfg = ArtifactConfig {
            out_root: &tmp,
            scenario_name: "demo",
            timestamp: "20260101-000000".into(),
            pi_version: "test".into(),
            binary: "fake-pi".into(),
        };
        // Build a minimal outcome via the public report shape.
        let report = crate::scenario::RunReport::new_for_test();
        let outcome = crate::scenario::RunOutcome { report, ok: true };
        let paths = prepare_run(&cfg).unwrap();
        write_run(&paths, &outcome, &cfg).unwrap();

        assert!(paths.transcript.exists());
        assert!(paths.summary.exists());
        assert!(paths.meta.exists());
        assert!(tmp.join("demo/latest/summary.json").exists());

        let summary: Value =
            serde_json::from_str(&fs::read_to_string(tmp.join("demo/latest/summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["ok"], json!(true));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn snapshot_is_truncated_and_loadable() {
        let tmp = std::env::temp_dir().join(format!("probe-snap-{:?}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let report = crate::scenario::RunReport::new_for_test();
        let outcome = crate::scenario::RunOutcome { report, ok: true };
        write_snapshot(&tmp, "demo", &outcome, 50).unwrap();
        let snap = tmp.join("demo/snapshot/transcript.jsonl");
        let content = fs::read_to_string(snap).unwrap();
        for line in content.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            assert!(v.get("seq").is_some());
            assert!(v.get("raw").is_some());
        }
        let summary: Value =
            serde_json::from_str(&fs::read_to_string(tmp.join("demo/snapshot/summary.json")).unwrap())
                .unwrap();
        assert!(summary.get("truncations").is_some());
        let _ = fs::remove_dir_all(&tmp);
    }
}
