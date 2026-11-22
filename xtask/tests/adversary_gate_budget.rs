//! Adversary cases for `story:gate-test-step-ends-within-two-minutes` (unit commit f29056ced).
//!
//! The unit replaced the gate's `cargo test --workspace` step with cargo-nextest under a budget.
//! Every case here executes `scripts/gate.sh`, `gate.yml`'s install step or the daemon lookup
//! against stand-ins, as `release_workflow.rs` does, and asserts what the gate did:
//!
//! - the first three cases are findings: the test run takes its thread count and its retries
//!   from the caller's environment and not from `.config/nextest.toml`, and an empty
//!   `SUBSTRATE_TEST_DAEMON` names a daemon;
//! - the others pin exit paths no case held, each of which a mutant of `scripts/gate.sh` or of
//!   `gate.yml` survived: a failed test build, failed doctests, a failed no-default lane, a run a
//!   signal ended, a failed run without a report, and an archive of the runner whose digest is not
//!   the pinned one; and one holds `slowest-tests` to the report of a real failed run, which two
//!   mutants of `slowest_tests.rs` no longer read while its own cases stayed green.
//!
//! `ADVERSARY_GATE_SCRIPT` names another gate script to run in the repository and
//! `ADVERSARY_GATE_WORKFLOW` another `gate.yml` to read the install step from, which is how a
//! mutated copy is run against these cases without touching the one under attack.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::Digest as _;

#[path = "../../test-support/daemon_binary.rs"]
mod daemon_binary;

const GATE_WORKFLOW: &str = include_str!("../../.github/workflows/gate.yml");

/// A `cargo` that stands in for every command of the gate's tests phase. The workspace's test run
/// writes what it was started with to `STUB_RUN_RECORD` and the report cargo-nextest would write,
/// unless `STUB_REPORT` is `none`; each step exits with the status its variable names.
const CARGO_STUB: &str = r#"#!/bin/sh
case " $* " in
  " nextest run --workspace "*" --no-run "*) exit "${STUB_BUILD_STATUS:-0}" ;;
  " nextest run --workspace "*)
    {
      printf 'arguments=%s\n' "$*"
      printf 'NEXTEST_TEST_THREADS=%s\n' "${NEXTEST_TEST_THREADS-<unset>}"
      printf 'NEXTEST_RETRIES=%s\n' "${NEXTEST_RETRIES-<unset>}"
    } > "$STUB_RUN_RECORD"
    if [ "${STUB_REPORT:-written}" != none ]; then
      mkdir -p "$CARGO_TARGET_DIR/nextest/default"
      printf '%s\n' '<testsuites><testsuite name="stub::lane">' \
        '<testcase name="quick" classname="stub::lane" time="0.250"/>' \
        '<testcase name="slow" classname="stub::lane" time="12.500"/>' \
        '</testsuite></testsuites>' > "$CARGO_TARGET_DIR/nextest/default/junit.xml"
    fi
    if [ -n "${STUB_RUN_SIGNAL:-}" ]; then kill -s "$STUB_RUN_SIGNAL" $$; fi
    exit "${STUB_RUN_STATUS:-0}"
    ;;
  " test --workspace "*" --doc "*) exit "${STUB_DOC_STATUS:-0}" ;;
  " nextest run -p b10x-substrate-host --no-default-features "*) exit "${STUB_LANE_STATUS:-0}" ;;
esac
exit 0
"#;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a repository parent")
        .to_path_buf()
}

/// The gate script under attack, or the copy `ADVERSARY_GATE_SCRIPT` names.
fn gate_script() -> PathBuf {
    std::env::var_os("ADVERSARY_GATE_SCRIPT")
        .map_or_else(|| repository_root().join("scripts/gate.sh"), PathBuf::from)
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).expect("stand-in");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("stand-in mode");
}

/// What one stubbed run of the gate's tests phase did.
struct GateRun {
    status: Option<i32>,
    printed: String,
    errors: String,
    /// What the workspace's test run was started with, when it started.
    run_record: Option<String>,
}

impl GateRun {
    fn shown(&self) -> String {
        format!(
            "exit {:?}\n--- stdout\n{}--- stderr\n{}--- the test run was started with\n{}",
            self.status,
            self.printed,
            self.errors,
            self.run_record
                .as_deref()
                .unwrap_or("<it was not started>\n")
        )
    }

