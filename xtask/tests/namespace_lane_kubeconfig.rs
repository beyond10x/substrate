//! `cargo xtask namespace-lane observe` reads only the kubeconfig it is given (design 34 § 10.2):
//! an ambient `KUBECONFIG` changes nothing, a given one whose server is not kind's loopback
//! endpoint is refused, and one under `~/.kube` is refused before it is read. Each case refuses
//! before any cluster, registry or API call.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn kubeconfig(server: &str) -> String {
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "lane",
        "clusters": [{"name": "lane", "cluster": {"server": server, "certificate-authority-data": "Q0E="}}],
        "users": [{"name": "lane", "user": {"token": "not-a-token"}}],
        "contexts": [{"name": "lane", "context": {"cluster": "lane", "user": "lane", "namespace": "lane"}}],
    })
    .to_string()
}

fn observe(home: &Path, ambient: &Path, given: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(root())
        .env("HOME", home)
        .env("KUBECONFIG", ambient)
        .args(["namespace-lane", "observe", "--admin-kubeconfig"])
        .arg(given)
        .output()
        .expect("xtask runs")
}

#[test]
fn a_given_non_loopback_server_is_refused_whatever_the_ambient_kubeconfig_says() {
    let home = tempfile::tempdir().expect("home");
    let ambient = home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41234")).expect("ambient");
    let given = home.path().join("given.json");
    fs::write(&given, kubeconfig("https://203.0.113.9:6443")).expect("given");
    let run = observe(home.path(), &ambient, &given);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stderr}");
    assert!(stderr.contains("https://203.0.113.9:6443"), "{stderr}");
    assert!(stderr.contains("https://127.0.0.1:<port>"), "{stderr}");
    assert!(!stderr.contains("41234"), "{stderr}");
    assert!(!String::from_utf8_lossy(&run.stdout).contains("Namespace-lane:"));
}

/// A kubeconfig whose current context is `context`, naming the cluster `cluster`, with `extra`
/// members on the cluster and on the user.
fn named_kubeconfig(
    context: &str,
    cluster: &str,
    cluster_extra: &[(&str, serde_json::Value)],
    user_extra: &[(&str, serde_json::Value)],
) -> String {
    let mut cluster_object = serde_json::json!({
        "server": "https://127.0.0.1:41234",
        "certificate-authority-data": "Q0E=",
    });
    for (member, value) in cluster_extra {
        cluster_object[*member] = value.clone();
    }
    let mut user = serde_json::json!({"token": "not-a-token"});
    for (member, value) in user_extra {
        user[*member] = value.clone();
    }
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": context,
        "clusters": [{"name": cluster, "cluster": cluster_object}],
        "users": [{"name": "admin", "user": user}],
        "contexts": [{"name": context, "context": {"cluster": cluster, "user": "admin", "namespace": "lane"}}],
    })
    .to_string()
}

/// A stand-in `docker` and `kind` that record their argv and answer only `network inspect`.
struct StandIn {
    directory: tempfile::TempDir,
}

impl StandIn {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let stand = Self {
            directory: tempfile::tempdir().expect("tools"),
        };
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\nif [ \"$1\" = network ]; then printf '%s\\n' '[{{\"Gateway\":\"172.18.0.1\"}}]'; exit 0; fi\nexit 1\n",
            stand.log().display()
        );
        for name in ["docker", "kind"] {
            let path = stand.directory.path().join(name);
            fs::write(&path, &script).expect("stand-in");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
        }
        stand
    }

    fn log(&self) -> PathBuf {
        self.directory.path().join("argv.log")
    }

    fn observe(&self, home: &Path, ambient: &Path, given: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .current_dir(root())
            .env("HOME", home)
            .env("KUBECONFIG", ambient)
            .args(["namespace-lane", "observe", "--docker"])
            .arg(self.directory.path().join("docker"))
            .arg("--kind")
            .arg(self.directory.path().join("kind"))
            .arg("--admin-kubeconfig")
            .arg(given)
            .output()
            .expect("xtask runs")
    }
}

#[test]
fn a_given_kubeconfig_for_a_cluster_this_lane_did_not_create_is_refused_before_anything_runs() {
    let stand = StandIn::new();
    let home = tempfile::tempdir().expect("home");
    let ambient = home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235")).expect("ambient");
    let lane = "kind-substrate-namespace-lane-0a1b2c3d4e5f";
    let cases = [
        ("kind-b10x-dev", "kind-b10x-dev", "kind-b10x-dev"),
        ("lane", "lane", "lane"),
        (
            "kind-substrate-namespace-lane-",
            "kind-substrate-namespace-lane-",
            "\"kind-substrate-namespace-lane-\"",
        ),
        (
            lane,
            "kind-substrate-namespace-lane-ffffffffffff",
            "kind-substrate-namespace-lane-ffffffffffff",
        ),
        (lane, "kind-b10x-dev", "kind-b10x-dev"),
        (
            "substrate-namespace-lane-0a1b2c3d4e5f",
            "substrate-namespace-lane-0a1b2c3d4e5f",
            "substrate-namespace-lane-0a1b2c3d4e5f",
        ),
        (
            "kind-substrate-namespace-lane-UPPER",
            "kind-substrate-namespace-lane-UPPER",
            "kind-substrate-namespace-lane-UPPER",
        ),
    ];
    for (index, (context, cluster, named)) in cases.into_iter().enumerate() {
        let given = home.path().join(format!("given-{index}.json"));
        fs::write(&given, named_kubeconfig(context, cluster, &[], &[])).expect("given");
        let run = stand.observe(home.path(), &ambient, &given);
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{context}/{cluster}: {stderr}");
        assert!(
            stderr.contains("this lane did not create"),
            "{context}/{cluster}: {stderr}"
        );
        assert!(stderr.contains(named), "{context}/{cluster}: {stderr}");
        assert!(!String::from_utf8_lossy(&run.stdout).contains("Namespace-lane:"));
    }
    assert!(
        !stand.log().exists(),
        "a tool ran before the refusal: {:?}",
        fs::read_to_string(stand.log())
    );
}

