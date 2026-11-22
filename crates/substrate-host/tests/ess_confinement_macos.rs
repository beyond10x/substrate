//! The confinement contract's macos lane (`story:macos-adapter-on-seatbelt`, S1, and
//! `story:macos-seatbelt-policy-runs-cargo-test`, S2), and the `macos-io-*` stage of design 32
//! (`story:adapter-stdin-pipe` and `story:adapter-live-output`).
//!
//! Executes every authored `-on-macos` scenario of `spec/confinement-suite.json` against the real
//! `macos` adapter, opened through `confinement::open` on a macOS host, and `absent-linux-*`
//! against `linux`, which is absent here. The count is pinned, the suite must hold exactly it, and
//! it must equal what ran.
//!
//! Every macos open step states the final declared set, `sealed_paths`, `exec_subpaths` and
//! `private_tmpdir` included, `stdin_pipe` and `output_live` since design 32 and
//! `exec_denied_paths` since design 35, and the adapter must have proved exactly that set; each
//! `macos-s2-*`, `macos-io-*` and `macos-lists-*` admission runs through the runner like every
//! other scenario.
//!
//! Beside the suite: a run is confined to its workspace and its declared read-only roots, a root
//! that would appear anywhere but at its host path is refused by name, and `limits`,
//! `network_egress` and `whole_tree_kill` are refused with nothing run.

#![cfg(target_os = "macos")]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, Network, ProcessBounds, RunState,
    linux::LinuxRequest,
    macos::{MacosRequest, S2_FACTS, S2_STORY},
};
use substrate_host::seatbelt::SANDBOX_EXEC;
use substrate_wire::{READ_ONLY_ROOT_INVALID, ReadOnlyRoot};

const BASH: &str = "/bin/bash";

fn macos() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Macos(MacosRequest::default()))
        .expect("macos opens on a macOS host whose Seatbelt sandbox enforces")
}

/// The real adapter each macos-lane open step names.
fn open_macos_lane(
    step: &support::OpenStep,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    match step.adapter.as_str() {
        "macos" => confinement::open(AdapterRequest::Macos(MacosRequest::default())),
        // The setup the scenario names: the runner is on macOS, where `linux` cannot open.
        "linux" => confinement::open(AdapterRequest::Linux(Box::new(LinuxRequest::new(
            substrate_host::HostConfig::minimum("/nonexistent/substrate-workspaces"),
        )))),
        other => panic!("no macos-lane scenario opens {other}"),
    }
}

fn fact_name(fact: &Fact) -> String {
    serde_json::to_value(fact)
        .expect("fact")
        .as_str()
        .expect("a fact is named")
        .to_owned()
}

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

fn events(records: &Mutex<Vec<AuditRecord>>) -> Vec<String> {
    records
        .lock()
        .iter()
        .map(|record| {
            serde_json::to_value(record).expect("record")["event"]
                .as_str()
                .expect("event")
                .to_owned()
        })
        .collect()
}

/// The authored `-on-macos` scenarios `spec/confinement-suite.json` holds. Pinned, so a scenario
/// lost or renamed out of the lane fails here instead of shrinking what runs.
const MACOS_DECLARED: usize = 11;

/// Of those, the `macos-s1-*`, the `macos-s2-*`, the `macos-io-*` (design 32) and the
/// `macos-lists-*` (design 35) scenarios; the rest are `absent-*`.
const MACOS_S1: usize = 4;
const MACOS_S2: usize = 3;
const MACOS_IO: usize = 2;
const MACOS_LISTS: usize = 1;