    /// The value the test run's record holds for `name`.
    fn recorded(&self, name: &str) -> &str {
        self.run_record
            .as_deref()
            .unwrap_or_else(|| panic!("the test run did not start:\n{}", self.shown()))
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("the record has no {name}"))
    }
}

/// Runs the gate's tests phase against [`CARGO_STUB`] with `environment` added to this process's.
fn gate_tests(environment: &[(&str, &str)]) -> GateRun {
    let tools = tempfile::tempdir().expect("stand-in directory");
    executable(&tools.path().join("cargo"), CARGO_STUB);
    executable(&tools.path().join("python3"), "#!/bin/sh\nexit 0\n");
    let target = tempfile::tempdir().expect("target directory");
    fs::create_dir(target.path().join("release")).expect("release directory");
    std::os::unix::fs::symlink(
        env!("CARGO_BIN_EXE_xtask"),
        target.path().join("release/xtask"),
    )
    .expect("xtask link");
    let record = tools.path().join("run-record");
    let mut path = OsString::from(tools.path());
    path.push(":");
    path.push(std::env::var_os("PATH").expect("PATH"));
    let output = Command::new("bash")
        .arg(gate_script())
        .arg("tests")
        .current_dir(repository_root())
        .env("PATH", path)
        .env("CARGO_TARGET_DIR", target.path())
        .env("STUB_RUN_RECORD", &record)
        // Whatever the runner of these cases set for itself is not the caller under test.
        .env_remove("NEXTEST_TEST_THREADS")
        .env_remove("NEXTEST_RETRIES")
        .envs(environment.iter().copied())
        .output()
        .expect("bash runs the gate");
    let shown = |bytes: Vec<u8>| {
        String::from_utf8(bytes)
            .expect("utf-8")
            .replace(target.path().to_str().expect("utf-8 target"), "<target>")
    };
    GateRun {
        status: output.status.code(),
        printed: shown(output.stdout),
        errors: shown(output.stderr),
        run_record: fs::read_to_string(&record).ok(),
    }
}

/// How the gate's line for the workspace's test run begins: the build's has `--no-run` here.
const TEST_RUN_STEP: &str =
    "gate: cargo nextest run --workspace --release --locked --test-threads ";
const SLOWEST: &str = "    12.500 s  stub::lane  slow\n";

// ---- findings ----

/// AGENTS.md, *The test step's budget*: "`test-threads = 12` is the same on every machine", and
/// `release_workflow.rs` holds the gate's line to "the thread count .config/nextest.toml holds".
/// cargo-nextest reads `NEXTEST_TEST_THREADS` before its configuration file, and the workstation's
/// build pool sets that variable for every job whose caller set none. The gate's run then takes
/// the pool's count: the bound was measured with twelve threads and is cancelled with four.
#[test]
fn adversary_the_test_run_takes_its_thread_count_from_the_repository_not_the_callers_environment() {
    let run = gate_tests(&[("NEXTEST_TEST_THREADS", "5")]);
    assert_eq!(run.status, Some(0), "{}", run.shown());
    let inherited = run.recorded("NEXTEST_TEST_THREADS");
    let named = run.recorded("arguments").contains("--test-threads");
    assert!(
        inherited == "<unset>" || named,
        "the gate's test run was started with NEXTEST_TEST_THREADS={inherited} from its caller \
         and names no thread count of its own, so cargo-nextest runs {inherited} tests at a \
         time and not the 12 `.config/nextest.toml` holds:\n{}",
        run.shown()
    );
}

/// A failed test fails the gate. cargo-nextest reads `NEXTEST_RETRIES` before its configuration
/// file and reports a test that passed on a later attempt as passed, with exit status 0; under
/// `cargo test` no variable of the caller could turn a failed test into a green gate.
#[test]
fn adversary_a_failed_test_is_not_retried_into_a_pass_by_the_callers_environment() {
    let run = gate_tests(&[("NEXTEST_RETRIES", "3")]);
    assert_eq!(run.status, Some(0), "{}", run.shown());
    let inherited = run.recorded("NEXTEST_RETRIES");
    let named = run.recorded("arguments").contains("--retries");
    assert!(
        inherited == "<unset>" || named,
        "the gate's test run was started with NEXTEST_RETRIES={inherited} from its caller and \
         names no retry count of its own, so a test that fails once and then passes leaves the \
         gate green:\n{}",
        run.shown()
    );
}

