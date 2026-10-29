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
