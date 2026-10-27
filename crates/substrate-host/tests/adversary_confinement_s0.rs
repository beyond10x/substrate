//! Adversary cases for `story:confinement-contract-on-linux-and-local` (portable lane).
//!
//! Each case asserts what the model (`spec/confinement/confinement.yaml`), its README or the
//! module's own documentation states, against the real adapters. A red case is a finding.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use substrate_host::confinement::{
    self, AdapterFacts, AdapterName, AdapterRequest, AdmittedProfile, AuditRecord, AuditSink,
    Command, ConfinementAdapter, ConfinementError, ConfinementProfile, ContractFamily, Fact,
    InnerAdapter, RenderedCommand, RunId, RunObservation, RunOutput, RunState, local::LocalRequest,
};
use substrate_wire::Signal;

const REASON: &str = "the adversary runs a host process on purpose";

fn local() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Local(LocalRequest::default())).expect("local opens")
}

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

fn events(records: &Mutex<Vec<AuditRecord>>) -> Vec<serde_json::Value> {
    records
        .lock()
        .iter()
        .map(|record| serde_json::to_value(record).expect("record"))
        .collect()
}

/// `profile.rs`: "The reason, when it is present and not blank." The implementor's decision is
/// that a blank reason counts as missing. A reason made only of invisible format characters
/// (zero-width space, byte-order mark) is blank to every reader of the audit record, and
/// `str::trim` keeps it.
#[test]
fn adv_a_reason_of_invisible_characters_is_no_reason() {
    let adapter = local();
    for reason in ["\u{200B}", "\u{FEFF}", "\u{200B}\u{2060}\u{200C}"] {
        let result = adapter.admit(&ConfinementProfile {
            local_reason: Some(reason.to_owned()),
            ..ConfinementProfile::default()
        });
        assert!(
            matches!(
                result,
                Err(ConfinementError::LocalReasonMissing {
                    adapter: AdapterName::Local
                })
            ),
            "a reason of invisible characters {:?} was admitted as a reason: {:?}",
            reason.escape_unicode().to_string(),
            result.map(|admitted| admitted.insecure_reason().map(str::to_owned))
        );
    }
}

/// README *Image digests*: "check that `@sha256:` is followed by exactly 64 lowercase hex
/// characters and nothing else". In this reference the first `@sha256:` is followed by 64 hex
/// characters and then by a second `@sha256:` and its digest.
#[test]
fn adv_an_image_naming_two_digests_is_not_pinned() {
    let digest = "a".repeat(64);
    let image = format!("registry.invalid/run@sha256:{digest}@sha256:{digest}");
    assert!(
        !confinement::image_is_pinned(&image),
        "{image} is accepted as pinned"
    );
}

/// What the `systemd` story's adapter will prove when it opens around `local`
/// (`confinement.yaml`, the declared set of "systemd around local"). The admission is the
/// trait's default `admit`, which is `confinement/mod.rs`'s `decide`; that file is S0's and the
/// systemd story does not edit it.
struct SystemdAroundLocal {
    facts: AdapterFacts,
}

impl SystemdAroundLocal {
    fn new() -> Self {
        Self {
            facts: AdapterFacts::new(
                AdapterName::Systemd,
                Some(InnerAdapter::Local),
                &[
                    (ContractFamily::ProcessRun, true),
                    (ContractFamily::ProcessWatch, true),
                    (ContractFamily::OutputRead, true),
                    (ContractFamily::Wait, true),
                    (ContractFamily::Workspace, true),
                    (ContractFamily::Limits, true),
                    (ContractFamily::Audit, true),
                ],
                &[
                    (Fact::DeclaredInsecure, true),
                    (Fact::PathsAreHostPaths, true),
                    (Fact::WholeTreeKill, true),
                    (Fact::LocalPermitted, true),
                    (Fact::HostNetwork, true),
                ],
            ),
        }
    }
}

#[async_trait]
impl ConfinementAdapter for SystemdAroundLocal {
    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    fn render(
        &self,
        _admitted: &AdmittedProfile,
        _command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        unreachable!("admission only")
    }

    async fn start(
        &self,
        _admitted: &AdmittedProfile,
        _command: &Command,
    ) -> Result<RunId, ConfinementError> {
        unreachable!("admission only")
    }

    async fn observe(&self, _run: &RunId) -> Result<RunObservation, ConfinementError> {
        unreachable!("admission only")
    }

    async fn output(&self, _run: &RunId) -> Result<RunOutput, ConfinementError> {
        unreachable!("admission only")
    }

    async fn wait(&self, _run: &RunId) -> Result<RunObservation, ConfinementError> {
        unreachable!("admission only")
    }

    async fn signal(&self, _run: &RunId, _signal: Signal) -> Result<(), ConfinementError> {
        unreachable!("admission only")
    }
}

