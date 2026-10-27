//! The confinement contract's macos lane (`story:macos-adapter-on-seatbelt`, S1).
//!
//! Executes every authored `-on-macos` scenario of `spec/confinement-suite.json` against the real
//! `macos` adapter, opened through `confinement::open` on a macOS host, and `absent-linux-*`
//! against `linux`, which is absent here. The count is read from the suite and must equal what
//! ran.
//!
//! Every macos open step states the final declared set, after
//! `story:macos-seatbelt-policy-runs-cargo-test` (S2). S1's adapter proves that set less
//! `sealed_paths`, `exec_subpaths` and `private_tmpdir`, so the runner compares the adapter's facts
//! with the declared set less exactly those three, and each `macos-s2-*` admission is asserted
//! refused `confinement.fact-unserved` by the fact it requires. S2 flips both: it removes the
//! subtraction and runs `macos-s2-*` through the runner.
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
use serde_json::{Value, json};
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, Network, ProcessBounds, RunState,
    linux::LinuxRequest,
    macos::{MacosRequest, S2_FACTS, S2_STORY},
};
use substrate_host::seatbelt::SANDBOX_EXEC;
use substrate_wire::{READ_ONLY_ROOT_INVALID, ReadOnlyRoot};

const BASH: &str = "/bin/bash";
const OPEN: &str = "substrate.confinement.OpenAdapter";
const ADMIT: &str = "substrate.confinement.AdmitProfile";

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

fn is_s2(id: &str) -> bool {
    id.contains("/macos-s2-")
}

/// The scenario with every macos open step's declared set less [`S2_FACTS`], each of which it must
/// declare: S1's adapter proves exactly that.
fn s1_declared(id: &str, scenario: &Value) -> Value {
    let mut scenario = scenario.clone();
    for step in scenario["steps"].as_array_mut().expect("steps") {
        if step["command"] != OPEN || step["input"]["adapter"]["value"] != "macos" {
            continue;
        }
        let facts = step["input"]["facts"]["value"]
            .as_array_mut()
            .expect("the open step declares facts");
        for fact in &S2_FACTS {
            let name = json!(fact_name(fact));
            assert!(
                facts.contains(&name),
                "{id} no longer declares {name}: the subtraction for {S2_STORY} is stale"
            );
            facts.retain(|held| *held != name);
        }
    }
    scenario
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| item.as_str().expect("text").to_owned())
        .collect()
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

/// A `macos-s2-*` admission S1 refuses: the scenario expects S2's admission, and S1 answers
/// `confinement.fact-unserved` naming the S2 fact the profile requires.
fn refused_until_s2(id: &str, scenario: &Value) {
    let steps = scenario["steps"].as_array().expect("steps");
    let admit = steps
        .iter()
        .find(|step| step["command"] == ADMIT && step["step"] == "execute_command")
        .unwrap_or_else(|| panic!("{id} admits no profile"));
    let expected = steps
        .iter()
        .find(|step| step["step"] == "expect_outcome" && step["outcome"]["command"] == ADMIT)
        .unwrap_or_else(|| panic!("{id} expects no admission outcome"));
    assert_eq!(
        expected["outcome"]["outcome"], "admitted",
        "{id} is an S2 admission"
    );
    let input = &admit["input"];
    let families: Vec<ContractFamily> = strings(&input["required_families"]["value"])
        .iter()
        .map(|name| serde_json::from_value(json!(name)).expect("family"))
        .collect();
    let facts: Vec<Fact> = strings(&input["required_facts"]["value"])
        .iter()
        .map(|name| serde_json::from_value(json!(name)).expect("fact"))
        .collect();
    let pending = facts
        .iter()
        .find(|fact| S2_FACTS.contains(fact))
        .unwrap_or_else(|| panic!("{id} requires no S2 fact"))
        .clone();
    let (sink, records) = recording();
    let result = macos().admit(&ConfinementProfile {
        families,
        facts,
        audit: Some(sink),
        ..ConfinementProfile::default()
    });
    match result {
        Err(ConfinementError::FactUnserved {
            adapter: AdapterName::Macos,
            fact,
        }) => assert_eq!(
            fact, pending,
            "{id}: refused by another fact than the S2 one it requires; {S2_STORY} flips this"
        ),
        Ok(_) => panic!(
            "{id} is admitted: {S2_STORY} has landed, so run it through the suite runner and \
             drop this refusal"
        ),
        Err(other) => panic!("{id}: expected confinement.fact-unserved until {S2_STORY}: {other}"),
    }
    assert_eq!(events(&records), ["profile_refused"], "{id}");
}

