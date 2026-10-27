//! Adversary pass 1 on `story:pod-lane-runs-adversary-and-security`: what removes a run read
//! `Unknown`.
//!
//! pod.rs, `monitor`: "a removal not confirmed within the limit reports the run `Unknown` while
//! the adapter keeps trying". The trying is a task on the embedding process's runtime, and it is
//! two bounded rounds of `REMOVE_ATTEMPTS` polls, not an open-ended retry. Both cases stop the
//! kind node's kubelet, kill a run, see `wait` report `Unknown`, start the kubelet again, wait for
//! the kubelet to remove the Pod, and then require the run's default-deny `NetworkPolicy` to be
//! gone after a few of the adapter's own polls:
//!
//! * after the process that held the adapter has ended (its runtime is dropped, which is what
//!   process exit does to the cleanup task), with a fresh adapter opened on the same cluster, as a
//!   restarted daemon would;
//! * with the adapter alive throughout, when the kubelet stays away longer than the two rounds.
//!
//! No wait here is longer than an adapter bound: reaching `Unknown` takes the adapter's
//! `STOP_CONFIRM` (60 s), which no case can shorten. The lane variables are the pod lane's
//! (`scripts/pod-lane.sh`), which does not run these cases. They stop the kubelet, so run them
//! with `--test-threads=1`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::confinement::{
    Command, ConfinementAdapter, ConfinementProfile, RunState,
    pod::{self, PodAdapter, PodRequest},
};

const IMAGE: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
/// pod.rs `UNREACHABLE_LIMIT`: the adapter's longest bound on one removal.
const UNREACHABLE_LIMIT: Duration = Duration::from_mins(2);
/// Ten of pod.rs's `POLL` (500 ms): a policy removal follows its Pod's within one.
const POLICY_GRACE: Duration = Duration::from_secs(5);
/// pod.rs `monitor` after `Unknown`: two removal rounds of `REMOVE_ATTEMPTS` (60) polls of
/// `POLL` (500 ms) plus a delete and a get each, about 31 s a round on a local cluster.
const BOTH_ROUNDS: Duration = Duration::from_secs(75);

fn lane() -> Option<(PathBuf, String)> {
    if std::env::var_os("SUBSTRATE_POD_LANE").as_deref() != Some("1".as_ref()) {
        println!("adversary pod cleanup lane: ABSENT; 0 executed");
        return None;
    }
    let kubeconfig = std::env::var_os("SUBSTRATE_POD_KUBECONFIG").map(PathBuf::from);
    let node = std::env::var("SUBSTRATE_POD_KIND_NODE").ok();
    if let (Some(kubeconfig), Some(node)) = (kubeconfig, node) {
        Some((kubeconfig, node))
    } else {
        println!(
            "adversary pod cleanup lane: SUBSTRATE_POD_KUBECONFIG or SUBSTRATE_POD_KIND_NODE \
             unset; ABSENT; 0 executed"
        );
        None
    }
}

fn node(node: &str, args: &[&str]) -> String {
    let output = std::process::Command::new("docker")
        .args(["exec", node])
        .args(args)
        .output()
        .expect("docker runs");
    assert!(
        output.status.success(),
        "docker exec {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Starts the kubelet again however the case ends.
struct KubeletStopped(String);

impl KubeletStopped {
    fn stop(kind_node: &str) -> Self {
        node(kind_node, &["systemctl", "stop", "kubelet"]);
        Self(kind_node.to_owned())
    }
}

impl Drop for KubeletStopped {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["exec", &self.0, "systemctl", "start", "kubelet"])
            .status();
    }
}

