//! Security-review conformance cases for `story:confinement-contract-on-linux-and-local` (S0).
//!
//! Each case states one invariant of the confinement contract and drives the real adapters
//! through `substrate_host::confinement` to check it. The delegated case is absent, never passed,
//! without `SUBSTRATE_VECTORS_CGROUP_ROOT`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use substrate_host::confinement::{
    self, AdapterFacts, AdapterRequest, AdmittedProfile, AuditRecord, AuditSink, Command,
    ConfinementAdapter, ConfinementError, ConfinementProfile, Fact, RenderedCommand, RunId,
    RunObservation, RunOutput,
    local::{LocalPolicy, LocalRequest},
};

const REASON: &str = "the security review runs a host process on purpose";

fn local(policy: LocalPolicy) -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Local(LocalRequest { policy })).expect("local opens")
}

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

/// A credential-shaped variable: the host driver's closed environment refuses the name
/// (`exec.environment-refused`, `process.rs` `validate_environment` / `is_secretish_name`).
fn credential_env() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        (
            "API_TOKEN".to_owned(),
            "security-review-probe-value".to_owned(),
        ),
    ])
}

#[cfg(target_os = "linux")]
fn linux(root: &Path, cgroup_root: Option<&Path>) -> Arc<dyn ConfinementAdapter> {
    let config = substrate_host::HostConfig::minimum(root.join("workspaces"));
    confinement::open(AdapterRequest::Linux(Box::new(
        confinement::linux::LinuxRequest::new(config)
            .with_cgroup_root(cgroup_root.map(Path::to_path_buf)),
    )))
    .expect("linux opens")
}

/// Invariant 4 of the review brief: `render` builds the argv `start` runs, so a command `start`
/// refuses is refused by `render` too, and a credential-named value never appears in an argv the
/// adapter would not run.
#[cfg(target_os = "linux")]
#[test]
fn linux_render_refuses_the_environment_start_refuses() {
    let root = tempfile::tempdir().expect("root");
    let adapter = linux(root.path(), None);
    let workspace = root.path().join("workspaces").join("ws_render_env");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace),
            ..ConfinementProfile::default()
        })
        .expect("a workspace-only profile is admitted");
    let rendered = adapter.render(
        &admitted,
        &Command {
            argv: vec!["/bin/true".into()],
            env: credential_env(),
        },
    );
    match rendered {
        Err(error) => assert_eq!(error.code(), "exec.environment-refused", "{error}"),
        Ok(rendered) => panic!(
            "render returned an argv for an environment start refuses \
             (exec.environment-refused); credential value in argv: {}",
            rendered
                .args
                .iter()
                .any(|arg| arg == "security-review-probe-value")
        ),
    }
}

/// The other half of the render finding, on the real sandbox: `start` refuses the same command.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn linux_start_refuses_a_credential_named_environment() {
    let Some(cgroup_root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") else {
        println!(
            "security review linux start env: ABSENT (SUBSTRATE_VECTORS_CGROUP_ROOT is unset)"
        );
        return;
    };
    let cgroup_root = std::path::PathBuf::from(cgroup_root);
    std::fs::write(
        cgroup_root.join("cgroup.subtree_control"),
        "+cpu +memory +pids",
    )
    .expect("delegated controllers");
    let root = tempfile::tempdir().expect("root");
    let adapter = linux(root.path(), Some(&cgroup_root));
    let workspace = root.path().join("workspaces").join("ws_start_env");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let command = Command {
        argv: vec!["/bin/true".into()],
        env: credential_env(),
    };
    let error = adapter
        .start(&admitted, &command)
        .await
        .expect_err("start refuses a credential-named variable");
    assert_eq!(error.code(), "exec.environment-refused", "{error}");
    let rendered = adapter.render(&admitted, &command);
    println!(
        "security review linux start env: executed; start refused {}, render {}",
        error.code(),
        match &rendered {
            Ok(_) => "returned an argv".to_owned(),
            Err(error) => format!("refused {}", error.code()),
        }
    );
}

