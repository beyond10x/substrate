//! Offline assertions for `scripts/wait-checks.sh`: its exit status is decided by the check list
//! it reads. 0 where every check succeeded or was skipped or neutral, 1 where one ended any
//! other way, 3 where one is pending or none is reported, 2 for a list of neither form.
//!
//! The lists are recorded from this repository on 2026-11-26 and reduced to the fields the
//! script reads: `gh pr checks <n> --json name,state,bucket` of pull requests 303 (green), 295
//! (one failure) and 301 (failures and cancellations), and the check runs of commits
//! `a75cc35ae5` (green) and `224e1e5ffd` (two failures). The pending lists are those shapes with
//! one entry still running, as the two forms state a running check.
//!
//! The wait itself runs against a stand-in `gh` first on `PATH` that answers each call with
//! the next of a sequence of lists, so a case reads how many times the script asked. Nothing
//! here reaches the network. `sh`, `jq`, `awk` and `sleep` are the system's.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use super::{executable, repository_root};

const PR_GREEN: &str = r#"[{"bucket":"pass","name":"common / Security and privacy","state":"SUCCESS"},{"bucket":"skipping","name":"Host crate on macOS","state":"SKIPPED"},{"bucket":"pass","name":"Classify the change / Changed paths","state":"SUCCESS"},{"bucket":"pass","name":"Full gate","state":"SUCCESS"},{"bucket":"pass","name":"Build documentation","state":"SUCCESS"},{"bucket":"skipping","name":"Gate: tests and Clippy","state":"SKIPPED"},{"bucket":"skipping","name":"Gate: static checks","state":"SKIPPED"},{"bucket":"skipping","name":"Pod lane","state":"SKIPPED"},{"bucket":"pass","name":"Classify the change / Changed paths","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (actions)","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (rust)","state":"SUCCESS"},{"bucket":"pass","name":"CodeQL","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (javascript-typescript)","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (python)","state":"SUCCESS"}]"#;

const PR_ONE_FAILURE: &str = r#"[{"bucket":"pass","name":"Host crate on macOS","state":"SUCCESS"},{"bucket":"pass","name":"Full gate","state":"SUCCESS"},{"bucket":"pass","name":"Gate: static checks","state":"SUCCESS"},{"bucket":"pass","name":"Pod lane","state":"SUCCESS"},{"bucket":"pass","name":"Gate: tests and Clippy","state":"SUCCESS"},{"bucket":"pass","name":"Chart cluster scenarios","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (javascript-typescript)","state":"SUCCESS"},{"bucket":"pass","name":"Classify the change / Changed paths","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (actions)","state":"SUCCESS"},{"bucket":"pass","name":"Namespace lane static checks","state":"SUCCESS"},{"bucket":"pass","name":"Build documentation","state":"SUCCESS"},{"bucket":"pass","name":"Chart static checks","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (rust)","state":"SUCCESS"},{"bucket":"pass","name":"Quota lane","state":"SUCCESS"},{"bucket":"pass","name":"Chart lane with node profiles","state":"SUCCESS"},{"bucket":"pass","name":"Confinement lane","state":"SUCCESS"},{"bucket":"fail","name":"CodeQL","state":"FAILURE"},{"bucket":"pass","name":"Namespace lane on kind","state":"SUCCESS"},{"bucket":"pass","name":"common / Security and privacy","state":"SUCCESS"},{"bucket":"pass","name":"Classify the change / Changed paths","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (python)","state":"SUCCESS"}]"#;

const PR_CANCELLED: &str = r#"[{"bucket":"fail","name":"Full gate","state":"FAILURE"},{"bucket":"pass","name":"Analyze (python)","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (javascript-typescript)","state":"SUCCESS"},{"bucket":"pass","name":"Analyze (actions)","state":"SUCCESS"},{"bucket":"skipping","name":"CodeQL","state":"NEUTRAL"},{"bucket":"pass","name":"Analyze (rust)","state":"SUCCESS"},{"bucket":"cancel","name":"Gate: tests and Clippy","state":"CANCELLED"},{"bucket":"cancel","name":"Gate: static checks","state":"CANCELLED"},{"bucket":"cancel","name":"Pod lane","state":"CANCELLED"},{"bucket":"cancel","name":"common / Security and privacy","state":"CANCELLED"},{"bucket":"cancel","name":"Host crate on macOS","state":"CANCELLED"},{"bucket":"pass","name":"Classify the change / Changed paths","state":"SUCCESS"},{"bucket":"cancel","name":"Build documentation","state":"CANCELLED"},{"bucket":"cancel","name":"Classify the change / Changed paths","state":"CANCELLED"}]"#;