#[test]
fn ess_confinement_macos_lane_executes_every_authored_macos_scenario() {
    let scenarios = support::scenarios("macos", |_| true);
    let declared = support::lane_size("macos");
    assert!(declared > 0, "the suite has no macos scenario");
    assert_eq!(scenarios.len(), declared);
    let (pending, runnable): (Vec<_>, Vec<_>) =
        scenarios.into_iter().partition(|(id, _)| is_s2(id));
    let s1 = runnable
        .iter()
        .filter(|(id, _)| id.contains("/macos-s1-"))
        .count();
    assert!(s1 > 0, "the suite has no macos-s1 scenario");
    assert!(
        runnable
            .iter()
            .all(|(id, _)| id.contains("/macos-s1-") || id.contains("/absent-")),
        "a macos-lane scenario is neither S1's, S2's nor an absent adapter's: {:?}",
        runnable.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
    let runnable: Vec<(String, Value)> = runnable
        .iter()
        .map(|(id, scenario)| (id.clone(), s1_declared(id, scenario)))
        .collect();
    let (executed, audited, _) = support::run(&runnable, open_macos_lane);
    assert_eq!(executed, runnable.len(), "a macos scenario did not run");
    assert!(audited > 0, "no refusal audit record was asserted");
    for (id, scenario) in &pending {
        println!("ESS scenario (refused until {S2_STORY}): {id}");
        refused_until_s2(id, scenario);
    }
    assert_eq!(executed + pending.len(), declared);
    println!(
        "ESS confinement macos lane: {executed} scenarios executed ({s1} macos-s1), {} macos-s2 \
         refused until {S2_STORY}, of {declared} declared; {audited} refusal audit records \
         asserted, 0 failed",
        pending.len()
    );
}

#[test]
fn s2_facts_are_absent_and_a_profile_requiring_each_is_refused_by_name() {
    let adapter = macos();
    let facts = adapter.facts();
    assert!(!facts.declared_insecure());
    assert!(facts.proves(&Fact::PathsAreHostPaths));
    assert!(facts.proves(&Fact::IpcSignalsContained));
    for fact in &S2_FACTS {
        assert!(
            !facts.proves(fact),
            "{fact:?} is proved: {S2_STORY} flips this assertion"
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
            let error = adapter
                .admit(&ConfinementProfile {
                    audit: Some(sink),
                    ..profile
                })
                .expect_err("refused until S2");
            match &error {
                ConfinementError::FactUnserved { fact, .. } => assert_eq!(
                    fact, &expected,
                    "refused naming another fact; {S2_STORY} flips this assertion"
                ),
                other => panic!(
                    "expected confinement.fact-unserved naming {expected:?} until {S2_STORY}: \
                     {other}"
                ),
            }
            assert_eq!(error.code(), "confinement.fact-unserved");
            assert_eq!(error.status(), 501);
            let record = serde_json::to_value(&records.lock()[0]).expect("record");
            assert_eq!(record["fact"], json!(fact_name(&expected)), "{record}");
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

fn bash(script: &str, args: &[&str]) -> Command {
    let mut argv = vec![BASH.to_owned(), "-c".into(), script.into(), "bash".into()];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
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
        assert_eq!(events(&records), ["profile_admitted"], "{host_path}");
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
