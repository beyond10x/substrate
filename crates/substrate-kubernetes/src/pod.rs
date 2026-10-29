//! The exec Pod (design 34 § 7.3, ADR 0047 decision 6): design 28's rendering under Pod Security
//! `restricted`, with the executor copied in by an init container and run as the container's
//! PID 1, the workspace PVC as the only writable mount, and the run spec carried as base64url
//! argument parts. A pure function of its inputs, held to `tests/golden/exec-pod.json`.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use substrate_executor::frame::MAX_CHUNK_BYTES;
use substrate_kubernetes_client::RUN_CONTAINER;
use substrate_wire::ExecLimits;

/// The init container that copies the executor into the shared volume.
pub const EXECUTOR_CONTAINER: &str = "executor";
/// Where the init container writes the executor: the shared volume, mounted writable for it.
pub const INSTALL_MOUNT: &str = "/x";
/// Where the run container sees the shared volume, read-only.
pub const EXECUTOR_MOUNT: &str = "/.substrate";
/// The workspace PVC's mount, the only writable one of the run container.
pub const WORKSPACE_MOUNT: &str = substrate_wire::EXEC_WORKSPACE_MOUNT;
/// The shared volume's `sizeLimit`, and the ephemeral storage it adds to a run.
pub const EXECUTOR_VOLUME_BYTES: u64 = 64 * 1_048_576;
/// `terminationGracePeriodSeconds`: what a `term` signal's deletion waits for.
pub const TERM_GRACE_SECONDS: u64 = 5;
/// The grace of a `kill` signal's deletion, and of the deletion that follows a shown end.
pub const KILL_GRACE_SECONDS: u64 = 1;
/// The node label the exec Pod is pinned to its keeper's node by (ADR 0047 decision 5).
pub const HOSTNAME_KEY: &str = "kubernetes.io/hostname";
/// The bytes one full frame line takes in the container log, as § 7.3 counts it.
pub const FRAME_LINE_BYTES: u64 = 8_202;
/// The control lines' allowance in the log budget of § 7.3.
pub const CONTROL_BYTES: u64 = 65_536;
/// The cap on `exec.output-limit-bytes`.
pub const MAX_OUTPUT_LIMIT_BYTES: u64 = 1_048_576;

const VOLUME_WORKSPACE: &str = "workspace";
const VOLUME_EXECUTOR: &str = "substrate";
const MAX_TIMEOUT_MS: u64 = 86_400_000;
const MAX_PROCESSES: u32 = 4_096;
const MIN_MEMORY_BYTES: u64 = 1_048_576;
const MAX_MEMORY_BYTES: u64 = 1_099_511_627_776;
const MAX_CPU_MILLIS: u64 = 86_400_000;

/// `exec.output-limit-bytes` for a kubelet keeping `container_log_max_bytes` per log file: the
/// largest L with `2 × ceil(L / 6144) × 8202 + 65536 ≤ container_log_max_bytes`, capped at 1 MiB.
/// Zero when not even one chunk per stream fits.
#[must_use]
pub fn output_limit_bytes(container_log_max_bytes: u64) -> u64 {
    let chunk = MAX_CHUNK_BYTES as u64;
    let chunks = container_log_max_bytes.saturating_sub(CONTROL_BYTES) / (2 * FRAME_LINE_BYTES);
    (chunks * chunk).min(MAX_OUTPUT_LIMIT_BYTES)
}

/// The most log bytes a run bounded at `output_bytes` per stream can write: both streams in full
/// frame lines plus the control allowance. A log longer than this is not the executor's.
#[must_use]
pub fn log_budget_bytes(output_bytes: u64) -> u64 {
    2 * output_bytes.div_ceil(MAX_CHUNK_BYTES as u64) * FRAME_LINE_BYTES + CONTROL_BYTES
}