/// An empty variable names nothing: the CLI's own rule for `SUBSTRATE_SOCKET`
/// (`empty_socket_variable_is_no_socket`). An empty `SUBSTRATE_TEST_DAEMON` is taken as the
/// daemon's path instead, and the daemon beside the test is not looked for.
#[test]
fn adversary_an_empty_daemon_variable_names_no_daemon() {
    let profile = tempfile::tempdir().expect("profile directory");
    let root = profile.path().join("release");
    fs::create_dir_all(root.join("deps")).expect("deps directory");
    let sibling = root.join("substrate-daemon");
    fs::write(&sibling, b"").expect("sibling daemon");
    assert_eq!(
        daemon_binary::resolve(Some(OsString::new()), &root.join("deps/adversary-0123")),
        Ok(sibling),
        "an empty SUBSTRATE_TEST_DAEMON was taken as the daemon to start"
    );
}

// ---- exit paths no case held ----

/// The test build is the setup step every test relies on: when it fails the gate fails with its
/// status and starts no test.
#[test]
fn adversary_a_failed_test_build_fails_the_gate_and_no_test_starts() {
    let run = gate_tests(&[("STUB_BUILD_STATUS", "101")]);
    assert_eq!(run.status, Some(101), "{}", run.shown());
    assert!(
        run.printed.contains("gate-time: test build: "),
        "{}",
        run.shown()
    );
    assert!(run.run_record.is_none(), "{}", run.shown());
    assert!(
        !run.printed.contains(TEST_RUN_STEP) && !run.printed.contains("passed"),
        "{}",
        run.shown()
    );
}

/// The doctests are tests: when they fail the gate fails and runs nothing after them.
#[test]
fn adversary_failed_doctests_fail_the_gate() {
    let run = gate_tests(&[("STUB_DOC_STATUS", "101")]);
    assert_eq!(run.status, Some(101), "{}", run.shown());
    assert!(
        run.printed.contains("gate-time: doctests: "),
        "{}",
        run.shown()
    );
    assert!(
        !run.printed.contains("clippy") && !run.printed.contains("passed"),
        "{}",
        run.shown()
    );
}

/// The no-default lane runs under cargo-nextest too: when it fails the gate fails.
#[test]
fn adversary_a_failed_no_default_lane_fails_the_gate() {
    let run = gate_tests(&[("STUB_LANE_STATUS", "100")]);
    assert_eq!(run.status, Some(100), "{}", run.shown());
    assert!(
        run.printed
            .contains("gate-time: no-default lane, its build included: "),
        "{}",
        run.shown()
    );
    assert!(!run.printed.contains("passed"), "{}", run.shown());
}

/// A test run a signal ended (the runner killed for memory, a cancelled job) fails the gate, after
/// the slowest tests of what it reported and before the doctests.
#[test]
fn adversary_a_test_run_a_signal_ended_fails_the_gate() {
    let run = gate_tests(&[("STUB_RUN_SIGNAL", "KILL")]);
    assert_eq!(run.status, Some(137), "{}", run.shown());
    assert!(run.printed.contains(SLOWEST), "{}", run.shown());
    assert!(
        !run.printed.contains("--doc") && !run.printed.contains("passed"),
        "{}",
        run.shown()
    );
}

/// A failed run that wrote no report fails the gate with the run's own status, not with the
/// status of the verb that could not read a report.
#[test]
fn adversary_a_failed_run_without_a_report_fails_the_gate_with_the_runs_status() {
    let run = gate_tests(&[("STUB_RUN_STATUS", "100"), ("STUB_REPORT", "none")]);
    assert_eq!(run.status, Some(100), "{}", run.shown());
    assert!(run.errors.contains("no JUnit report at"), "{}", run.shown());
    assert!(
        !run.printed.contains("--doc") && !run.printed.contains("passed"),
        "{}",
        run.shown()
    );
}