const PR_PENDING: &str = r#"[{"bucket":"pass","name":"Classify the change / Changed paths","state":"SUCCESS"},{"bucket":"pending","name":"Full gate","state":"IN_PROGRESS"},{"bucket":"skipping","name":"Pod lane","state":"SKIPPED"}]"#;

const RUNS_GREEN: &str = r#"{"check_runs":[{"conclusion":"success","name":"CodeQL","status":"completed"},{"conclusion":"skipped","name":"Pod lane","status":"completed"},{"conclusion":"skipped","name":"Gate: tests and Clippy","status":"completed"},{"conclusion":"skipped","name":"Gate: static checks","status":"completed"},{"conclusion":"success","name":"Full gate","status":"completed"},{"conclusion":"skipped","name":"Host crate on macOS","status":"completed"},{"conclusion":"success","name":"common / Security and privacy","status":"completed"},{"conclusion":"success","name":"Build documentation","status":"completed"},{"conclusion":"success","name":"Classify the change / Changed paths","status":"completed"},{"conclusion":"success","name":"Classify the change / Changed paths","status":"completed"},{"conclusion":"success","name":"Analyze (javascript-typescript)","status":"completed"},{"conclusion":"success","name":"Analyze (actions)","status":"completed"},{"conclusion":"success","name":"Analyze (rust)","status":"completed"},{"conclusion":"success","name":"Analyze (python)","status":"completed"}],"total_count":14}"#;

const RUNS_RED: &str = r#"{"check_runs":[{"conclusion":"success","name":"Full gate","status":"completed"},{"conclusion":"success","name":"Classify the change / Changed paths","status":"completed"},{"conclusion":"success","name":"Gate: static checks","status":"completed"},{"conclusion":"success","name":"Pod lane","status":"completed"},{"conclusion":"success","name":"Gate: tests and Clippy","status":"completed"},{"conclusion":"success","name":"publish / publish","status":"completed"},{"conclusion":"success","name":"Host crate on macOS","status":"completed"},{"conclusion":"success","name":"Chart cluster scenarios","status":"completed"},{"conclusion":"success","name":"Classify the change / Changed paths","status":"completed"},{"conclusion":"success","name":"Chart lane with node profiles","status":"completed"},{"conclusion":"success","name":"Quota lane","status":"completed"},{"conclusion":"success","name":"Confinement lane","status":"completed"},{"conclusion":"success","name":"Namespace lane on kind","status":"completed"},{"conclusion":"success","name":"common / Security and privacy","status":"completed"},{"conclusion":"success","name":"Build documentation","status":"completed"},{"conclusion":"success","name":"Namespace lane static checks","status":"completed"},{"conclusion":"success","name":"Analyze (python)","status":"completed"},{"conclusion":"failure","name":"Analyze (rust)","status":"completed"},{"conclusion":"success","name":"Analyze (actions)","status":"completed"},{"conclusion":"success","name":"Chart static checks","status":"completed"},{"conclusion":"failure","name":"Analyze (javascript-typescript)","status":"completed"}],"total_count":21}"#;

const RUNS_PENDING: &str = r#"{"check_runs":[{"conclusion":"success","name":"Classify the change / Changed paths","status":"completed"},{"conclusion":null,"name":"Full gate","status":"in_progress"},{"conclusion":null,"name":"Pod lane","status":"queued"}],"total_count":3}"#;

/// Two pages of check runs as `gh api --paginate` prints them, one after the other.
const RUNS_TWO_PAGES: &str = r#"{"check_runs":[{"conclusion":"success","name":"Full gate","status":"completed"}],"total_count":2}
{"check_runs":[{"conclusion":"timed_out","name":"Pod lane","status":"completed"}],"total_count":2}"#;

fn script() -> std::path::PathBuf {
    repository_root().join("scripts/wait-checks.sh")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Runs the script with `arguments` and `stdin` on standard input, with `bin` first on `PATH`.
fn run_with(bin: Option<&Path>, arguments: &[&str], stdin: &str) -> Output {
    let path = bin.map_or_else(
        || "/usr/bin:/bin".to_owned(),
        |bin| format!("{}:/usr/bin:/bin", bin.display()),
    );
    let mut child = Command::new("sh")
        .arg(script())
        .args(arguments)
        .env_clear()
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sh runs wait-checks.sh");
    child
        .stdin
        .take()
        .expect("standard input")
        .write_all(stdin.as_bytes())
        .expect("the list is written");
    child.wait_with_output().expect("wait-checks.sh ends")
}

/// The script's exit status and standard output for `list`, read from a file by `--from`.
fn from_file(list: &str) -> (Option<i32>, String) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let file = tree.path().join("checks.json");
    fs::write(&file, list).expect("the recorded list");
    let output = run_with(None, &["--from", file.to_str().expect("UTF-8")], "");
    (output.status.code(), text(&output.stdout))
}

