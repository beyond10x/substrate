//! The real project-quota lane.
//!
//! `crates/substrate-host/src/git/quota_tests.rs` carries seven `real_quota_*` cases that need a
//! filesystem mounted with project quotas and an exclusive project-identity range. They are
//! `#[ignore]`d, so `cargo test` never selects them — and neither `scripts/gate.sh` nor
//! `scripts/delegated-lane.sh` passes `--ignored`, so no script ran them even on a host that had
//! the fixture. The only recorded execution is a hand-typed `docker exec` against a quota-lab
//! image (`.engineering/planning/review-result/git-workspace-quota-lifecycle-pass-1.md`, the three
//! `docker exec … --ignored` commands).
//!
//! This verb is that invocation, repeatable. It **refuses by name** when a prerequisite is missing
//! rather than reporting nothing, and it names every case it ran and every case it did not — so a
//! green run cannot be read as covering cases that never executed. `--inventory` prints the same
//! statement without running anything, which is how `scripts/delegated-lane.sh` says what it does
//! not cover.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context as _, Result};
use clap::Args as ClapArgs;

/// Where the cases are declared, relative to the workspace root.
const SOURCE: &str = "crates/substrate-host/src/git/quota_tests.rs";
/// The package whose library test binary carries them.
const PACKAGE: &str = "b10x-substrate-host";
/// The module path the harness names them by; `--exact` is built from it.
const MODULE: &str = "git::materialization_tests::quota_tests";
/// `Fixture::open` asserts `range.1 - range.0 >= 127`, an inclusive range of this many identities.
const MINIMUM_IDS: u32 = 128;
/// The mount options that say a filesystem accounts project identities. ext4 spells it
/// `prjquota`; XFS accepts `pquota` and reports `prjquota`.
const QUOTA_OPTIONS: [&str; 2] = ["prjquota", "pquota"];

const ROOT_VAR: &str = "SUBSTRATE_TEST_QUOTA_ROOT";
const IDS_VAR: &str = "SUBSTRATE_TEST_PROJECT_QUOTA_IDS";

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// State which cases exist and whether the fixture would run them; run nothing, exit 0.
    #[arg(long)]
    pub inventory: bool,
    /// Build and run in the release profile, as the recorded 2026-09-05 execution did.
    #[arg(long)]
    pub release: bool,
}

/// One resolved fixture: a directory on a project-quota filesystem and an exclusive ID range.
#[derive(Debug)]
struct Fixture {
    root: PathBuf,
    ids: (u32, u32),
    mount: String,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let source = root.join(SOURCE);
    let text = std::fs::read_to_string(&source)
        .with_context(|| format!("reading {}", source.display()))?;
    let cases = declared_cases(&text);
    if cases.is_empty() {
        // A filter that matches nothing still exits 0, so an empty inventory must not be a run.
        eprintln!(
            "quota lane: refusing — {SOURCE} declares no ignored `real_quota_*` case. Either the \
             cases were removed or this verb's parser no longer recognises them; it must not \
             report a green lane having selected nothing."
        );
        return Ok(ExitCode::FAILURE);
    }

    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let fixture = prerequisites(
        std::env::var_os(ROOT_VAR).map(PathBuf::from),
        std::env::var(IDS_VAR).ok(),
        &mountinfo,
    );

    if args.inventory {
        report_inventory(&cases, fixture.as_ref());
        return Ok(ExitCode::SUCCESS);
    }

    let fixture = match fixture {
        Ok(fixture) => fixture,
        Err(missing) => {
            report_refusal(&cases, &missing);
            return Ok(ExitCode::FAILURE);
        }
    };

