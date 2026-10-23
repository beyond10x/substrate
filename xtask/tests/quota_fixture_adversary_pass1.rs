//! Adversary pass 1 on `story:project-quota-lane-has-a-provisioned-runner`.
//!
//! Two halves. The release step `Require a recorded quota-lane run for this commit` is executed
//! exactly as `release.yml` carries it, against a throwaway tagged repository. The host side of
//! `cargo xtask quota-fixture run` is executed through the shipped binary with a stand-in `docker`
//! on `PATH`, so what it records can be observed without a container: the stand-in answers
//! `info`, `build` and `run` the way a green container would, and logs every argv it received.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");
const QUOTA_STEP: &str = "Require a recorded quota-lane run for this commit";
const QUOTA_SOURCE: &str = "crates/substrate-host/src/git/quota_tests.rs";
const TAG: &str = "9.9.9";
const AT: &str = "2026-10-23T10:00:00Z";

fn step_script(name: &str) -> String {
    let start = WORKFLOW
        .find(&format!("- name: {name}\n"))
        .unwrap_or_else(|| panic!("release workflow has no step {name:?}"));
    let mut lines = WORKFLOW[start..].lines().skip(1);
    for line in lines.by_ref() {
        if line.trim() == "run: |" {
            break;
        }
    }
    let mut body = Vec::new();
    let mut indent = None;
    for line in lines {
        if line.trim().is_empty() {
            body.push(String::new());
            continue;
        }
        let width = line.len() - line.trim_start().len();
        let indent = *indent.get_or_insert(width);
        if width < indent {
            break;
        }
        body.push(line[indent..].to_owned());
    }
    body.join("\n") + "\n"
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=adversary",
            "-c",
            "user.email=adversary@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

fn quota_source(cases: usize) -> String {
    let mut source = String::from("use super::*;\n");
    for case in 0..cases {
        write!(
            source,
            "\n#[tokio::test]\n#[ignore = \"requires the explicitly delegated ext4 project-quota \
             fixture\"]\nasync fn real_quota_case_{case}() {{\n}}\n"
        )
        .expect("String write");
    }
    source
}

struct Tagged {
    directory: tempfile::TempDir,
    sha: String,
}

impl Tagged {
    fn new(source: &str, message: &str) -> Self {
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        git(root, &["init", "--quiet"]);
        let file = root.join(QUOTA_SOURCE);
        fs::create_dir_all(file.parent().expect("parent")).expect("tree");
        fs::write(&file, source).expect("source");
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "-m", "tagged source"]);
        let sha = git(root, &["rev-parse", "HEAD"]);
        let message = message.replace("{sha}", &sha);
        git(
            root,
            &[
                "tag",
                "--annotate",
                "--cleanup=verbatim",
                TAG,
                "-m",
                &message,
            ],
        );
        Self { directory, sha }
    }

    /// (success, stderr, `GITHUB_OUTPUT` contents)
    fn run(&self, input: &str) -> (bool, String, String) {
        let output_file = self.directory.path().join("github-output");
        fs::write(&output_file, "").expect("GITHUB_OUTPUT");
        let output = Command::new("bash")
            .current_dir(self.directory.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("TAG", TAG)
            .env("INPUT_EVIDENCE", input)
            .env("SOURCE_SHA", &self.sha)
            .env("GITHUB_OUTPUT", &output_file)
            .args(["-e", "-o", "pipefail", "-c", &step_script(QUOTA_STEP)])
            .output()
            .expect("bash runs");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            fs::read_to_string(&output_file).expect("GITHUB_OUTPUT"),
        )
    }
}

// ---------------------------------------------------------------------------------------------
// The release step.
// ---------------------------------------------------------------------------------------------

/// A tag carrying two `Quota-lane:` records that disagree is ambiguous evidence. The step takes the
/// first with `head -n 1` and never looks at the second, so the published record is whichever line
/// happens to come first.
#[test]
fn adv1_a_tag_with_two_conflicting_quota_lane_records_is_refused() {
    let source = Tagged::new(
        &quota_source(7),
        &format!(
            "release\n\nQuota-lane: {{sha}} 7 {AT}\nQuota-lane: \
             0123456789abcdef0123456789abcdef01234567 3 {AT}\n"
        ),
    );
    let (success, stderr, output) = source.run("");
    assert!(
        !success,
        "two disagreeing Quota-lane records were accepted as {output:?}; stderr: {stderr}"
    );
}