/// `confinement.yaml`, `AdmitProfile` outcome `adapter-unserved`, first disjunct:
/// `{all: ["defined(inner)", "not defined(input.inner)"]}` — "`systemd` requested without the
/// `inner` it was opened around" is `confinement.adapter-unserved`. `decide` only compares when
/// the profile names an `inner`.
#[test]
fn adv_systemd_requested_without_its_inner_is_adapter_unserved() {
    let adapter = SystemdAroundLocal::new();
    confinement::check_open(adapter.facts()).expect("the declared set opens");
    // Control: the pair that opened is admitted.
    adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            inner: Some(InnerAdapter::Local),
            ..ConfinementProfile::default()
        })
        .expect("systemd around local admits a profile naming local");
    let (sink, records) = recording();
    let result = adapter.admit(&ConfinementProfile {
        local_reason: Some(REASON.into()),
        inner: None,
        audit: Some(sink),
        ..ConfinementProfile::default()
    });
    assert!(
        matches!(
            result,
            Err(ConfinementError::AdapterUnserved {
                adapter: AdapterName::Systemd,
                ..
            })
        ),
        "a systemd profile without inner was not refused adapter-unserved: {:?}; audit {:?}",
        result.map(|admitted| admitted.record()),
        events(&records)
    );
}

/// `confinement/mod.rs`: "Every admission and every refusal writes one record to the profile's
/// [`AuditSink`]." A start refused `confinement.adapter-unserved` (the profile was admitted by
/// another adapter) writes none.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_start_refused_adapter_unserved_writes_an_audit_record() {
    let first = local();
    let second = local();
    let workspace = tempfile::tempdir().expect("workspace");
    let (sink, records) = recording();
    let admitted = first
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let error = second
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect_err("refused");
    assert_eq!(error.code(), "confinement.adapter-unserved");
    let events = events(&records);
    assert!(
        events
            .iter()
            .any(|record| record["event"] == "profile_refused"
                && record["code"] == "confinement.adapter-unserved"),
        "the start's refusal {} wrote no audit record: {events:?}",
        error.code()
    );
}

/// The same rule on `linux` without a delegated cgroup: the probe proves no `exec`, so
/// `process_run` is not served. A profile naming only a workspace is admitted, and its start is
/// refused `confinement.family-unserved` naming `process_run` with no audit record.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_linux_start_refused_family_unserved_writes_an_audit_record() {
    let root = tempfile::tempdir().expect("root");
    let mut config = substrate_host::HostConfig::minimum(root.path().join("workspaces"));
    config.cgroup_root = None;
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        confinement::linux::LinuxRequest::new(config),
    )))
    .expect("linux opens without a delegated cgroup");
    assert!(!adapter.facts().serves(&ContractFamily::ProcessRun));
    let workspace = root.path().join("workspaces").join("ws_audit");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let (sink, records) = recording();
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("a workspace-only profile is admitted");
    let error = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect_err("process_run is not served");
    assert_eq!(error.code(), "confinement.family-unserved");
    let events = events(&records);
    assert!(
        events
            .iter()
            .any(|record| record["event"] == "profile_refused"
                && record["family"] == "process_run"),
        "the start's refusal naming process_run wrote no audit record: {events:?}"
    );
}

/// The story: "every observation carries `adapter: local, declared_insecure: <reason>`", and
/// `local.rs`: "Its own process group, so a signal reaches what it started and stayed in the
/// group." Once the leader has exited, `signal` returns `Ok` and delivers nothing, while
/// `observe` still reports the run `Running`: a background process the run started in its own
/// group keeps the run alive, and `wait` does not return.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_local_signal_reaches_the_group_after_the_leader_exits() {
    let adapter = local();
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "/bin/sleep 1000 & echo $! > child.pid; exit 0".into(),
                ],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let pid_file = workspace.path().join("child.pid");
    let mut child = None;
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            child = Some(pid);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let child = child.expect("the background child wrote its pid");
    // The leader has exited by now; give the reaper task a moment to record it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let observed = adapter.observe(&run).await.expect("observed");
    assert_eq!(observed.state, RunState::Running, "{observed:?}");
    adapter.signal(&run, Signal::Kill).await.expect("signalled");
    let ended = tokio::time::timeout(Duration::from_secs(5), adapter.wait(&run)).await;
    let alive = PathBuf::from(format!("/proc/{child}")).exists();
    // Clean up the orphan whatever the outcome.
    let _ = std::process::Command::new("/bin/kill")
        .args(["-KILL", &child.to_string()])
        .status();
    assert!(
        ended.is_ok() && !alive,
        "signal(KILL) returned Ok on a Running run and delivered nothing: wait ended = {}, \
         the run's background process {child} alive after the signal = {alive}",
        ended.is_ok()
    );
}

/// Design 26 and the delegated runtime vectors: a working directory that is a symlink is refused
/// `exec.workdir-missing` before dispatch. `linux` refuses it; `local` follows the link out of the
/// workspace and starts there, so one profile means two things depending on the adapter.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_local_workdir_symlinked_out_of_the_workspace_is_refused() {
    let adapter = local();
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("link")).expect("symlink");
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            workdir: Some("link".into()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let started = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), "printf here > marker".into()],
                env: BTreeMap::new(),
            },
        )
        .await;
    if let Ok(run) = &started {
        let _ = adapter.wait(run).await;
    }
    let escaped = outside.path().join("marker").exists();
    assert!(
        matches!(&started, Err(error) if error.code() == substrate_wire::EXEC_WORKDIR_MISSING),
        "a workdir symlinked out of the workspace was not refused: started = {}, wrote outside \
         the workspace = {escaped}",
        started.is_ok()
    );
}