/// The bounds one exec Pod is rendered with, resolved from a start's limits as design 28 resolves
/// a profile's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBounds {
    /// `activeDeadlineSeconds`: the wall bound, rounded up to whole seconds.
    pub deadline_seconds: u64,
    /// The CPU rate that, held for the whole wall bound, spends at most the CPU budget.
    pub millicores: u64,
    /// Memory request and limit.
    pub memory_bytes: u64,
    /// The executor's per-stream output bound.
    pub output_bytes: u64,
}

impl RunBounds {
    /// Resolves `limits` for a cluster whose stated per-Pod process bound is `pod_pids_limit` and
    /// whose log budget allows `output_limit` per stream.
    ///
    /// # Errors
    ///
    /// What is wrong, when a bound is outside the host driver's ranges, the CPU budget is under
    /// one millicore over the wall bound, the output bound passes `output_limit`, or the stated
    /// process bound is above the requested one (design 34 § 14 Q2 A): the caller refuses
    /// `exec.limit-unserved`.
    pub fn resolve(
        limits: &ExecLimits,
        pod_pids_limit: u32,
        output_limit: u64,
    ) -> Result<Self, &'static str> {
        if limits.timeout_ms == 0
            || limits.timeout_ms > MAX_TIMEOUT_MS
            || limits.processes == 0
            || limits.processes > MAX_PROCESSES
            || limits.memory_bytes < MIN_MEMORY_BYTES
            || limits.memory_bytes > MAX_MEMORY_BYTES
            || limits.cpu_millis == 0
            || limits.cpu_millis > MAX_CPU_MILLIS
            || limits.output_bytes == 0
        {
            return Err("a bound is outside the driver's ranges");
        }
        if limits.output_bytes > output_limit {
            return Err("the output bound passes exec.output-limit-bytes");
        }
        if pod_pids_limit > limits.processes {
            return Err("the cluster's stated per-Pod process bound is above the requested one");
        }
        let deadline_seconds = limits.timeout_ms.div_ceil(1000);
        let millicores = limits.cpu_millis / deadline_seconds;
        if millicores == 0 {
            return Err("the CPU budget is under one millicore over the wall bound");
        }
        Ok(Self {
            deadline_seconds,
            millicores,
            memory_bytes: limits.memory_bytes,
            output_bytes: limits.output_bytes,
        })
    }
}

/// Everything one exec Pod is rendered from.
#[derive(Debug, Clone)]
pub struct ExecPod<'a> {
    /// The namespace.
    pub namespace: &'a str,
    /// `sbx-ex-<h>`.
    pub name: &'a str,
    /// The four labels of § 4.
    pub labels: BTreeMap<&'static str, String>,
    /// The `b10x-substrate/operation` annotation's value.
    pub operation: String,
    /// The workspace PVC: its name and the uid its create answered.
    pub claim: (&'a str, &'a str),
    /// The keeper's `spec.nodeName`.
    pub node: &'a str,
    /// The executor image, by digest.
    pub executor_image: &'a str,
    /// The exec image, by digest.
    pub exec_image: &'a str,
    /// The uid and gid every container runs as.
    pub run_as_user: u32,
    /// The run spec's base64url parts.
    pub spec_parts: Vec<String>,
    /// The resolved bounds.
    pub bounds: RunBounds,
    /// The kubelet's stated `containerLogMaxSize`.
    pub container_log_max_bytes: u64,
    /// The workspace is mounted read-only (`workspace_access: read-only`).
    pub read_only: bool,
}