    println!(
        "quota lane: root {} ids {}-{} on {}",
        fixture.root.display(),
        fixture.ids.0,
        fixture.ids.1,
        fixture.mount
    );
    let mut ran = Vec::new();
    let mut red = Vec::new();
    for case in &cases {
        if execute(&root, &fixture, args.release, case)? {
            ran.push(case.clone());
        } else {
            red.push(case.clone());
        }
    }
    println!(
        "quota lane: {} declared, {} ran, {} failed",
        cases.len(),
        ran.len(),
        red.len()
    );
    for case in &ran {
        println!("quota lane:   ran  {MODULE}::{case}");
    }
    for case in &red {
        println!("quota lane:   FAIL {MODULE}::{case}");
    }
    Ok(if red.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The statement `scripts/delegated-lane.sh` prints: these cases exist and this lane did not run
/// them.
fn report_inventory(cases: &[String], fixture: Result<&Fixture, &Vec<String>>) {
    println!(
        "quota lane: {} real-quota cases are declared in {SOURCE} and are NOT part of this lane",
        cases.len()
    );
    for case in cases {
        println!("quota lane:   not run  {MODULE}::{case}");
    }
    match fixture {
        Ok(fixture) => println!(
            "quota lane: fixture present ({} on {}); run them with `cargo xtask quota-lane`",
            fixture.root.display(),
            fixture.mount
        ),
        Err(missing) => {
            for reason in missing {
                println!("quota lane: fixture absent — {reason}");
            }
        }
    }
}

/// The refusal: what is missing, by name, and which cases therefore did not run.
fn report_refusal(cases: &[String], missing: &[String]) {
    eprintln!("quota lane: refusing — the project-quota fixture is absent:");
    for reason in missing {
        eprintln!("quota lane:   {reason}");
    }
    eprintln!("quota lane: these {} cases did not run:", cases.len());
    for case in cases {
        eprintln!("quota lane:   not run  {MODULE}::{case}");
    }
    eprintln!(
        "quota lane: provision the fixture, then \
         `{ROOT_VAR}=<dir> {IDS_VAR}=<start>-<end> cargo xtask quota-lane` (AGENTS.md, § *The real \
         project-quota lane*)."
    );
}

/// Every `#[ignore]`d test function the source declares, in declaration order.
///
/// Read from the source rather than from `--ignored --list`, so the refusal and the inventory cost
/// no build: `scripts/delegated-lane.sh` prints the inventory on a host that has no fixture, and a
/// refusal that had to compile the host crate first would not be a refusal.
fn declared_cases(source: &str) -> Vec<String> {
    let mut cases = Vec::new();
    let mut ignored = false;
    for line in source.lines() {
        let line = line.trim_start();
        if line.starts_with("#[ignore") {
            ignored = true;
        } else if let Some(name) = function_name(line) {
            if ignored {
                cases.push(name.to_owned());
            }
            ignored = false;
        } else if !line.starts_with("#[") && !line.is_empty() {
            ignored = false;
        }
    }
    cases
}

fn function_name(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("async fn ")
        .or_else(|| line.strip_prefix("fn "))?;
    let end = rest.find(|character: char| !character.is_alphanumeric() && character != '_')?;
    rest.get(..end).filter(|name| !name.is_empty())
}

/// Resolve the fixture, or name every prerequisite that is missing.
fn prerequisites(
    root: Option<PathBuf>,
    ids: Option<String>,
    mountinfo: &str,
) -> Result<Fixture, Vec<String>> {
    let mut missing = Vec::new();
    let mut resolved = None;
    match root {
        None => missing.push(format!(
            "{ROOT_VAR} is unset; it must name a directory on a filesystem mounted with project \
             quotas"
        )),
        Some(root) => match root.canonicalize() {
            Err(error) => missing.push(format!(
                "{ROOT_VAR}={} cannot be resolved: {error}",
                root.display()
            )),
            Ok(root) if !root.is_dir() => {
                missing.push(format!("{ROOT_VAR}={} is not a directory", root.display()));
            }
            Ok(root) => match quota_mount(mountinfo, &root) {
                Some(mount) => resolved = Some((root, mount)),
                None => missing.push(mount_refusal(mountinfo, &root)),
            },
        },
    }

    let range = match ids {
        None => {
            missing.push(format!(
                "{IDS_VAR} is unset; it must be START-END with START > 0 and at least \
                 {MINIMUM_IDS} identities"
            ));
            None
        }
        Some(ids) => match parse_ids(&ids) {
            Ok(range) => Some(range),
            Err(reason) => {
                missing.push(reason);
                None
            }
        },
    };

    match (resolved, range) {
        (Some((root, mount)), Some(ids)) if missing.is_empty() => Ok(Fixture { root, ids, mount }),
        _ => Err(missing),
    }
}

fn parse_ids(ids: &str) -> Result<(u32, u32), String> {
    let malformed = || format!("{IDS_VAR}={ids} is not START-END with two decimal identities");
    let (start, end) = ids.trim().split_once('-').ok_or_else(malformed)?;
    let start: u32 = start.parse().map_err(|_| malformed())?;
    let end: u32 = end.parse().map_err(|_| malformed())?;
    if start == 0 {
        return Err(format!(
            "{IDS_VAR}={ids} starts at 0, which is not a project identity"
        ));
    }
    if end < start {
        return Err(format!("{IDS_VAR}={ids} ends before it starts"));
    }
    let count = u64::from(end - start) + 1;
    if count < u64::from(MINIMUM_IDS) {
        return Err(format!(
            "{IDS_VAR}={ids} offers {count} identities; the fixture needs at least {MINIMUM_IDS}"
        ));
    }
    Ok((start, end))
}

/// The mount point backing `path`, if the filesystem there accounts project identities.
///
/// The longest mount point that is an ancestor of `path` wins, which is what the kernel resolves
/// to. Mount points carrying an octal escape (`\040` for a space) are compared as written; no
/// fixture root in this repository has one.
fn quota_mount(mountinfo: &str, path: &Path) -> Option<String> {
    let (point, options) = backing_mount(mountinfo, path)?;
    QUOTA_OPTIONS
        .iter()
        .any(|option| options.split(',').any(|present| present == *option))
        .then(|| format!("{point} ({options})"))
}

fn mount_refusal(mountinfo: &str, root: &Path) -> String {
    let wanted = QUOTA_OPTIONS.join(" nor ");
    match backing_mount(mountinfo, root) {
        Some((point, options)) => format!(
            "the filesystem mounted at {point} backing {ROOT_VAR}={} carries neither {wanted} \
             (options: {options})",
            root.display()
        ),
        None => format!(
            "no mount in /proc/self/mountinfo backs {ROOT_VAR}={}, so project-quota support cannot \
             be established",
            root.display()
        ),
    }
}

/// The mount point backing `path` and the union of its per-mount and super-block options.
fn backing_mount(mountinfo: &str, path: &Path) -> Option<(String, String)> {
    let mut best: Option<(PathBuf, String)> = None;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let (Some(point), Some(per_mount)) = (fields.get(4), fields.get(5)) else {
            continue;
        };
        let Some(separator) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        let super_options = fields.get(separator + 3).copied().unwrap_or_default();
        let point = Path::new(point);
        if !path.starts_with(point) {
            continue;
        }
        // Deeper wins, and at equal depth the later line wins: mountinfo lists an overmount after
        // what it covers, and the last one at a point is the one a path actually resolves through.
        let deeper = best
            .as_ref()
            .is_none_or(|(chosen, _)| point.components().count() >= chosen.components().count());
        if deeper {
            best = Some((
                point.to_owned(),
                format!("{per_mount},{super_options}")
                    .trim_end_matches(',')
                    .to_owned(),
            ));
        }
    }
    best.map(|(point, options)| (point.display().to_string(), options))
}

/// Run one case on its own, exactly as the recorded execution selected them: `--ignored --exact`,
/// one thread, output uncaptured. Returns whether the harness reported that one case passed.
fn execute(root: &Path, fixture: &Fixture, release: bool, case: &str) -> Result<bool> {
    let exact = format!("{MODULE}::{case}");
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command.current_dir(root);
    command.args(["test", "-p", PACKAGE, "--lib", "--locked"]);
    if release {
        command.arg("--release");
    }
    command.args([
        "--",
        "--ignored",
        "--exact",
        "--test-threads=1",
        "--nocapture",
        &exact,
    ]);
    command.env(ROOT_VAR, &fixture.root);
    command.env(IDS_VAR, format!("{}-{}", fixture.ids.0, fixture.ids.1));
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {exact}"))?;
    let stdout = child
        .stdout
        .take()
        .context("the child was spawned with a piped stdout")?;
    let mut selected = false;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        // A filter that matches nothing exits 0 with `0 passed`, so the summary — not the exit
        // status — is what says the case ran.
        if line.starts_with("test result: ok. 1 passed;") {
            selected = true;
        }
        println!("{line}");
    }
    let status = child.wait().with_context(|| format!("awaiting {exact}"))?;
    if !status.success() {
        eprintln!("quota lane: {exact} exited {status}");
        return Ok(false);
    }
    if !selected {
        eprintln!("quota lane: {exact} selected no case; the harness reported no `1 passed`");
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::{
        MINIMUM_IDS, backing_mount, declared_cases, parse_ids, prerequisites, quota_mount,
    };
    use std::path::{Path, PathBuf};

    const EXT4: &str =
        "25 1 259:2 / / rw,noatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro\n";
    fn quota_mountinfo(point: &Path) -> String {
        format!(
            "{EXT4}42 25 7:0 / {} rw,noatime shared:2 - ext4 /dev/loop0 rw,prjquota\n",
            point.display()
        )
    }

    #[test]
    fn only_ignored_test_functions_are_declared_cases() {
        let source = "#[tokio::test]\n\
                      #[ignore = \"fixture\"]\n\
                      async fn real_quota_one() {}\n\
                      #[tokio::test]\n\
                      async fn portable_two() {}\n\
                      #[test]\n\
                      #[ignore]\n\
                      fn real_quota_three() {}\n\
                      // #[ignore] in prose\n\
                      async fn not_a_case() {}\n";
        assert_eq!(
            declared_cases(source),
            vec!["real_quota_one".to_owned(), "real_quota_three".to_owned()]
        );
    }

    #[test]
    fn the_repositorys_own_quota_cases_are_all_found_and_all_real_quota() {
        let root = crate::repo::root().expect("workspace root");
        let source = std::fs::read_to_string(root.join(super::SOURCE)).expect("quota test source");
        let cases = declared_cases(&source);
        assert!(
            cases.len() >= 7,
            "the lane must select every ignored case; found {cases:?}"
        );
        for case in &cases {
            assert!(
                case.starts_with("real_quota_"),
                "{case} is ignored but is not a real-quota case; the lane's `--exact` selection \
                 would run it under a fixture it does not need"
            );
        }
    }

    #[test]
    fn an_identity_range_must_be_nonzero_ordered_and_large_enough() {
        assert_eq!(parse_ids("200000-200511"), Ok((200_000, 200_511)));
        assert_eq!(
            parse_ids(&format!("1-{MINIMUM_IDS}")),
            Ok((1, MINIMUM_IDS)),
            "an inclusive range of exactly {MINIMUM_IDS} identities is enough"
        );
        for invalid in [
            "",
            "200000",
            "0-1000",
            "200511-200000",
            "200000-200126",
            "a-b",
        ] {
            assert!(
                parse_ids(invalid).is_err(),
                "{invalid} must be refused by name"
            );
        }
    }

    #[test]
    fn the_longest_backing_mount_wins_and_its_options_are_both_fields() {
        let (point, options) = backing_mount(
            &quota_mountinfo(Path::new("/quota")),
            Path::new("/quota/git-quota-a"),
        )
        .expect("a backing mount");
        assert_eq!(point, "/quota");
        assert!(options.contains("prjquota"), "{options}");
        let (point, _) = backing_mount(EXT4, Path::new("/home/one")).expect("a backing mount");
        assert_eq!(point, "/");
        assert!(backing_mount("", Path::new("/quota")).is_none());
    }

    #[test]
    fn a_mount_without_project_quotas_is_not_a_fixture() {
        assert!(quota_mount(EXT4, Path::new("/srv")).is_none());
        assert!(quota_mount(&quota_mountinfo(Path::new("/quota")), Path::new("/quota")).is_some());
    }

    #[test]
    fn every_missing_prerequisite_is_named_rather_than_skipped() {
        let missing = prerequisites(None, None, EXT4).expect_err("no fixture");
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(
            missing[0].contains("SUBSTRATE_TEST_QUOTA_ROOT is unset"),
            "{missing:?}"
        );
        assert!(
            missing[1].contains("SUBSTRATE_TEST_PROJECT_QUOTA_IDS is unset"),
            "{missing:?}"
        );

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().canonicalize().expect("canonical root");
        let plain = format!(
            "25 1 259:2 / / rw,noatime shared:1 - ext4 /dev/nvme0n1p2 rw\n\
             42 25 7:0 / {} rw shared:2 - ext4 /dev/loop0 rw\n",
            root.display()
        );
        let missing = prerequisites(Some(root.clone()), Some("200000-200511".to_owned()), &plain)
            .expect_err("no project quotas");
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("prjquota"), "{missing:?}");

        let fixture = prerequisites(
            Some(root.clone()),
            Some("200000-200511".to_owned()),
            &quota_mountinfo(&root),
        )
        .expect("a provisioned fixture resolves");
        assert_eq!(fixture.root, root);
        assert_eq!(fixture.ids, (200_000, 200_511));

        let absent = PathBuf::from("/nonexistent-quota-root-for-this-test");
        let missing = prerequisites(Some(absent), Some("200000-200511".to_owned()), EXT4)
            .expect_err("an unresolvable root");
        assert!(missing[0].contains("cannot be resolved"), "{missing:?}");
    }
}