#[test]
fn ess_confinement_macos_lane_executes_every_authored_macos_scenario() {
    let scenarios = support::scenarios("macos", |_| true);
    let declared = MACOS_DECLARED;
    assert_eq!(
        scenarios.len(),
        declared,
        "spec/confinement-suite.json holds another number of authored -on-macos scenarios than \
         this lane pins"
    );
    let count = |stage: &str| {
        scenarios
            .iter()
            .filter(|(id, _)| id.contains(&format!("/macos-{stage}-")))
            .count()
    };
    let (s1, s2, io, lists) = (count("s1"), count("s2"), count("io"), count("lists"));
    assert_eq!(s1, MACOS_S1, "the macos-s1 scenarios this lane pins");
    assert_eq!(s2, MACOS_S2, "the macos-s2 scenarios for {S2_STORY}");
    assert_eq!(io, MACOS_IO, "the macos-io scenarios of design 32");
    assert_eq!(lists, MACOS_LISTS, "the macos-lists scenarios of design 35");
    assert!(
        scenarios.iter().all(|(id, _)| id.contains("/macos-s1-")
            || id.contains("/macos-s2-")
            || id.contains("/macos-io-")
            || id.contains("/macos-lists-")
            || id.contains("/absent-")),
        "a macos-lane scenario is neither S1's, S2's, the I/O stage's, the lists' nor an absent \
         adapter's: {:?}",
        scenarios.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
    let (executed, audited, _) = support::run(&scenarios, open_macos_lane);
    assert_eq!(executed, declared, "a macos scenario did not run");
    assert!(audited > 0, "no refusal audit record was asserted");
    println!(
        "ESS confinement macos lane: {executed} scenarios executed ({s1} macos-s1, {s2} macos-s2, {io} macos-io, \
         {lists} macos-lists) of {declared} declared; {audited} refusal audit records asserted, 0 failed"
    );
}

#[test]
fn s2_facts_are_proved_and_a_profile_requiring_each_is_admitted() {
    let adapter = macos();
    let facts = adapter.facts();
    assert!(!facts.declared_insecure());
    assert!(facts.proves(&Fact::PathsAreHostPaths));
    assert!(facts.proves(&Fact::IpcSignalsContained));
    for fact in &S2_FACTS {
        assert!(
            facts.proves(fact),
            "{fact:?} is not proved: {S2_STORY} makes the Seatbelt build probe prove it at open"
        );
    }
    let by_field = [
        (
            ConfinementProfile {
                sealed_paths: vec![".git".into()],
                ..ConfinementProfile::default()
            },
            Fact::SealedPaths,
        ),
        (
            ConfinementProfile {
                exec_subpaths: vec!["target/debug".into()],
                ..ConfinementProfile::default()
            },
            Fact::ExecSubpaths,
        ),
        (
            ConfinementProfile {
                private_tmpdir: true,
                ..ConfinementProfile::default()
            },
            Fact::PrivateTmpdir,
        ),
    ];
    for (profile, expected) in by_field {
        let by_name = ConfinementProfile {
            facts: vec![expected.clone()],
            ..ConfinementProfile::default()
        };
        for profile in [profile, by_name] {
            let (sink, records) = recording();
            let admitted = adapter
                .admit(&ConfinementProfile {
                    audit: Some(sink),
                    ..profile
                })
                .unwrap_or_else(|error| {
                    panic!("{expected:?} is refused after {S2_STORY}: {error}")
                });
            assert!(!admitted.declared_insecure());
            let record = serde_json::to_value(&records.lock()[0]).expect("record");
            assert_eq!(record["event"], "profile_admitted", "{record}");
            assert_eq!(record["facts"], json!([fact_name(&expected)]), "{record}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn limits_egress_and_whole_tree_kill_are_refused_by_name_and_nothing_runs() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let cases = [
        (
            ConfinementProfile {
                process_bounds: Some(ProcessBounds {
                    timeout_ms: 1_000,
                    processes: 8,
                    memory_bytes: 1 << 26,
                    cpu_millis: 1_000,
                }),
                ..ConfinementProfile::default()
            },
            "confinement.family-unserved",
            ("family", "limits"),
        ),
        (
            ConfinementProfile {
                network: Some(Network::Egress {
                    apertures: vec!["registry".into()],
                }),
                ..ConfinementProfile::default()
            },
            "confinement.family-unserved",
            ("family", "network_egress"),
        ),
        (
            ConfinementProfile {
                facts: vec![Fact::WholeTreeKill],
                ..ConfinementProfile::default()
            },
            "confinement.fact-unserved",
            ("fact", "whole_tree_kill"),
        ),
    ];
    for (profile, code, (field, named)) in cases {
        let (sink, records) = recording();
        let error = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                audit: Some(sink),
                ..profile
            })
            .expect_err(named);
        assert_eq!(error.code(), code, "{named}: {error}");
        assert_eq!(error.status(), 501);
        let record = serde_json::to_value(&records.lock()[0]).expect("record");
        assert_eq!(record[field], named, "{record}");
        // Admission returned no profile, so nothing can start: one refusal record and no start.
        assert_eq!(events(&records), ["profile_refused"], "{named}");
    }
    assert_eq!(
        std::fs::read_dir(workspace.path())
            .expect("workspace")
            .count(),
        0,
        "something ran in the workspace"
    );
}

fn bash(script: &str, extra: &[&str]) -> Command {
    let mut argv = vec![BASH.to_owned(), "-c".into(), script.into(), "bash".into()];
    argv.extend(extra.iter().map(|arg| (*arg).to_owned()));
    Command {
        argv,
        env: BTreeMap::new(),
    }
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

async fn run_to_end(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: ConfinementProfile,
    command: &Command,
) -> (Option<i32>, String, String) {
    let admitted = adapter.admit(&profile).expect("admitted");
    let run = adapter.start(&admitted, command).await.expect("started");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let output = adapter.output(&run).await.expect("output");
    (
        ended.exit.and_then(|exit| exit.code),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_writes_its_workspace_and_reports_how_it_ended() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(workspace.path().join("sub")).expect("workdir");
    let (sink, records) = recording();
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            workdir: Some("sub".into()),
            families: vec![ContractFamily::ProcessRun, ContractFamily::Wait],
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(!admitted.declared_insecure());
    let mut command = bash(
        "pwd; printf done > ../marker; printf '%s' \"$GREETING\"; echo oops >&2; exit 3",
        &[],
    );
    command.env.insert("GREETING".into(), "hello".into());
    let run = adapter.start(&admitted, &command).await.expect("started");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    assert_eq!(ended.adapter, AdapterName::Macos);
    assert_eq!(ended.declared_insecure, None);
    assert_eq!(ended.exit.and_then(|exit| exit.code), Some(3));
    let output = adapter.output(&run).await.expect("output");
    let sub = std::fs::canonicalize(workspace.path().join("sub")).expect("canonical");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\nhello", sub.display())
    );
    assert_eq!(output.stderr, b"oops\n");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("marker")).expect("marker"),
        "done"
    );
    assert_eq!(events(&records), ["profile_admitted", "run_started"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environment_is_only_the_runs_own() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    assert!(
        std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        "cargo sets CARGO_MANIFEST_DIR for the test process"
    );
    let (code, stdout, stderr) = run_to_end(
        &adapter,
        ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        },
        &bash("printf '%s' \"${CARGO_MANIFEST_DIR-unset}\"", &[]),
    )
    .await;
    assert_eq!((code, stdout.as_str()), (Some(0), "unset"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_outside_the_workspace_fails_inside_the_run() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let escaped = outside.path().join("escaped");
    let profile = || ConfinementProfile {
        workspace: Some(workspace.path().to_path_buf()),
        ..ConfinementProfile::default()
    };
    let inside = workspace.path().join("inside");
    let (code, _, stderr) = run_to_end(
        &adapter,
        profile(),
        &bash("printf x > \"$1\"", &[text(&inside)]),
    )
    .await;
    assert_eq!(code, Some(0), "control: {stderr}");
    assert_eq!(std::fs::read(&inside).expect("inside"), b"x");
    let (code, _, _) = run_to_end(
        &adapter,
        profile(),
        &bash("printf x > \"$1\"", &[text(&escaped)]),
    )
    .await;
    assert_ne!(code, Some(0));
    assert!(!escaped.exists(), "the run wrote outside its workspace");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_root_is_readable_at_its_host_path_and_not_writable() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("file"), "inside the root\n").expect("file");
    let host_path = text(root.path()).to_owned();
    let (code, stdout, stderr) = run_to_end(
        &adapter,
        ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: host_path.clone(),
                mount: host_path.clone(),
            }],
            ..ConfinementProfile::default()
        },
        &bash(
            "IFS= read -r line < \"$1/file\"; printf '%s' \"$line\"; printf x > \"$1/new\"",
            &[&host_path],
        ),
    )
    .await;
    assert_eq!(stdout, "inside the root", "{stderr}");
    assert_ne!(code, Some(0), "the write into the root succeeded");
    assert!(!root.path().join("new").exists(), "the root was written");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_anywhere_but_its_host_path_is_refused_by_name() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let root = tempfile::tempdir().expect("root");
    let (sink, records) = recording();
    let error = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: text(root.path()).to_owned(),
                mount: "/opt/elsewhere".into(),
            }],
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect_err("a remapped root");
    assert_eq!(error.code(), READ_ONLY_ROOT_INVALID, "{error}");
    assert!(
        error.to_string().contains("paths_are_host_paths"),
        "{error}"
    );
    assert_eq!(events(&records), ["profile_refused"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_root_overlapping_the_workspace_is_refused_before_anything_runs() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(workspace.path().join("sub")).expect("sub");
    let parent = workspace.path().parent().expect("parent").to_path_buf();
    for root in [workspace.path().join("sub"), parent] {
        let host_path = text(&root).to_owned();
        let (sink, records) = recording();
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                read_only_roots: vec![ReadOnlyRoot {
                    host_path: host_path.clone(),
                    mount: host_path.clone(),
                }],
                audit: Some(sink),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = adapter
            .start(&admitted, &bash("printf ran > marker", &[]))
            .await
            .expect_err("an overlapping root");
        assert_eq!(error.code(), READ_ONLY_ROOT_INVALID, "{host_path}: {error}");
        assert!(!workspace.path().join("marker").exists());
        // The refused start writes its own record (ADR 0109 decision 2).
        assert_eq!(
            events(&records),
            ["profile_admitted", "profile_refused"],
            "{host_path}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workdir_through_a_symlink_out_of_the_workspace_is_refused() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("link")).expect("symlink");
    for workdir in ["link", "absent"] {
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.path().to_path_buf()),
                workdir: Some(workdir.into()),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = adapter
            .start(&admitted, &bash("printf ran > marker", &[]))
            .await
            .expect_err(workdir);
        assert_eq!(
            error.code(),
            substrate_wire::EXEC_WORKDIR_MISSING,
            "{error}"
        );
    }
    assert!(!outside.path().join("marker").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_program_itself_is_executable() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let profile = || ConfinementProfile {
        workspace: Some(workspace.path().to_path_buf()),
        ..ConfinementProfile::default()
    };
    let (code, stdout, stderr) = run_to_end(&adapter, profile(), &bash("echo builtin", &[])).await;
    assert_eq!((code, stdout.as_str()), (Some(0), "builtin\n"), "{stderr}");
    let (code, stdout, _) = run_to_end(&adapter, profile(), &bash("/bin/echo exec", &[])).await;
    assert_ne!(code, Some(0));
    assert_eq!(stdout, "", "an unlisted executable ran");
}

/// Design 35 § 8.3 and the check of `story:executable-deny-list`: `/usr/bin` allowed and one
/// program beneath it denied. Another program of `/usr/bin` runs and the denied one does not.
/// A symbolic link to the denied program is denied, because Seatbelt matches the resolved
/// path; a deny entry that does not exist at the start denies the file once the run creates
/// it; and a file entry in `exec_subpaths` admits that file and not its neighbour (D2).
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_program_beneath_an_allowed_directory_does_not_run() {
    let adapter = macos();
    assert!(
        adapter.facts().proves(&Fact::ExecDeniedPaths),
        "the Seatbelt build probe did not prove exec_denied_paths: {:?}",
        adapter.facts()
    );
    let root = tempfile::tempdir().expect("workspace");
    let workspace = root.path().canonicalize().expect("canonical workspace");
    std::fs::create_dir(workspace.join("bin")).expect("bin");
    std::os::unix::fs::symlink("/usr/bin/true", workspace.join("link")).expect("a link");
    let profile = |allowed: &[&str], denied: &[&str]| ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: allowed.iter().map(|entry| (*entry).to_owned()).collect(),
        exec_denied_paths: denied.iter().map(|entry| (*entry).to_owned()).collect(),
        ..ConfinementProfile::default()
    };

    // The story's check, and a link to the denied program.
    let (code, stdout, stderr) = run_to_end(
        &adapter,
        profile(&["/usr/bin"], &["/usr/bin/true"]),
        &bash(
            "/usr/bin/false; echo $?; /usr/bin/true 2>/dev/null; echo $?; \
             ./link 2>/dev/null; echo $?",
            &[],
        ),
    )
    .await;
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}{stderr}");
    assert_eq!(
        lines[0], "1",
        "the neighbour of the denied program ran: {stderr}"
    );
    assert_ne!(lines[1], "0", "the denied program ran");
    assert_ne!(lines[2], "0", "a link to the denied program ran it");
    assert_eq!(code, Some(0), "{stderr}");

    // A deny entry that does not exist yet denies the file once it is created. The control is
    // its neighbour, created the same way in the same listed directory.
    let (_, stdout, stderr) = run_to_end(
        &adapter,
        profile(&["/usr/bin", "/bin", "bin"], &["bin/later"]),
        &bash(
            "/bin/cp /usr/bin/true bin/later && /bin/cp /usr/bin/true bin/other || exit 9; \
             bin/other; echo $?; bin/later 2>/dev/null; echo $?",
            &[],
        ),
    )
    .await;
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}{stderr}");
    assert_eq!(
        lines[0], "0",
        "a program created in the listed directory ran: {stderr}"
    );
    assert_ne!(lines[1], "0", "the deny entry created during the run ran");

    // D2: a file entry admits that file only.
    let (_, stdout, stderr) = run_to_end(
        &adapter,
        profile(&["/usr/bin/true"], &[]),
        &bash(
            "/usr/bin/true; echo $?; /usr/bin/false 2>/dev/null; echo $?",
            &[],
        ),
    )
    .await;
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}{stderr}");
    assert_eq!(lines[0], "0", "the file entry's program ran: {stderr}");
    assert!(
        lines[1] != "0" && lines[1] != "1",
        "the neighbour of a file entry ran: {stdout}"
    );

    // D13 on this adapter: a deny-list without an allow-list is refused by name.
    let error = adapter
        .admit(&profile(&[], &["/usr/bin/true"]))
        .expect_err("a deny-list without an allow-list");
    assert_eq!(
        (error.code(), error.status()),
        ("confinement.exec-denied-paths-without-subpaths", 422),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signal_ends_the_run() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sleep".into(), "30".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    assert_eq!(
        adapter.observe(&run).await.expect("observed").state,
        RunState::Running
    );
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("signalled");
    let ended = tokio::time::timeout(Duration::from_secs(10), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.exit.and_then(|exit| exit.signal), Some(9));
}