/// The Pod an exec creates. Nothing is sent anywhere.
#[must_use]
pub fn render_exec_pod(pod: &ExecPod<'_>) -> Value {
    let (claim, claim_uid) = pod.claim;
    let resources = json!({
        "cpu": format!("{}m", pod.bounds.millicores),
        "memory": pod.bounds.memory_bytes.to_string(),
        "ephemeral-storage": (EXECUTOR_VOLUME_BYTES + 2 * pod.container_log_max_bytes).to_string(),
    });
    let mut command = vec![
        format!("{EXECUTOR_MOUNT}/substrate-executor"),
        "run".to_owned(),
    ];
    command.extend(pod.spec_parts.iter().cloned());
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": pod.name,
            "namespace": pod.namespace,
            "labels": pod.labels,
            "annotations": {crate::names::OPERATION_ANNOTATION: pod.operation},
            "ownerReferences": [{
                "apiVersion": "v1",
                "kind": "PersistentVolumeClaim",
                "name": claim,
                "uid": claim_uid,
                "controller": false,
            }],
        },
        "spec": {
            "activeDeadlineSeconds": pod.bounds.deadline_seconds,
            "affinity": {"nodeAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": {
                "nodeSelectorTerms": [{"matchExpressions": [{
                    "key": HOSTNAME_KEY, "operator": "In", "values": [pod.node],
                }]}],
            }}},
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "hostIPC": false,
            "hostNetwork": false,
            "hostPID": false,
            "restartPolicy": "Never",
            "terminationGracePeriodSeconds": TERM_GRACE_SECONDS,
            "securityContext": security_context(pod.run_as_user, false),
            "volumes": [
                {"name": VOLUME_WORKSPACE, "persistentVolumeClaim": {"claimName": claim}},
                {"name": VOLUME_EXECUTOR, "emptyDir": {"sizeLimit": EXECUTOR_VOLUME_BYTES.to_string()}},
            ],
            "initContainers": [{
                "name": EXECUTOR_CONTAINER,
                "image": pod.executor_image,
                "imagePullPolicy": "IfNotPresent",
                "command": [
                    "/substrate-executor",
                    "install",
                    format!("{INSTALL_MOUNT}/substrate-executor"),
                ],
                "resources": {"requests": resources, "limits": resources},
                "securityContext": security_context(pod.run_as_user, true),
                "volumeMounts": [{"name": VOLUME_EXECUTOR, "mountPath": INSTALL_MOUNT}],
            }],
            "containers": [{
                "name": RUN_CONTAINER,
                "image": pod.exec_image,
                "imagePullPolicy": "IfNotPresent",
                "command": command,
                "workingDir": WORKSPACE_MOUNT,
                "resources": {"requests": resources, "limits": resources},
                "securityContext": security_context(pod.run_as_user, true),
                "volumeMounts": [
                    {"name": VOLUME_WORKSPACE, "mountPath": WORKSPACE_MOUNT, "readOnly": pod.read_only},
                    {"name": VOLUME_EXECUTOR, "mountPath": EXECUTOR_MOUNT, "readOnly": true},
                ],
                "stdin": false,
                "tty": false,
            }],
        },
    })
}