#[test]
fn a_green_check_list_exits_zero_and_prints_every_check() {
    for (form, list, count) in [
        ("pull request", PR_GREEN, 14),
        ("check runs", RUNS_GREEN, 14),
    ] {
        let (code, stdout) = from_file(list);
        assert_eq!(code, Some(0), "{form}: {stdout}");
        assert_eq!(stdout.lines().count(), count, "{form}: {stdout}");
        assert!(
            stdout.lines().any(|line| line.ends_with(" Full gate")),
            "{form}: {stdout}"
        );
        assert!(stdout.contains("skipped"), "{form}: {stdout}");
    }
}

#[test]
fn a_red_check_list_exits_one_and_names_the_red_check() {
    for (form, list, red) in [
        (
            "pull request, one failure",
            PR_ONE_FAILURE,
            "failure          CodeQL",
        ),
        (
            "pull request, cancelled",
            PR_CANCELLED,
            "cancelled        Pod lane",
        ),
        ("check runs", RUNS_RED, "failure          Analyze (rust)"),
        (
            "two pages of check runs",
            RUNS_TWO_PAGES,
            "timed_out        Pod lane",
        ),
    ] {
        let (code, stdout) = from_file(list);
        assert_eq!(code, Some(1), "{form}: {stdout}");
        assert!(stdout.lines().any(|line| line == red), "{form}: {stdout}");
    }
}

#[test]
fn a_pending_or_empty_check_list_exits_three() {
    for (form, list) in [
        ("pull request", PR_PENDING),
        ("check runs", RUNS_PENDING),
        ("no check", "[]"),
        ("no input", ""),
    ] {
        let (code, stdout) = from_file(list);
        assert_eq!(code, Some(3), "{form}: {stdout}");
    }
}

#[test]
fn a_list_of_neither_form_and_a_bad_invocation_exit_two() {
    for list in [r#"{"checks": []}"#, "[1]", "not json"] {
        let (code, stdout) = from_file(list);
        assert_eq!(code, Some(2), "{list}: {stdout}");
    }
    for arguments in [
        &[][..],
        &["--commit", "not-a-sha"],
        &["12", "--commit", "abc123"],
        &["--interval", "soon", "12"],
        &["--from"],
    ] {
        let output = run_with(None, arguments, "");
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
    }
}

#[test]
fn a_list_on_standard_input_is_decided_as_one_from_a_file() {
    let output = run_with(None, &["--from", "-"], PR_ONE_FAILURE);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    let output = run_with(None, &["--from", "-"], RUNS_GREEN);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
}

/// A stand-in `gh` that answers its n-th call with the n-th of `answers` (the last one again
/// after that) and records its arguments, and the script's output with `arguments`.
fn wait(answers: &[&str], arguments: &[&str]) -> (Output, Vec<String>) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let bin = tree.path().join("bin");
    fs::create_dir(&bin).expect("the stand-in directory");
    for (index, answer) in answers.iter().enumerate() {
        fs::write(tree.path().join(format!("answer-{}", index + 1)), answer).expect("an answer");
    }
    let calls = tree.path().join("calls");
    executable(
        &bin.join("gh"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\nn=$(wc -l < '{calls}')\n\
             [ \"$n\" -le {last} ] || n={last}\ncat '{root}/answer-'\"$n\"\n",
            calls = calls.display(),
            last = answers.len(),
            root = tree.path().display(),
        ),
    );
    let output = run_with(Some(&bin), arguments, "");
    let calls = fs::read_to_string(&calls)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    (output, calls)
}

#[test]
fn the_wait_asks_again_until_no_check_is_pending() {
    let (output, calls) = wait(
        &[PR_PENDING, PR_PENDING, PR_GREEN],
        &["--interval", "0", "303"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|call| call == "pr checks 303 --json name,state,bucket"),
        "{calls:?}"
    );

    let (output, calls) = wait(
        &[RUNS_PENDING, RUNS_RED],
        &["--interval", "0", "--commit", "224e1e5ffd"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls[0].starts_with("api --paginate repos/{owner}/{repo}/commits/224e1e5ffd/check-runs"),
        "{calls:?}"
    );
}

#[test]
fn the_wait_ends_with_three_when_a_check_is_still_pending_at_its_timeout() {
    let (output, calls) = wait(&[PR_PENDING], &["--interval", "0", "--timeout", "0", "12"]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(text(&output.stdout).contains("in_progress      Full gate"));
}