// ---- the report of a real run ----

/// What cargo-nextest 0.9.143 wrote for a test that panicked, recorded from a run and shortened:
/// the panic message is an attribute and names the thread between `&apos;`, as every panic
/// message does. The second case is one the runner timed at less than half a millisecond.
const FAILED_RUN_REPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" skipped="0" failures="1" errors="0" uuid="58856b39-154c-44f9-bdc9-a586a09a699c" timestamp="2026-11-22T01:01:31.540+01:00" time="0.008">
    <testsuite name="budget-probe" tests="2" skipped="0" errors="0" failures="1">
        <testcase name="probes::nasty_failure" classname="budget-probe" timestamp="2026-11-22T01:01:31.540+01:00" time="0.007">
            <failure message="thread &apos;probes::nasty_failure&apos; (1089360) panicked at src/lib.rs:53:9" type="test failure with exit code 101">thread &apos;probes::nasty_failure&apos; (1089360) panicked at src/lib.rs:53:9:
it&apos;s a &lt;failure message=&quot;x&quot;&gt; &amp; more ]]&gt;
 second line &apos;single&apos;</failure>
            <system-out>
stdout: &lt;testcase name=&quot;forged&quot; classname=&quot;x&quot; time=&quot;999&quot;/&gt; ]]&gt; &amp; &apos; &quot;
</system-out>
            <system-err>stderr: &lt;/testcase&gt;&lt;/testsuite&gt; &lt;failure&gt; it&apos;s &quot;quoted&quot;
</system-err>
        </testcase>
        <testcase name="probes::instant" classname="budget-probe" timestamp="2026-11-22T01:01:31.540+01:00" time="0.000"/>
    </testsuite>
</testsuites>
"#;

/// The verb's own cases hold no `&apos;` and no zero time, and a `slowest_tests.rs` that refuses
/// either still passes them. A failed run is the one whose slowest tests the gate must still name.
#[test]
fn adversary_the_report_of_a_real_failed_run_is_read() {
    let directory = tempfile::tempdir().expect("report directory");
    let report = directory.path().join("junit.xml");
    fs::write(&report, FAILED_RUN_REPORT).expect("report");
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("slowest-tests")
        .arg("--junit")
        .arg(&report)
        .output()
        .expect("xtask runs");
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).as_ref(),
            String::from_utf8_lossy(&output.stderr).as_ref()
        ),
        (
            Some(0),
            "slowest 2 of 2 tests (the 2 test times sum to 0.007 s):\n\
             \x20    0.007 s  budget-probe  probes::nasty_failure  FAILED\n\
             \x20    0.000 s  budget-probe  probes::instant\n",
            ""
        )
    );
}

// ---- the pinned runner ----

/// `gate.yml`, or the copy `ADVERSARY_GATE_WORKFLOW` names.
fn gate_workflow() -> String {
    std::env::var_os("ADVERSARY_GATE_WORKFLOW").map_or_else(
        || GATE_WORKFLOW.to_owned(),
        |path| fs::read_to_string(path).expect("the named workflow is readable"),
    )
}

/// The script of the `gate.yml` step named `name`, as the runner would write it to a file.
fn workflow_step_script(name: &str) -> String {
    let workflow = gate_workflow();
    let header = format!("      - name: {name}\n");
    let step = &workflow[workflow
        .find(&header)
        .unwrap_or_else(|| panic!("gate.yml has no step {name}"))
        + header.len()..];
    let step = &step[..step.find("\n      - name: ").unwrap_or(step.len())];
    let body = &step[step
        .find("        run: |\n")
        .expect("the step runs a script")
        + 15..];
    let mut script = String::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            script.push('\n');
        } else if let Some(line) = line.strip_prefix("          ") {
            script.push_str(line);
            script.push('\n');
        } else {
            break;
        }
    }
    script
}

/// One `NAME: value` of `gate.yml`.
fn workflow_value(name: &str) -> String {
    gate_workflow()
        .lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("gate.yml has no {name}"))
        .to_owned()
}