/// Invariant 6 of the review brief: no secret reaches `local`. `local` refuses sealed slots, and
/// the environment shape the host driver holds every run to (credential-shaped names refused)
/// applies to a `local` start as well.
#[tokio::test(flavor = "multi_thread")]
async fn local_refuses_a_credential_named_environment() {
    let adapter = local(LocalPolicy::FromHost);
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let started = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf %s \"$API_TOKEN\"".into(),
                ],
                env: credential_env(),
            },
        )
        .await;
    match started {
        Err(error) => assert_eq!(error.code(), "exec.environment-refused", "{error}"),
        Ok(run) => {
            adapter.wait(&run).await.expect("waited");
            let output = adapter.output(&run).await.expect("output");
            panic!(
                "local started a host process with a credential-named variable; the child read \
                 it back: {}",
                output.stdout == b"security-review-probe-value"
            );
        }
    }
}

/// ADR 0109 decision 2: every start, refusal and route decision is an audit record. A start the
/// adapter refuses after admission writes one.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_start_writes_an_audit_record() {
    let adapter = local(LocalPolicy::FromHost);
    let workspace = tempfile::tempdir().expect("workspace");
    let (sink, records) = recording();
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            workdir: Some("../outside".into()),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let error = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect_err("a workdir outside the workspace is refused");
    assert_eq!(error.code(), substrate_wire::EXEC_WORKDIR_INVALID);
    let events: Vec<serde_json::Value> = records
        .lock()
        .iter()
        .map(|record| serde_json::to_value(record).expect("record"))
        .collect();
    assert!(
        events.len() == 2 && events[1]["code"] == substrate_wire::EXEC_WORKDIR_INVALID,
        "the refused start wrote no audit record naming {}: {events:?}",
        substrate_wire::EXEC_WORKDIR_INVALID
    );
}

/// Invariant 5 of the review brief: the working directory is beneath the workspace. `linux`
/// refuses a symlinked workdir (`exec.workdir-missing`, design 26); `local` names the same
/// refusal for "not a directory beneath the workspace".
#[tokio::test(flavor = "multi_thread")]
async fn local_workdir_does_not_follow_a_symlink_out_of_the_workspace() {
    let adapter = local(LocalPolicy::FromHost);
    let host = tempfile::tempdir().expect("host");
    let workspace = host.path().join("workspace");
    let outside = host.path().join("outside");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, workspace.join("link")).expect("link");
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace),
            workdir: Some("link".into()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let started = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), "pwd -P".into()],
                env: BTreeMap::new(),
            },
        )
        .await;
    match started {
        Err(error) => assert_eq!(
            error.code(),
            substrate_wire::EXEC_WORKDIR_MISSING,
            "{error}"
        ),
        Ok(run) => {
            adapter.wait(&run).await.expect("waited");
            let output = adapter.output(&run).await.expect("output");
            panic!(
                "local started in a directory outside the workspace: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            );
        }
    }
}

/// An adapter that hands out a copy of another adapter's proof: `AdapterFacts` is `Clone`, its
/// `facts` list is public, and the copy keeps the original's instance.
struct CopiedProof {
    facts: AdapterFacts,
}

#[async_trait]
impl ConfinementAdapter for CopiedProof {
    fn facts(&self) -> &AdapterFacts {
        &self.facts
    }

    fn render(
        &self,
        _admitted: &AdmittedProfile,
        _command: &Command,
    ) -> Result<RenderedCommand, ConfinementError> {
        unreachable!("only admit is called")
    }

    async fn start(
        &self,
        _admitted: &AdmittedProfile,
        _command: &Command,
    ) -> Result<RunId, ConfinementError> {
        unreachable!("only admit is called")
    }

    async fn observe(&self, _run: &RunId) -> Result<RunObservation, ConfinementError> {
        unreachable!("only admit is called")
    }

    async fn output(&self, _run: &RunId) -> Result<RunOutput, ConfinementError> {
        unreachable!("only admit is called")
    }

    async fn wait(&self, _run: &RunId) -> Result<RunObservation, ConfinementError> {
        unreachable!("only admit is called")
    }

    async fn signal(
        &self,
        _run: &RunId,
        _signal: substrate_wire::Signal,
    ) -> Result<(), ConfinementError> {
        unreachable!("only admit is called")
    }
}

