//! Security review of design 34 U5 (`cargo xtask namespace-lane observe --admin-kubeconfig`).
//!
//! Each case drives the shipped binary with a stand-in `docker` and `kind` that record their argv
//! and answer only `network inspect` with a gateway, so no container, cluster or API is reached:
//! the given kubeconfigs carry CA data that is not a certificate, so a run the lane does not
//! refuse stops when the client is built, before any connection.
//!
//! Two cases are integrity checks of the rule that the lane acts only on its own kind cluster:
//! a loopback server reached through a `tls-server-name` (a local tunnel or proxy to another
//! cluster, which kind never writes) and the file `KUBECONFIG` names, the other location of the
//! default kubeconfig beside `~/.kube`. The others pin the path and server forms the existing
//! cases do not measure, and hold the canary credential out of every output.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CANARY_TOKEN: &str = "canary-token-u5-review-0123456789";
const CANARY_KEY: &str = "Y2FuYXJ5LWtleS11NS1yZXZpZXc=";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn kubeconfig(server: &str, extra: Option<(&str, &str)>) -> String {
    let mut cluster = serde_json::json!({"server": server, "certificate-authority-data": "Q0E="});
    if let Some((member, value)) = extra {
        cluster[member] = serde_json::json!(value);
    }
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "lane",
        "clusters": [{"name": "lane", "cluster": cluster}],
        "users": [{"name": "lane", "user": {"token": CANARY_TOKEN, "client-key-data": CANARY_KEY}}],
        "contexts": [{"name": "lane", "context": {"cluster": "lane", "user": "lane", "namespace": "lane"}}],
    })
    .to_string()
}

struct Stand {
    home: tempfile::TempDir,
    tools: tempfile::TempDir,
}

impl Stand {
    fn new() -> Self {
        let stand = Self {
            home: tempfile::tempdir().expect("home"),
            tools: tempfile::tempdir().expect("tools"),
        };
        let log = stand.log();
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\nif [ \"$1\" = network ]; then printf '%s\\n' '[{{\"Gateway\":\"172.18.0.1\"}}]'; exit 0; fi\nexit 1\n",
            log.display()
        );
        for name in ["docker", "kind"] {
            let path = stand.tools.path().join(name);
            fs::write(&path, &script).expect("stand-in");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
        }
        stand
    }

    fn log(&self) -> PathBuf {
        self.tools.path().join("argv.log")
    }

    fn tools_ran(&self) -> bool {
        self.log().exists()
    }

    fn observe(&self, cwd: &Path, ambient: &Path, given: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("KUBECONFIG", ambient)
            .args(["namespace-lane", "observe", "--docker"])
            .arg(self.tools.path().join("docker"))
            .arg("--kind")
            .arg(self.tools.path().join("kind"))
            .arg("--admin-kubeconfig")
            .arg(given)
            .output()
            .expect("xtask runs")
    }
}

fn texts(run: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    )
}

fn assert_no_secret_and_no_record(run: &Output) {
    let (stdout, stderr) = texts(run);
    for text in [&stdout, &stderr] {
        assert!(!text.contains(CANARY_TOKEN), "token printed: {text}");
        assert!(!text.contains(CANARY_KEY), "client key printed: {text}");
        assert!(!text.contains("Namespace-lane:"), "record printed: {text}");
    }
}

#[test]
fn a_loopback_server_reached_through_a_tls_server_name_is_refused() {
    let stand = Stand::new();
    let ambient = stand.home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235", None)).expect("ambient");
    let given = stand.home.path().join("tunnel.json");
    fs::write(
        &given,
        kubeconfig(
            "https://127.0.0.1:41234",
            Some(("tls-server-name", "api.cluster.example.test")),
        ),
    )
    .expect("given");
    let run = stand.observe(&root(), &ambient, &given);
    let (_, stderr) = texts(&run);
    assert!(!run.status.success(), "{stderr}");
    assert_no_secret_and_no_record(&run);
    assert!(
        stderr.contains("tls-server-name"),
        "a loopback server with a tls-server-name is a tunnel to another cluster, never kind's \
         own endpoint, and must be refused by name; the lane went on instead: {stderr}"
    );
}