#[test]
fn render_is_the_sandbox_exec_argv_and_runs_nothing() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let rendered = adapter
        .render(&admitted, &bash("printf ran > marker", &[]))
        .expect("rendered");
    assert_eq!(rendered.program, PathBuf::from(SANDBOX_EXEC));
    assert_eq!(rendered.args[0], "-p");
    assert!(
        rendered.args[1].starts_with("(version 1)\n(deny default)\n"),
        "{}",
        rendered.args[1]
    );
    assert_eq!(
        rendered.args[2..],
        [BASH, "-c", "printf ran > marker", "bash"]
    );
    assert!(!workspace.path().join("marker").exists());
}

// A9: no Seatbelt probe proves a narrower workspace yet, so `macos` refuses it by name.
#[test]
fn a_narrower_workspace_is_refused_scoped_write_unserved() {
    let adapter = macos();
    for access in [
        confinement::WorkspaceAccess::ReadOnly,
        confinement::WorkspaceAccess::Scoped {
            writable_subtrees: vec!["out".to_owned()],
        },
    ] {
        let (sink, records) = recording();
        let error = adapter
            .admit(&ConfinementProfile {
                workspace_access: access.clone(),
                audit: Some(sink),
                ..ConfinementProfile::default()
            })
            .expect_err("refused");
        assert_eq!(
            error.code(),
            substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED,
            "{access:?}: {error}"
        );
        assert_eq!(error.status(), 501);
        assert_eq!(events(&records), ["profile_refused"]);
    }
}