/// Design 28's security context: the Pod's, or with `container` a container's, which adds every
/// member Pod Security `restricted` requires of one.
fn security_context(run_as_user: u32, container: bool) -> Value {
    let mut context = json!({
        "runAsNonRoot": true,
        "runAsUser": run_as_user,
        "runAsGroup": run_as_user,
        "seccompProfile": {"type": "RuntimeDefault"},
    });
    if container {
        context["allowPrivilegeEscalation"] = json!(false);
        context["privileged"] = json!(false);
        context["readOnlyRootFilesystem"] = json!(true);
        context["capabilities"] = json!({"drop": ["ALL"]});
    }
    context
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use substrate_executor::spec::RunSpec;
    use substrate_host::{BackendKind, BackendRole};
    use substrate_wire::ExecLimits;

    use super::{ExecPod, RunBounds, log_budget_bytes, output_limit_bytes, render_exec_pod};
    use crate::names;

    const GOLDEN: &str = include_str!("../tests/golden/exec-pod.json");
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn limits() -> ExecLimits {
        ExecLimits {
            timeout_ms: 30_500,
            output_bytes: 65_536,
            processes: 1_024,
            memory_bytes: 67_108_864,
            cpu_millis: 15_500,
        }
    }

    /// The exec Pod rendered from fixed inputs is the committed golden, byte for byte: no `env`,
    /// no token, `restricted`'s fields on both containers, the PVC the only writable mount, the
    /// executor read-only, the Pod pinned to the keeper's node and owned by the PVC.
    #[test]
    fn the_exec_pod_renders_as_its_golden() {
        let deployment = "deploy-a";
        let id = "exec_0001";
        let hash = names::resource_hash(deployment, id);
        let name = names::exec_pod(deployment, id);
        let claim = names::workspace_pvc(deployment, "ws_0001");
        let spec = RunSpec {
            argv: vec!["/bin/echo".to_owned(), "$(HOME) $$".to_owned()],
            env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            memory_bytes: 67_108_864,
            cpu_millicores: 500,
            output_bytes: 65_536,
        };
        let bounds = RunBounds::resolve(&limits(), 1_024, 1_048_576).expect("in range");
        assert_eq!(bounds.millicores, 500);
        assert_eq!(bounds.deadline_seconds, 31);
        let executor = format!("registry.invalid/substrate/executor@sha256:{DIGEST}");
        let image = format!("registry.invalid/busybox@sha256:{DIGEST}");
        let rendered = render_exec_pod(&ExecPod {
            namespace: "substrate",
            name: &name,
            labels: names::labels(deployment, BackendKind::Exec, &hash, BackendRole::Pod),
            operation: names::operation_annotation(&"ab".repeat(32)),
            claim: (&claim, "00000000-0000-4000-8000-000000000001"),
            node: "node-a",
            executor_image: &executor,
            exec_image: &image,
            run_as_user: 65_532,
            spec_parts: spec.encode_parts(),
            bounds,
            container_log_max_bytes: 10_485_760,
            read_only: false,
        });
        let text = serde_json::to_string_pretty(&rendered).expect("serializes") + "\n";
        if std::env::var_os("SUBSTRATE_BLESS_GOLDENS").is_some() {
            std::fs::write(
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/exec-pod.json"),
                &text,
            )
            .expect("the golden is written");
        }
        assert_eq!(text, GOLDEN, "the exec Pod moved from its golden");
        assert!(!text.contains('$'), "kubelet expands nothing in the Pod");
        assert!(
            !text.contains("\"env\""),
            "the run container carries no env"
        );
    }

    /// § 7.3's budget: the largest L whose two full streams of frame lines and the control
    /// allowance fit one kubelet log file, capped at 1 MiB; the log read is capped at that budget.
    #[test]
    fn the_output_limit_fits_one_log_file() {
        assert_eq!(output_limit_bytes(10_485_760), 1_048_576);
        let limit = output_limit_bytes(1_048_576);
        assert_eq!(limit, 59 * 6_144);
        assert!(log_budget_bytes(limit) <= 1_048_576);
        assert!(log_budget_bytes(limit + 6_144) > 1_048_576);
        assert_eq!(output_limit_bytes(65_536 + 16_403), 0);
    }

    /// Each bound outside its range, a CPU budget under one millicore over the wall bound, an
    /// output bound past the log budget and a stated process bound above the requested one are
    /// refused, never rendered.
    #[test]
    fn bounds_the_pod_cannot_hold_are_refused() {
        let mut cases = Vec::new();
        for change in [
            |l: &mut ExecLimits| l.timeout_ms = 0,
            |l: &mut ExecLimits| l.memory_bytes = 1,
            |l: &mut ExecLimits| l.processes = 0,
            |l: &mut ExecLimits| l.output_bytes = 0,
            |l: &mut ExecLimits| l.output_bytes = 2_000_000,
            |l: &mut ExecLimits| l.processes = 512,
            |l: &mut ExecLimits| {
                l.cpu_millis = 10;
                l.timeout_ms = 60_000;
            },
        ] {
            let mut changed = limits();
            change(&mut changed);
            cases.push(RunBounds::resolve(&changed, 1_024, 1_048_576));
        }
        assert!(cases.iter().all(Result::is_err), "{cases:?}");
    }
}