#[test]
fn a_lane_named_kubeconfig_passes_the_name_check_and_stops_before_any_write() {
    let stand = StandIn::new();
    let home = tempfile::tempdir().expect("home");
    let ambient = home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235")).expect("ambient");
    let lane = "kind-substrate-namespace-lane-0a1b2c3d4e5f";
    let given = home.path().join("given.json");
    fs::write(&given, named_kubeconfig(lane, lane, &[], &[])).expect("given");
    let run = stand.observe(home.path(), &ambient, &given);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stderr}");
    assert!(!stderr.contains("this lane did not create"), "{stderr}");
    assert!(stderr.contains("certificate-authority-data"), "{stderr}");
    assert!(!String::from_utf8_lossy(&run.stdout).contains("Namespace-lane:"));
}

#[test]
fn a_given_cluster_or_user_member_kind_never_writes_is_refused_by_name_before_anything_runs() {
    let stand = StandIn::new();
    let home = tempfile::tempdir().expect("home");
    let ambient = home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235")).expect("ambient");
    let lane = "kind-substrate-namespace-lane-0a1b2c3d4e5f";
    let text = serde_json::json!("x");
    let cases: Vec<(&str, String)> = vec![
        (
            "tls-server-name",
            named_kubeconfig(lane, lane, &[("tls-server-name", text.clone())], &[]),
        ),
        (
            "proxy-url",
            named_kubeconfig(
                lane,
                lane,
                &[("proxy-url", serde_json::json!("http://127.0.0.1:3128"))],
                &[],
            ),
        ),
        (
            "insecure-skip-tls-verify",
            named_kubeconfig(
                lane,
                lane,
                &[("insecure-skip-tls-verify", serde_json::json!(true))],
                &[],
            ),
        ),
        (
            "exec",
            named_kubeconfig(
                lane,
                lane,
                &[],
                &[("exec", serde_json::json!({"command": "true"}))],
            ),
        ),
        (
            "auth-provider",
            named_kubeconfig(
                lane,
                lane,
                &[],
                &[("auth-provider", serde_json::json!({"name": "oidc"}))],
            ),
        ),
    ];
    for (index, (member, config)) in cases.into_iter().enumerate() {
        let given = home.path().join(format!("given-{index}.json"));
        fs::write(&given, config).expect("given");
        let run = stand.observe(home.path(), &ambient, &given);
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{member}: {stderr}");
        assert!(
            stderr.contains(&format!("member {member}")),
            "{member}: {stderr}"
        );
        assert!(!String::from_utf8_lossy(&run.stdout).contains("Namespace-lane:"));
    }
    assert!(
        !stand.log().exists(),
        "a tool ran before the refusal: {:?}",
        fs::read_to_string(stand.log())
    );
}

#[test]
fn every_entry_of_a_kubeconfig_list_is_refused_when_given_whatever_its_spelling() {
    let home = tempfile::tempdir().expect("home");
    let first = home.path().join("first.json");
    let second = home.path().join("second.json");
    for path in [&first, &second] {
        fs::write(path, kubeconfig("https://127.0.0.1:41234")).expect("config");
    }
    let link = home.path().join("link.json");
    std::os::unix::fs::symlink(&second, &link).expect("link");
    let list =
        std::env::join_paths([first.as_path(), Path::new(""), link.as_path()]).expect("list");
    for given in [
        first.clone(),
        second.clone(),
        home.path().join(".").join("second.json"),
    ] {
        let run = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .current_dir(root())
            .env("HOME", home.path())
            .env("KUBECONFIG", &list)
            .args(["namespace-lane", "observe", "--admin-kubeconfig"])
            .arg(&given)
            .output()
            .expect("xtask runs");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{}: {stderr}", given.display());
        assert!(
            stderr.contains("KUBECONFIG"),
            "{}: {stderr}",
            given.display()
        );
        assert!(!stderr.contains("41234"), "{}: {stderr}", given.display());
    }
}

#[test]
fn a_given_kubeconfig_under_home_kube_is_refused_before_it_is_read() {
    let home = tempfile::tempdir().expect("home");
    let kube = home.path().join(".kube");
    fs::create_dir(&kube).expect(".kube");
    let given = kube.join("config");
    fs::write(&given, kubeconfig("https://127.0.0.1:41234")).expect("given");
    let ambient = home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235")).expect("ambient");
    let run = observe(home.path(), &ambient, &given);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stderr}");
    assert!(stderr.contains("~/.kube"), "{stderr}");
    assert!(!stderr.contains("41235"), "{stderr}");
}
