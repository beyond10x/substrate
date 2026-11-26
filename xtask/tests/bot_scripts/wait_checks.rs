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

/// What a bounded wait came to: the exit status (`None` when the wait did not end within its
/// bound and was killed), standard error and the stand-ins' calls.
struct Bounded {
    code: Option<i32>,
    ended: bool,
    stderr: String,
    calls: Vec<String>,
}

/// Runs the script under `shell` with `arguments`, a stand-in `gh` that answers its n-th call
/// with the n-th of `answers` (the last one again after that; an answer `FAIL` exits 1 with
/// gh's message), and a stand-in `sleep` that records its argument and returns at once. The
/// run is killed when it has not ended after `bound`.
fn bounded(
    shell: &str,
    answers: &[&str],
    arguments: &[&str],
    bound: std::time::Duration,
) -> Bounded {
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
            "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> '{calls}'\nn=$(grep -c '^gh ' '{calls}')\n\
             [ \"$n\" -le {last} ] || n={last}\nanswer='{root}/answer-'\"$n\"\n\
             if [ \"$(cat \"$answer\")\" = FAIL ]; then\n\
             echo 'GraphQL: Could not resolve to a PullRequest with the number of 999999.' >&2\n\
             exit 1\nfi\ncat \"$answer\"\n",
            calls = calls.display(),
            last = answers.len(),
            root = tree.path().display(),
        ),
    );
    executable(
        &bin.join("sleep"),
        &format!(
            "#!/bin/sh\nprintf 'sleep %s\\n' \"$*\" >> '{calls}'\n",
            calls = calls.display()
        ),
    );
    let mut child = Command::new(shell)
        .arg(script())
        .args(arguments)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shell runs wait-checks.sh");
    let started = std::time::Instant::now();
    let ended = loop {
        if child.try_wait().expect("the wait's state").is_some() {
            break true;
        }
        if started.elapsed() > bound {
            child.kill().expect("the wait is killed");
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let output = child.wait_with_output().expect("wait-checks.sh ends");
    let calls = fs::read_to_string(&calls)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    Bounded {
        code: if ended { output.status.code() } else { None },
        ended,
        stderr: text(&output.stderr),
        calls,
    }
}

/// `sh`, and `dash` and `bash` where the host has them: the script says it is POSIX sh.
fn shells() -> Vec<String> {
    let mut shells = vec!["sh".to_owned()];
    for name in ["dash", "bash"] {
        if let Some(path) = ["/usr/bin", "/bin"]
            .iter()
            .map(|directory| Path::new(directory).join(name))
            .find(|path| path.is_file())
        {
            shells.push(path.to_str().expect("UTF-8").to_owned());
        }
    }
    shells
}

/// A check that ended at startup, or went stale, has ended and did not succeed: the header
/// names `stale` among the outcomes that exit 1, and the commit form exits 1 for both. In the
/// pull-request form `gh pr checks --json name,state,bucket` (gh 2.102, `aggregateChecks`)
/// files `STARTUP_FAILURE` and `STALE` under the bucket `pending`, so a status read from the
/// bucket alone waits for a check that will never run again and ends with 3 at the timeout.
#[test]
fn a_startup_failure_or_a_stale_check_exits_one_in_both_forms() {
    for (form, list) in [
        (
            "check runs, startup_failure",
            r#"{"check_runs":[{"conclusion":"startup_failure","name":"Gate: tests and Clippy","status":"completed"}],"total_count":1}"#,
        ),
        (
            "check runs, stale",
            r#"{"check_runs":[{"conclusion":"stale","name":"Gate: tests and Clippy","status":"completed"}],"total_count":1}"#,
        ),
        (
            "pull request, STARTUP_FAILURE as gh buckets it",
            r#"[{"bucket":"pending","name":"Gate: tests and Clippy","state":"STARTUP_FAILURE"}]"#,
        ),
        (
            "pull request, STALE as gh buckets it",
            r#"[{"bucket":"pending","name":"Gate: tests and Clippy","state":"STALE"}]"#,
        ),
    ] {
        let (code, stdout) = from_file(list);
        assert_eq!(code, Some(1), "{form}: {stdout}");
    }
}

/// `--interval 0` is accepted, and `--timeout` still bounds the wait: a check that stays
/// pending ends the wait with 3 once the timeout has passed, whatever the interval.
#[test]
fn a_zero_interval_still_ends_at_the_timeout() {
    let run = bounded(
        "sh",
        &[PR_PENDING],
        &["--interval", "0", "--timeout", "1", "12"],
        std::time::Duration::from_secs(3),
    );
    assert!(
        run.ended,
        "the wait did not end within 3 s; gh was asked {} times",
        run.calls
            .iter()
            .filter(|call| call.starts_with("gh "))
            .count()
    );
    assert_eq!(run.code, Some(3), "{}", run.stderr);
}

/// An interval written with a leading zero is the number its digits name, as `number` admits
/// it: one pending answer, one sleep, one green answer, exit 0, in every shell.
#[test]
fn an_interval_with_a_leading_zero_waits_and_decides_like_its_number() {
    for shell in shells() {
        let run = bounded(
            &shell,
            &[PR_PENDING, PR_GREEN],
            &["--interval", "08", "--timeout", "100", "303"],
            std::time::Duration::from_secs(10),
        );
        assert!(run.ended, "{shell}: {:?}", run.calls);
        assert_eq!(
            run.code,
            Some(0),
            "{shell}: {}\n{:?}",
            run.stderr,
            run.calls
        );
    }
}

/// A pull request `gh` cannot read on any ask (a number that names none, no login, an API
/// that refuses) is not a pull request whose checks have not started: the status says the
/// list was unreadable (2), not "no check is reported, ask again" (3).
#[test]
fn a_pull_request_gh_never_reads_is_not_reported_as_one_without_checks() {
    let run = bounded(
        "sh",
        &["FAIL"],
        &["--interval", "1", "--timeout", "3", "999999"],
        std::time::Duration::from_secs(5),
    );
    assert!(run.ended, "{:?}", run.calls);
    assert_eq!(run.code, Some(2), "{}\n{:?}", run.stderr, run.calls);
}

/// The asks gh answered `FAIL` end the wait with 2 only when they are the last ones: three in
/// a row, or the last ask before the timeout. Its last error line is printed; a failed ask
/// followed by a list is decided by the list.
#[test]
fn gh_failing_ends_the_wait_with_two_only_when_its_last_asks_failed() {
    let bound = std::time::Duration::from_secs(5);
    let run = bounded(
        "sh",
        &["FAIL", "FAIL", PR_GREEN],
        &["--interval", "1", "--timeout", "100", "5"],
        bound,
    );
    assert_eq!(run.code, Some(0), "{}\n{:?}", run.stderr, run.calls);

    let run = bounded(
        "sh",
        &["FAIL"],
        &["--interval", "1", "--timeout", "0", "5"],
        bound,
    );
    assert_eq!(run.code, Some(2), "{}\n{:?}", run.stderr, run.calls);
    assert_eq!(run.calls.len(), 1, "{:?}", run.calls);
    assert!(
        run.stderr
            .contains("Could not resolve to a PullRequest with the number of 999999."),
        "{}",
        run.stderr
    );

    let run = bounded(
        "sh",
        &["FAIL", "FAIL", "FAIL", PR_GREEN],
        &["--interval", "1", "--timeout", "100", "5"],
        bound,
    );
    assert_eq!(run.code, Some(2), "{}\n{:?}", run.stderr, run.calls);
    let asks = run
        .calls
        .iter()
        .filter(|call| call.starts_with("gh "))
        .count();
    assert_eq!(asks, 3, "{:?}", run.calls);
}

/// gh's answer for a pull request whose checks have not started (`no checks reported on the
/// '<branch>' branch`, exit 1) is a list without checks, not an unreadable one: the wait goes on
/// and ends with 3 at its timeout.
#[test]
fn a_pull_request_whose_checks_have_not_started_waits_and_ends_with_three() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let bin = tree.path().join("bin");
    fs::create_dir(&bin).expect("the stand-in directory");
    let calls = tree.path().join("calls");
    executable(
        &bin.join("gh"),
        &format!(
            "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> '{calls}'\n\
             echo \"no checks reported on the 'story/x' branch\" >&2\nexit 1\n",
            calls = calls.display()
        ),
    );
    executable(
        &bin.join("sleep"),
        &format!(
            "#!/bin/sh\nprintf 'sleep %s\\n' \"$*\" >> '{calls}'\n",
            calls = calls.display()
        ),
    );
    let output = run_with(Some(&bin), &["--interval", "1", "--timeout", "4", "5"], "");
    let calls = fs::read_to_string(&calls).expect("the calls");
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert_eq!(calls.matches("gh ").count(), 5, "{calls}");
}

