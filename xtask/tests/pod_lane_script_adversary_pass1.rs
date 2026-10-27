//! Adversary pass 1 on `story:pod-lane-runs-adversary-and-security`.
//!
//! `scripts/pod-lane.sh` is executed as the gate's `pod-lane` job runs it, with a stand-in `cargo`
//! first on `PATH` that prints, for each `--test <binary>`, the log a harness run could print. The
//! script's own header states what it refuses: "each suite is required by the harness's own
//! `test result:` line, with at least one case passed, and by the absence of any `ABSENT` line";
//! the story's outcome is that the job "fails when any of them is absent or fails".

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const SUITES: [&str; 3] = [
    "ess_confinement_pod",
    "adversary_pod",
    "security_review_pod",
];

const STANDIN: &str = r#"#!/usr/bin/env bash
binary=""
previous=""
for argument in "$@"; do
  if [[ "$previous" == "--test" ]]; then
    binary="$argument"
    break
  fi
  previous="$argument"
done
printf '%s\n' "$*" >> "$STANDIN_DIR/argv.log"
cat "$STANDIN_DIR/$binary.log"
"#;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/pod-lane.sh")
}

fn passed(count: u32, ignored: u32, filtered: u32) -> String {
    format!(
        "\nrunning {count} tests\n\ntest result: ok. {count} passed; 0 failed; {ignored} ignored; \
         0 measured; {filtered} filtered out; finished in 0.10s\n\n"
    )
}

/// Runs the script with the stand-in printing `logs[suite]` (the suite's default otherwise);
/// returns whether it exited 0, and its combined output.
fn run_lane(logs: &[(&str, String)], extra: &[&str]) -> (bool, String) {
    let directory = tempfile::tempdir().expect("tempdir");
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).expect("bin");
    let cargo = bin.join("cargo");
    fs::write(&cargo, STANDIN).expect("stand-in");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("chmod");
    for suite in SUITES {
        let log = logs
            .iter()
            .find(|(name, _)| *name == suite)
            .map_or_else(|| passed(7, 0, 0), |(_, log)| log.clone());
        fs::write(directory.path().join(format!("{suite}.log")), log).expect("log");
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("bash")
        .arg(script())
        .args(extra)
        .env("PATH", path)
        .env("STANDIN_DIR", directory.path())
        .env("SUBSTRATE_POD_LANE", "1")
        .env(
            "SUBSTRATE_POD_KUBECONFIG",
            directory.path().join("pod.json"),
        )
        .env(
            "SUBSTRATE_POD_KUBECONFIG_NOLOG",
            directory.path().join("pod-nolog.json"),
        )
        .env("SUBSTRATE_POD_KIND_NODE", "lane-control-plane")
        .output()
        .expect("bash runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(6)..].join("\n")
}

/// Control: a short log carrying an `ABSENT` line is refused, as the script says.
#[test]
fn control_an_absent_line_in_a_short_log_fails_the_lane() {
    let log = format!(
        "\nrunning 7 tests\npod lane: ABSENT; 0 executed\n{}",
        passed(7, 0, 0)
    );
    let (green, output) = run_lane(&[("ess_confinement_pod", log)], &[]);
    assert!(
        !green,
        "a short log with ABSENT went green:\n{}",
        tail(&output)
    );
}

/// `printf '%s\n' "${log}" | grep -q 'ABSENT'` under `set -o pipefail`: `grep -q` exits at the
/// first match, `printf` then writes into a closed pipe and fails, the pipeline's status is that
/// failure, and the `if` reads it as "no ABSENT line". Any log whose bytes after the `ABSENT`
/// line do not fit in the pipe buffer is waved through.
#[test]
fn an_absent_line_followed_by_more_than_a_pipe_holds_fails_the_lane() {
    let filler = "x".repeat(63);
    let mut log = String::from("\nrunning 7 tests\npod lane: ABSENT; 0 executed\n");
    for _ in 0..4096 {
        log.push_str(&filler);
        log.push('\n');
    }
    log.push_str(&passed(7, 0, 0));
    let (green, output) = run_lane(&[("ess_confinement_pod", log)], &[]);
    assert!(
        !green,
        "a suite printed ABSENT and the pod lane went green:\n{}",
        tail(&output)
    );
}

/// The script requires "at least one case passed". Every suite holds offline cases that pass with
/// no cluster, so a suite whose cluster case is `#[ignore]`d still has a non-zero passed count.
#[test]
fn a_suite_with_an_ignored_case_fails_the_lane() {
    let (green, output) = run_lane(&[("adversary_pod", passed(6, 1, 0))], &[]);
    assert!(
        !green,
        "adversary_pod ignored a case and the pod lane went green:\n{}",
        tail(&output)
    );
}

/// "Extra arguments go to every suite's test harness": a filter that deselects the cluster cases
/// leaves each suite's offline cases, and each still reports a non-zero passed count.
#[test]
fn a_filter_that_deselects_the_cluster_cases_fails_the_lane() {
    let logs: Vec<(&str, String)> = SUITES
        .iter()
        .map(|suite| (*suite, passed(5, 0, 3)))
        .collect();
    let (green, output) = run_lane(&logs, &["--skip", "lane"]);
    assert!(
        !green,
        "every suite filtered out cases and the pod lane went green:\n{}",
        tail(&output)
    );
}