#[test]
fn the_file_the_ambient_kubeconfig_names_is_refused_when_given() {
    let stand = Stand::new();
    let default = stand.home.path().join("config").join("default.json");
    fs::create_dir_all(default.parent().expect("parent")).expect("dir");
    fs::write(&default, kubeconfig("https://127.0.0.1:41234", None)).expect("default");
    let run = stand.observe(&root(), &default, &default);
    let (_, stderr) = texts(&run);
    assert!(!run.status.success(), "{stderr}");
    assert_no_secret_and_no_record(&run);
    assert!(
        stderr.contains("KUBECONFIG"),
        "the file KUBECONFIG names is the default kubeconfig, as ~/.kube is, and must be refused \
         before it is read; the lane read it: {stderr}"
    );
    assert!(
        !stand.tools_ran(),
        "a tool ran before the refusal: {:?}",
        fs::read_to_string(stand.log())
    );
}

#[test]
fn every_path_form_of_home_kube_is_refused_before_anything_runs() {
    let stand = Stand::new();
    let home = stand.home.path();
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let real = elsewhere.path().join("kube");
    fs::create_dir(&real).expect("real kube");
    fs::write(
        real.join("config"),
        kubeconfig("https://127.0.0.1:41234", None),
    )
    .expect("config");
    std::os::unix::fs::symlink(&real, home.join(".kube")).expect(".kube link");
    fs::create_dir(home.join("sub")).expect("sub");
    let outside_link = home.join("lane.json");
    std::os::unix::fs::symlink(home.join(".kube").join("config"), &outside_link).expect("link");
    let ambient = home.join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235", None)).expect("ambient");
    let cases: Vec<(PathBuf, PathBuf)> = vec![
        (root(), home.join(".kube").join("config")),
        (root(), real.join("config")),
        (root(), outside_link.clone()),
        (
            root(),
            home.join("sub").join("..").join(".kube").join("config"),
        ),
        (home.to_path_buf(), PathBuf::from(".kube/config")),
        (home.join("sub"), PathBuf::from("../.kube/./config")),
    ];
    for (cwd, given) in cases {
        let run = stand.observe(&cwd, &ambient, &given);
        let (_, stderr) = texts(&run);
        assert!(!run.status.success(), "{}: {stderr}", given.display());
        assert!(stderr.contains("~/.kube"), "{}: {stderr}", given.display());
        assert!(!stderr.contains("41235"), "{}: {stderr}", given.display());
        assert_no_secret_and_no_record(&run);
    }
    assert!(
        !stand.tools_ran(),
        "a tool ran: {:?}",
        fs::read_to_string(stand.log())
    );
}

#[test]
fn every_server_form_but_kinds_loopback_endpoint_is_refused_before_anything_runs() {
    let stand = Stand::new();
    let ambient = stand.home.path().join("ambient.json");
    fs::write(&ambient, kubeconfig("https://127.0.0.1:41235", None)).expect("ambient");
    for (index, server) in [
        "HTTPS://127.0.0.1:6443",
        "https://user@127.0.0.1:6443",
        "https://user:pass@127.0.0.1:6443",
        "https://localhost:6443",
        "https://LOCALHOST:6443",
        "https://127.0.0.2:6443",
        "https://[::1]:6443",
        "https://127.1:6443",
        "https://0x7f000001:6443",
        "https://127.0.0.1:6443/extra",
        "https://127.0.0.1:6443?x=1",
        "https://127.0.0.1:6443#x",
        " https://127.0.0.1:6443",
        "https://127.0.0.1:6443 ",
    ]
    .into_iter()
    .enumerate()
    {
        let given = stand.home.path().join(format!("given-{index}.json"));
        fs::write(&given, kubeconfig(server, None)).expect("given");
        let run = stand.observe(&root(), &ambient, &given);
        let (_, stderr) = texts(&run);
        assert!(!run.status.success(), "{server}: {stderr}");
        assert!(
            stderr.contains("https://127.0.0.1:<port>"),
            "{server}: {stderr}"
        );
        assert!(!stderr.contains("41235"), "{server}: {stderr}");
        assert_no_secret_and_no_record(&run);
    }
    assert!(
        !stand.tools_ran(),
        "a tool ran: {:?}",
        fs::read_to_string(stand.log())
    );
}