// T14 of design 32: `/bin/cat` under Seatbelt reads three writes through the adapter's writer,
// and `close_stdin` ends it.
#[tokio::test(flavor = "multi_thread")]
async fn a_macos_run_reads_its_piped_stdin() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    support::stdin_echoes_through(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            stdin: confinement::StdinMode::Piped { limit_bytes: 1_024 },
            ..ConfinementProfile::default()
        },
        &Command {
            argv: vec!["/bin/cat".into()],
            env: BTreeMap::new(),
        },
        &[b"one\n", b"two\n", b"three\n"],
    )
    .await;
}

// B9: a `macos` run keeps the last bytes under `Tail`. The numbers come from bash's own `printf`:
// only the program itself is executable, so `/usr/bin/seq` ends the run with 126.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_under_tail_keeps_the_last_bytes() {
    const LAST: u32 = 300_000;
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            output_retention: confinement::OutputRetention {
                stdout: confinement::Retention::Tail,
                stderr: confinement::Retention::Head,
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &bash(&format!("printf '%s\\n' {{1..{LAST}}}"), &[]),
        )
        .await
        .expect("started");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.exit.and_then(|exit| exit.code), Some(0));
    let output = adapter.output(&run).await.expect("output");
    support::assert_retained(
        "macos tail",
        &support::seq_bytes(LAST),
        &output.stdout,
        &output.stdout_kept,
        confinement::Retention::Tail,
        confinement::macos::MACOS_OUTPUT_LIMIT_BYTES,
        None,
    );
    assert!(output.stdout_truncated);
}

// T14 of design 32: under Seatbelt a live run's chunks on both streams are contiguous and are
// exactly the record at its natural end. Bash's own `printf`: only the program is executable.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_macos_run_delivers_its_record_on_a_natural_end() {
    let adapter = macos();
    let workspace = tempfile::tempdir().expect("workspace");
    support::live_natural_end(
        "macos live natural end",
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        },
        &bash(
            "printf 'out-one\\n'; printf 'err-one\\n' >&2; printf 'out-two\\n'; \
             printf 'err-two\\n' >&2",
            &[],
        ),
        b"out-one\nout-two\n",
        b"err-one\nerr-two\n",
    )
    .await;
}