/// `mod.rs`: "Only admission makes one, and only the adapter that admitted it starts it", and
/// `local.rs`: "An embedder can only tighten the host's policy". A `local` opened under a refusing
/// policy does not start a profile admitted against a copy of its proof with `local_permitted`
/// added.
#[tokio::test(flavor = "multi_thread")]
async fn a_copied_proof_does_not_start_local_on_a_host_that_refuses_it() {
    let refused = local(LocalPolicy::Refuse);
    assert!(!refused.facts().proves(&Fact::LocalPermitted));
    let mut facts = refused.facts().clone();
    facts.facts.push(Fact::LocalPermitted);
    let copied = CopiedProof { facts };
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = copied
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("the copy admits");
    let started = refused
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
            },
        )
        .await;
    match started {
        Err(error) => assert_eq!(error.code(), "confinement.local-refused", "{error}"),
        Ok(run) => {
            refused.wait(&run).await.expect("waited");
            panic!("local ran a host process on an adapter opened under a refusing policy");
        }
    }
}

const POLICY_PROBE: &str = "SECURITY_REVIEW_POLICY_PROBE";

/// Runs only inside `the_host_policy_variable_refuses_local_unless_it_is_exactly_permit`, which
/// sets `SUBSTRATE_CONFINEMENT_LOCAL` for this process alone.
#[test]
fn host_policy_probe() {
    let Some(expected) = std::env::var_os(POLICY_PROBE) else {
        return;
    };
    let adapter = local(LocalPolicy::FromHost);
    let profile = ConfinementProfile {
        local_reason: Some(REASON.into()),
        ..ConfinementProfile::default()
    };
    match expected.to_str() {
        Some("permitted") => {
            assert!(adapter.facts().proves(&Fact::LocalPermitted));
            adapter.admit(&profile).expect("permitted");
        }
        Some("refused") => {
            assert!(!adapter.facts().proves(&Fact::LocalPermitted));
            let error = adapter.admit(&profile).expect_err("refused");
            assert_eq!(error.code(), "confinement.local-refused");
            assert_eq!(error.status(), 403);
        }
        other => panic!("unknown probe expectation {other:?}"),
    }
    let restricted = local(LocalPolicy::Refuse);
    assert!(!restricted.facts().proves(&Fact::LocalPermitted));
    println!("POLICY-PROBE-RAN");
}

/// ADR 0109 F3 A: the host refuses `local` through `SUBSTRATE_CONFINEMENT_LOCAL`, and any value but
/// exactly `permit` forbids it. Each value runs in its own process, so no other case sees it.
#[test]
fn the_host_policy_variable_refuses_local_unless_it_is_exactly_permit() {
    let program = std::env::current_exe().expect("test binary");
    for (value, expected) in [
        (Some("permit"), "permitted"),
        (None, "permitted"),
        (Some("refuse"), "refused"),
        (Some(""), "refused"),
        (Some("PERMIT"), "refused"),
        (Some("permit "), "refused"),
        (Some("1"), "refused"),
    ] {
        let mut child = std::process::Command::new(&program);
        child
            .args([
                "--exact",
                "host_policy_probe",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(POLICY_PROBE, expected);
        match value {
            Some(value) => child.env(confinement::local::LOCAL_POLICY_ENV, value),
            None => child.env_remove(confinement::local::LOCAL_POLICY_ENV),
        };
        let output = child.output().expect("probe runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success()
                && stdout.contains("POLICY-PROBE-RAN")
                && stdout.contains("1 passed"),
            "policy {value:?} expected {expected}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Invariant 5 of the review brief, under `linux`: a workspace that is a symlink beneath the root,
/// that climbs out with `..`, or that is relative is refused before any argv is produced.
#[cfg(target_os = "linux")]
#[test]
fn linux_render_refuses_a_workspace_outside_its_root() {
    let host = tempfile::tempdir().expect("host");
    let adapter = linux(host.path(), None);
    let root = host.path().join("workspaces");
    let outside = host.path().join("outside");
    std::fs::create_dir_all(root.join("ws_real")).expect("workspace");
    std::fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, root.join("ws_link")).expect("link");
    let command = Command {
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
    };
    for workspace in [
        root.join("ws_link"),
        root.join("ws_real").join("..").join("..").join("outside"),
        std::path::PathBuf::from("ws_real"),
        root.clone(),
    ] {
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.clone()),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let rendered = adapter.render(&admitted, &command);
        assert!(
            rendered.is_err(),
            "{} rendered: {rendered:?}",
            workspace.display()
        );
    }
}