/// A sha is 7 to 40 lower-case hex digits, and a number at most nine decimal digits after its
/// leading zeros; anything else is a usage error before gh is asked.
#[test]
fn a_sha_or_a_number_out_of_its_form_exits_two() {
    for arguments in [
        &["--commit", "abc123"][..],
        &["--commit", &"a".repeat(41)],
        &["--commit", "ABCDEF1"],
        &["--interval", "1234567890", "12"],
        &["--timeout", "99999999999999999999", "12"],
        &["1234567890"],
    ] {
        let tree = tempfile::tempdir().expect("a temporary directory");
        let asked = tree.path().join("asked");
        for name in ["gh", "sleep"] {
            executable(
                &tree.path().join(name),
                &format!(
                    "#!/bin/sh\necho {name} >> '{asked}'\nexit 1\n",
                    asked = asked.display()
                ),
            );
        }
        let output = run_with(Some(tree.path()), arguments, "");
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(!asked.exists(), "{arguments:?}: gh or sleep ran");
    }
    for arguments in [
        &[
            "--interval",
            "000000000001",
            "--timeout",
            "0",
            "--commit",
            "abcdef1",
        ][..],
        &["--timeout", "0", "--commit", &"a".repeat(40)],
    ] {
        let tree = tempfile::tempdir().expect("a temporary directory");
        executable(
            &tree.path().join("gh"),
            &format!("#!/bin/sh\nprintf '%s' '{RUNS_GREEN}'\n"),
        );
        let output = run_with(Some(tree.path()), arguments, "");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
}