fn objects(kubeconfig: &Path, namespace: &str, run: &str, kinds: &str) -> String {
    let output = std::process::Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args(["--namespace", namespace, "get", kinds])
        .arg(format!("--selector=b10x-substrate/run={run}"))
        .args(["--output", "name"])
        .output()
        .expect("kubectl runs");
    assert!(
        output.status.success(),
        "kubectl: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

/// Waits, at most `UNREACHABLE_LIMIT`, for the kubelet to remove the run's Pod, then
/// `POLICY_GRACE`; returns what of the run is left.
fn left_once_the_pod_is_gone(kubeconfig: &Path, namespace: &str, run: &str) -> String {
    let deadline = Instant::now() + UNREACHABLE_LIMIT;
    while !objects(kubeconfig, namespace, run, "pods").is_empty() {
        assert!(
            Instant::now() < deadline,
            "the kubelet did not remove the Pod"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    std::thread::sleep(POLICY_GRACE);
    objects(kubeconfig, namespace, run, "pods,networkpolicies")
}

fn open(kubeconfig: &Path) -> Arc<PodAdapter> {
    pod::open_pod(PodRequest {
        cluster: Some(kubeconfig.to_path_buf()),
    })
    .expect("opens")
}

/// Starts `sleep 600`, stops the kubelet, kills the run and requires `wait` to report `Unknown`.
/// Returns the run id; the kubelet stays stopped until the returned guard drops.
async fn killed_while_the_kubelet_is_away(
    adapter: &PodAdapter,
    kind_node: &str,
) -> (String, KubeletStopped) {
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let command = Command {
        argv: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "echo started; exec sleep 600".to_owned(),
        ],
        env: BTreeMap::new(),
    };
    let run = adapter.start(&admitted, &command).await.expect("started");
    let deadline = Instant::now() + Duration::from_mins(1);
    while !String::from_utf8_lossy(&adapter.output(&run).await.expect("output").stdout)
        .contains("started")
    {
        assert!(Instant::now() < deadline, "did not start");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let stopped = KubeletStopped::stop(kind_node);
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("killed");
    let ended = tokio::time::timeout(UNREACHABLE_LIMIT, adapter.wait(&run))
        .await
        .expect("wait returned")
        .expect("waited");
    assert_eq!(
        ended.state,
        RunState::Unknown,
        "the setup did not reach a run read Unknown"
    );
    println!("run {} reads Unknown", run.as_str());
    (run.as_str().to_owned(), stopped)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// Red until `story:pod-unknown-cleanup-outlives-adapter` lands. Not in `scripts/pod-lane.sh`:
/// run it by hand against a kind cluster with the pod lane's variables and `--test-threads=1`.
#[test]
fn adversary_lane_a_run_read_unknown_leaves_nothing_once_its_process_has_ended() {
    let Some((kubeconfig, kind_node)) = lane() else {
        return;
    };
    let started = Instant::now();
    let first = runtime();
    let (namespace, run, stopped) = first.block_on(async {
        let adapter = open(&kubeconfig);
        let (run, stopped) = killed_while_the_kubelet_is_away(&adapter, &kind_node).await;
        (adapter.settings().namespace.clone(), run, stopped)
    });
    // The embedding process ends right after `wait` returned: its runtime, and the adapter's
    // cleanup task on it, end with it.
    drop(first);
    drop(stopped);
    // The restarted embedder opens `pod` again on the same cluster and stays up.
    let second = runtime();
    let _adapter = second.block_on(async { open(&kubeconfig) });
    let left = left_once_the_pod_is_gone(&kubeconfig, &namespace, &run);
    println!("wall time {:?}", started.elapsed());
    assert!(
        left.is_empty(),
        "the kubelet removed the Pod of a run read Unknown whose process ended, and {left} is left"
    );
}

/// Red until `story:pod-unknown-cleanup-outlives-adapter` lands. Not in `scripts/pod-lane.sh`:
/// run it by hand against a kind cluster with the pod lane's variables and `--test-threads=1`.
#[tokio::test(flavor = "multi_thread")]
async fn adversary_lane_a_run_read_unknown_is_removed_after_a_longer_kubelet_absence() {
    let Some((kubeconfig, kind_node)) = lane() else {
        return;
    };
    let started = Instant::now();
    let adapter = open(&kubeconfig);
    let namespace = adapter.settings().namespace.clone();
    let (run, stopped) = killed_while_the_kubelet_is_away(&adapter, &kind_node).await;
    // The node stays away for both of `monitor`'s removal rounds, and no longer.
    tokio::time::sleep(BOTH_ROUNDS).await;
    drop(stopped);
    let left = tokio::task::spawn_blocking(move || {
        left_once_the_pod_is_gone(&kubeconfig, &namespace, &run)
    })
    .await
    .expect("polled");
    println!("wall time {:?}", started.elapsed());
    assert!(
        left.is_empty(),
        "the kubelet removed the Pod of a run read Unknown, the adapter is alive, and {left} is \
         left"
    );
    drop(adapter);
}