/// `release.yml` says it refuses "a timestamp that is not an RFC 3339 UTC instant". Month 13,
/// day 45 and 25:61:61 are not an instant in any calendar.
#[test]
fn adv2_a_calendar_impossible_timestamp_is_refused() {
    let source = Tagged::new(
        &quota_source(7),
        "release\n\nQuota-lane: {sha} 7 2026-13-45T25:61:61Z\n",
    );
    let (success, stderr, output) = source.run("");
    assert!(
        !success,
        "an impossible instant was accepted as {output:?}; stderr: {stderr}"
    );
    assert!(stderr.contains("RFC 3339 UTC instant"), "{stderr}");
}

/// The recovery dispatch's `quota_evidence` is a free string. The step validates only the first
/// line (`read -r`) and then appends the whole value to `GITHUB_OUTPUT`, so a second line
/// `evidence=…` overrides the validated output, and the release notes publish the forged value.
#[test]
fn adv3_a_multi_line_dispatch_input_cannot_replace_the_validated_evidence_output() {
    let source = Tagged::new(&quota_source(7), "release\n");
    let input = format!("{} 7 {AT}\nevidence=forged 0 never", source.sha);
    let (success, stderr, output) = source.run(&input);
    let evidence_lines: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with("evidence="))
        .collect();
    assert!(
        !success || evidence_lines == [format!("evidence={} 7 {AT}", source.sha).as_str()],
        "the step accepted a multi-line input and wrote {evidence_lines:?} to GITHUB_OUTPUT; the \
         runner keeps the last value, so the release publishes `forged 0 never`. stderr: {stderr}"
    );
}

/// The fixture records `declared_cases(...).len()` — every `#[ignore]`d function, whatever its
/// name — while the release step counts every `fn real_quota_*` line, ignored or not. A source with
/// one non-test helper named `real_quota_*` makes the step refuse the record the fixture printed
/// for that very source.
#[test]
fn adv4_the_release_step_counts_the_cases_the_fixture_counts() {
    let mut text = quota_source(7);
    text.push_str("\nfn real_quota_fixture_root() -> std::path::PathBuf {\n    todo!()\n}\n");
    // The count the fixture records for this source: seven ignored functions.
    let source = Tagged::new(&text, &format!("release\n\nQuota-lane: {{sha}} 7 {AT}\n"));
    let (success, stderr, _) = source.run("");
    assert!(
        success,
        "the release step refused the fixture's own count for this source: {stderr}"
    );
}

/// Attacks that did not break: an abbreviated or upper-case commit, a CR-terminated dispatch
/// value, an offset instead of `Z` and a zero-padded count are all refused.
#[test]
fn adv5_green_abbreviated_upper_case_cr_and_offset_records_are_refused() {
    let source = Tagged::new(&quota_source(7), "release\n");
    for input in [
        format!("{} 7 {AT}", &source.sha[..12]),
        format!("{} 7 {AT}", source.sha.to_uppercase()),
        format!("{} 7 {AT}\r", source.sha),
        format!("{} 7 2026-10-23T10:00:00+00:00", source.sha),
        format!("{} 07 {AT}", source.sha),
    ] {
        let (success, stderr, output) = source.run(&input);
        assert!(!success, "{input:?} accepted: {output} {stderr}");
        assert!(!output.contains("evidence="), "{output}");
    }
}

// ---------------------------------------------------------------------------------------------
// The host side of `cargo xtask quota-fixture run`, with a stand-in docker.
// ---------------------------------------------------------------------------------------------

struct Checkout {
    directory: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
}