/// What the install step did with the archive a stand-in `curl` served.
struct Install {
    status: Option<i32>,
    shown: String,
    /// Whether the served archive's `cargo-nextest` ran.
    ran: bool,
    /// Whether anything of the archive was unpacked where a later step finds it.
    unpacked: bool,
    /// Whether the step put a directory on the path of the steps after it.
    on_path: bool,
}

/// Runs the install step as the hosted runner does (`bash -e`), with a `curl` that serves an
/// archive holding a `cargo-nextest` of this test's own. The pinned digest is the one `gate.yml`
/// pins, or the served archive's own when `pin_the_served_archive`.
fn install(pin_the_served_archive: bool) -> Install {
    let work = tempfile::tempdir().expect("work directory");
    let marker = work.path().join("ran");
    let payload = work.path().join("payload");
    fs::create_dir(&payload).expect("payload directory");
    executable(
        &payload.join("cargo-nextest"),
        &format!(
            "#!/bin/sh\necho ran > '{}'\necho 'cargo-nextest 0.0.0 (stand-in)'\n",
            marker.display()
        ),
    );
    let archive = work.path().join("served.tar.gz");
    let packed = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&payload)
        .arg("cargo-nextest")
        .status()
        .expect("tar runs");
    assert!(packed.success(), "the stand-in archive was not packed");
    let tools = work.path().join("tools");
    fs::create_dir(&tools).expect("tools directory");
    executable(
        &tools.join("curl"),
        &format!(
            "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = --output ]; then cp '{}' \"$2\"; exit 0; fi\n  shift\ndone\nexit 2\n",
            archive.display()
        ),
    );
    let runner_temp = work.path().join("runner-temp");
    fs::create_dir(&runner_temp).expect("runner temp");
    let github_path = work.path().join("github-path");
    fs::write(&github_path, b"").expect("GITHUB_PATH file");
    let script = work.path().join("step.sh");
    fs::write(
        &script,
        workflow_step_script("Install the pinned cargo-nextest release"),
    )
    .expect("step script");
    let digest = if pin_the_served_archive {
        hex::encode(sha2::Sha256::digest(
            fs::read(&archive).expect("served archive"),
        ))
    } else {
        workflow_value("NEXTEST_SHA256")
    };
    let mut path = OsString::from(&tools);
    path.push(":");
    path.push(std::env::var_os("PATH").expect("PATH"));
    let output = Command::new("bash")
        .arg("-e")
        .arg(&script)
        .current_dir(work.path())
        .env("PATH", path)
        .env("RUNNER_TEMP", &runner_temp)
        .env("GITHUB_PATH", &github_path)
        .env("NEXTEST_VERSION", workflow_value("NEXTEST_VERSION"))
        .env("NEXTEST_SHA256", digest)
        .output()
        .expect("bash runs the step");
    Install {
        status: output.status.code(),
        shown: format!(
            "exit {:?}\n{}{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        ran: marker.exists(),
        unpacked: runner_temp.join("nextest-bin/cargo-nextest").exists(),
        on_path: !fs::read_to_string(&github_path)
            .expect("GITHUB_PATH file")
            .is_empty(),
    }
}

/// `release_workflow.rs` holds the step to containing a `sha256sum --check` line. This runs the
/// step: an archive whose digest is not the pinned one fails the job before anything of it is
/// unpacked, put on the path or run. The control serves the same archive under its own digest and
/// the step then unpacks and runs it, so the refusal above is the digest's and not this harness's.
#[test]
fn adversary_an_archive_that_is_not_the_pinned_one_fails_the_job_before_anything_of_it_runs() {
    let refused = install(false);
    assert_ne!(refused.status, Some(0), "{}", refused.shown);
    assert!(
        refused.shown.contains("FAILED") || refused.shown.contains("did NOT match"),
        "the step did not fail at the digest:\n{}",
        refused.shown
    );
    assert!(
        !refused.ran && !refused.unpacked && !refused.on_path,
        "an archive that is not the pinned one was used before it was refused (ran: {}, \
         unpacked: {}, on the path: {}):\n{}",
        refused.ran,
        refused.unpacked,
        refused.on_path,
        refused.shown
    );

    let control = install(true);
    assert_eq!(control.status, Some(0), "{}", control.shown);
    assert!(
        control.ran && control.unpacked && control.on_path,
        "{}",
        control.shown
    );
}