impl Checkout {
    /// A workspace with seven ignored cases, committed clean. `on_run` is shell run by the stand-in
    /// `docker run` before it prints the green summary, with `$ROOT` set to the checkout.
    fn new(on_run: &str) -> Self {
        let directory = tempfile::tempdir().expect("checkout");
        let root = directory.path().join("substrate");
        fs::create_dir_all(root.join("fixtures/quota-lab")).expect("recipe dir");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").expect("manifest");
        fs::write(root.join("fixtures/quota-lab/Dockerfile"), "FROM scratch\n").expect("recipe");
        let file = root.join(QUOTA_SOURCE);
        fs::create_dir_all(file.parent().expect("parent")).expect("tree");
        fs::write(&file, quota_source(7)).expect("source");
        git(&root, &["init", "--quiet"]);
        git(&root, &["add", "--all"]);
        git(&root, &["commit", "--quiet", "-m", "source"]);

        let bin = directory.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let log = directory.path().join("docker.log");
        let script = format!(
            "#!/bin/sh\n\
             ROOT='{root}'\n\
             printf '%s\\n' \"--- $*\" >> '{log}'\n\
             case \"$1\" in\n\
               run)\n\
                 {on_run}\n\
                 echo 'quota lane: root /quota ids 200000-200511 on /quota (rw,prjquota)'\n\
                 echo 'quota lane: 7 declared, 7 ran, 0 failed'\n\
                 exit 0 ;;\n\
               *) exit 0 ;;\n\
             esac\n",
            root = root.display(),
            log = log.display(),
        );
        let docker = bin.join("docker");
        fs::write(&docker, script).expect("stand-in docker");
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).expect("chmod");
        Self {
            directory,
            bin,
            log,
        }
    }

    fn root(&self) -> PathBuf {
        self.directory.path().join("substrate")
    }

    /// (success, stdout, stderr)
    fn run(&self) -> (bool, String, String) {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["quota-fixture", "run"])
            .current_dir(self.root())
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("xtask runs");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn run_argv(&self) -> String {
        fs::read_to_string(&self.log)
            .expect("docker log")
            .lines()
            .find(|line| line.starts_with("--- run "))
            .expect("a docker run")
            .to_owned()
    }
}

/// Control: with nothing changing, the stand-in green container yields a record for HEAD. This is
/// what makes adv7 mean something.
#[test]
fn adv6_green_control_a_clean_green_run_prints_the_record() {
    let checkout = Checkout::new(":");
    let (success, stdout, stderr) = checkout.run();
    let head = git(&checkout.root(), &["rev-parse", "HEAD"]);
    assert!(success, "{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("Quota-lane: {head} 7 ")),
        "{stdout}{stderr}"
    );
}

/// The fixture reads `HEAD` and `git status --porcelain` once, before a build and run that take
/// minutes, against a live read-write checkout bind-mounted into the container. A tracked file
/// changed during the run is not seen, and the record names HEAD for bytes that are not HEAD's.
#[test]
fn adv7_a_checkout_changed_during_the_run_yields_no_record() {
    let checkout = Checkout::new(&format!(
        "echo '// edited mid-run' >> \"$ROOT/{QUOTA_SOURCE}\""
    ));
    let (_, stdout, stderr) = checkout.run();
    let dirty = git(&checkout.root(), &["status", "--porcelain"]);
    assert!(!dirty.is_empty(), "the stand-in did not dirty the tree");
    assert!(
        !stdout.contains("Quota-lane:"),
        "a release record was printed for a checkout that was dirty when the run ended \
         ({dirty}):\n{stdout}{stderr}"
    );
}

/// Every checkout on the host mounts the same named volume as the container's `CARGO_TARGET_DIR`,
/// and every checkout appears at the same path `/src` inside it. Cargo's freshness for path
/// packages is mtime-based, so a checkout whose files are older than another checkout's last build
/// in that volume is judged fresh and the lane runs the other checkout's binaries under this
/// checkout's record.
#[test]
fn adv8_two_checkouts_do_not_share_one_container_build_target() {
    let first = Checkout::new(":");
    let second = Checkout::new(":");
    first.run();
    second.run();
    let volume = |argv: &str| -> String {
        argv.split(' ')
            .find(|word| word.starts_with("type=volume,") && word.ends_with("target=/target"))
            .unwrap_or_else(|| panic!("no target volume in {argv}"))
            .to_owned()
    };
    let (a, b) = (volume(&first.run_argv()), volume(&second.run_argv()));
    assert_ne!(
        a, b,
        "two different checkouts build into the same container target volume"
    );
}
