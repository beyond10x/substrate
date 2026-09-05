//! A real TLS/upload-pack peer exercises the production gix path, including HTTP reuse.

#[path = "quota_tests.rs"]
mod quota_tests;

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use git2::{Repository, Signature};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use substrate_wire::{GitSource, StorageLimit};
use tempfile::{TempDir, tempdir};
use zeroize::Zeroizing;

use super::materialize;
use crate::GitSourceBinding;

const AUTHORITY: &str = "transient-fixture-authority";
const LIMIT: StorageLimit = StorageLimit {
    max_bytes: 8 * 1024 * 1024,
    max_inodes: 2000,
};

#[derive(Clone, Copy)]
enum Mode {
    V2,
    Legacy,
    Redirect,
    TruncatedPack,
    DuplicateRef,
}

struct Request {
    path: String,
    headers: Vec<String>,
    body: Vec<u8>,
    response_bytes: usize,
}

struct Server {
    locator: url::Url,
    binding: GitSourceBinding,
    requests: Arc<Mutex<Vec<Request>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    _certificates: TempDir,
}

impl Server {
    fn start(repository: &Path, mode: Mode) -> Self {
        Self::start_observed(repository, mode, None)
    }

    fn start_observed(
        repository: &Path,
        mode: Mode,
        on_request: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let certificates = tempdir().expect("certificate directory");
        let identity = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()])
            .expect("TLS fixture identity");
        let ca_path = certificates.path().join("ca.pem");
        std::fs::write(&ca_path, identity.cert.pem()).expect("fixture trust root");
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![identity.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der())
                    .into(),
            )
            .expect("TLS server config");
        let listener = TcpListener::bind("127.0.0.1:0").expect("Git listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let origin = format!(
            "https://{}/",
            listener.local_addr().expect("listener address")
        );
        let locator = url::Url::parse(&format!("{origin}repository.git")).expect("Git URL");
        let binding =
            GitSourceBinding::new("connector", &origin, Some(ca_path)).expect("source binding");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::clone(&connections);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let repository = repository.to_path_buf();
        let worker = thread::spawn(move || {
            let config = Arc::new(config);
            while !stopped.load(Ordering::Relaxed) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                accepted.fetch_add(1, Ordering::Relaxed);
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("read timeout");
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .expect("write timeout");
                let tls = ServerConnection::new(Arc::clone(&config)).expect("server session");
                let mut stream = BufReader::new(StreamOwned::new(tls, socket));
                while !stopped.load(Ordering::Relaxed) {
                    if serve_request(
                        &mut stream,
                        &repository,
                        mode,
                        &observed,
                        on_request.as_deref(),
                    )
                    .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            locator,
            binding,
            requests,
            connections,
            stop,
            thread: Some(worker),
            _certificates: certificates,
        }
    }

    fn source(&self, commit: git2::Oid) -> GitSource {
        GitSource {
            source: "connector".to_owned(),
            locator: self.locator.to_string(),
            reference: "provider-default".to_owned(),
            commit: commit.to_string(),
            depth: 50,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .take()
            .expect("server thread")
            .join()
            .expect("server exited");
    }
}

fn serve_request(
    stream: &mut BufReader<StreamOwned<ServerConnection, TcpStream>>,
    repository: &Path,
    mode: Mode,
    observed: &Mutex<Vec<Request>>,
    on_request: Option<&(dyn Fn() + Send + Sync)>,
) -> std::io::Result<()> {
    let mut line = String::new();
    if stream.read_line(&mut line)? == 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    }
    let parts: Vec<_> = line.split_ascii_whitespace().collect();
    let method = parts[0].to_owned();
    let path = parts[1].to_owned();
    let mut headers = Vec::new();
    loop {
        line.clear();
        stream.read_line(&mut line)?;
        if line == "\r\n" {
            break;
        }
        headers.push(line.trim_end().to_owned());
    }
    let length = headers
        .iter()
        .find_map(|header| {
            let (name, value) = header.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("body length"))
        })
        .unwrap_or(0);
    assert!(length < 64 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    if let Some(on_request) = on_request {
        on_request();
    }
    let discovery = method == "GET";
    let mut response = upload_pack(repository, discovery, !matches!(mode, Mode::Legacy), &body);
    if matches!(mode, Mode::DuplicateRef)
        && body
            .windows(b"command=ls-refs".len())
            .any(|part| part == b"command=ls-refs")
    {
        let repeated = response[..response.len() - 4].to_vec();
        response.splice(0..0, repeated);
    }
    if matches!(mode, Mode::TruncatedPack)
        && body
            .windows(b"command=fetch".len())
            .any(|part| part == b"command=fetch")
    {
        response.truncate(response.len() / 2);
    }
    observed.lock().expect("request records").push(Request {
        path,
        headers,
        body,
        response_bytes: response.len(),
    });
    if matches!(mode, Mode::Redirect) {
        write!(
            stream.get_mut(),
            "HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:1/refused\r\nContent-Length: 0\r\n\r\n"
        )?;
    } else {
        let content = if discovery { "advertisement" } else { "result" };
        write!(
            stream.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-{content}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            response.len()
        )?;
        stream.get_mut().write_all(&response)?;
    }
    stream.get_mut().flush()
}

fn upload_pack(repository: &Path, discovery: bool, v2: bool, body: &[u8]) -> Vec<u8> {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    if v2 {
        command.env("GIT_PROTOCOL", "version=2");
    }
    command.args(["upload-pack", "--stateless-rpc"]);
    if discovery {
        command.arg("--advertise-refs");
    }
    let mut child = command
        .arg(repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Git upload-pack installed");
    child
        .stdin
        .take()
        .expect("upload-pack stdin")
        .write_all(body)
        .expect("upload-pack input");
    let output = child.wait_with_output().expect("upload-pack output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn repository(extra_refs: usize) -> (TempDir, git2::Oid) {
    let directory = tempdir().expect("Git source directory");
    let repository = Repository::init(directory.path()).expect("repository");
    let blob = repository.blob(b"bounded v2 workspace\n").expect("blob");
    let mut builder = repository.treebuilder(None).expect("tree builder");
    builder
        .insert("README.md", blob, 0o100_644)
        .expect("tree entry");
    let tree = repository
        .find_tree(builder.write().expect("tree write"))
        .expect("tree");
    let signature =
        Signature::now("Substrate Test", "substrate@example.invalid").expect("signature");
    let mut parent = None;
    for number in 0..65 {
        let parents: Vec<_> = parent.iter().collect();
        let oid = repository
            .commit(
                Some("refs/heads/provider-default"),
                &signature,
                &signature,
                &format!("fixture {number}"),
                &tree,
                &parents,
            )
            .expect("commit");
        parent = Some(repository.find_commit(oid).expect("parent"));
    }
    let commit = parent.as_ref().expect("tip").id();
    repository
        .set_head("refs/heads/provider-default")
        .expect("provider HEAD");
    repository
        .tag_lightweight(
            "fixture-tag",
            parent.as_ref().expect("tip").as_object(),
            false,
        )
        .expect("tag");
    let mut refs = String::from("# pack-refs with: peeled fully-peeled sorted\n");
    for number in 0..extra_refs {
        writeln!(refs, "{commit} refs/heads/other-{number:06}").expect("reference line");
    }
    std::fs::write(repository.path().join("packed-refs"), refs).expect("many refs");
    (directory, commit)
}

#[test]
fn v2_materializes_fifty_commits_over_one_tls_connection_and_restricts_refs() {
    let (source, commit) = repository(10_000);
    let server = Server::start(source.path(), Mode::V2);
    let target = tempdir().expect("target");
    let started = Instant::now();
    let usage = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect("v2 materialization");
    let elapsed = started.elapsed();
    let repository = Repository::open(target.path()).expect("materialized repository");
    assert_eq!(repository.head().expect("HEAD").target(), Some(commit));
    assert!(repository.head_detached().expect("detached HEAD"));
    assert!(repository.is_shallow());
    let mut history = repository.revwalk().expect("history");
    history.push_head().expect("history start");
    assert_eq!(
        history
            .collect::<Result<Vec<_>, _>>()
            .expect("usable shallow history")
            .len(),
        50
    );
    assert_eq!(
        std::fs::read(target.path().join("README.md")).expect("editable file"),
        b"bounded v2 workspace\n"
    );
    assert!(repository.tag_names(None).expect("tags").is_empty());
    let local_config =
        std::fs::read_to_string(repository.path().join("config")).expect("local config");
    assert!(!local_config.contains(AUTHORITY));
    assert!(!local_config.contains("extraHeader"));
    assert!(usage.used_bytes > 0);
    assert_eq!(
        server.connections.load(Ordering::Relaxed),
        1,
        "discovery, refs and fetch share TLS"
    );
    let requests = server.requests.lock().expect("request records");
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert!(
            request
                .headers
                .iter()
                .any(|header| header.eq_ignore_ascii_case("Git-Protocol: version=2"))
        );
        assert!(
            request
                .headers
                .iter()
                .any(|header| header == &format!("X-B10X-Git-Source-Authorization: {AUTHORITY}"))
        );
    }
    let refs = String::from_utf8_lossy(&requests[1].body);
    let fetch = String::from_utf8_lossy(&requests[2].body);
    assert!(refs.contains("command=ls-refs"));
    assert!(
        refs.contains("ref-prefix refs/heads/provider-default"),
        "{refs}"
    );
    assert!(fetch.contains("command=fetch"));
    assert!(fetch.contains("deepen 50"));
    assert!(fetch.contains(&format!("want {commit}")));
    assert!(!fetch.contains("include-tag"));
    assert!(!fetch.contains("have "));
    assert!(!fetch.contains("filter "));
    assert!(
        requests[0]
            .path
            .ends_with("/info/refs?service=git-upload-pack")
    );
    let legacy = upload_pack(source.path(), true, false, &[]);
    let v2_discovery = requests[0].response_bytes + requests[1].response_bytes;
    assert!(
        v2_discovery * 100 < legacy.len(),
        "v2 discovery should avoid advertising 10,000 unrelated refs"
    );
    eprintln!(
        "Git fixture: refs=10000 legacy_discovery_bytes={} v2_discovery_bytes={v2_discovery} materialize_ms={} installed_bytes={} installed_inodes={} tls_connections=1",
        legacy.len(),
        elapsed.as_millis(),
        usage.used_bytes,
        usage.used_inodes
    );
    eprintln!("v2 ls-refs={refs} v2 fetch={fetch}");
}

#[test]
fn legacy_handshake_is_refused_before_ref_listing_or_pack_request() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::Legacy);
    let target = tempdir().expect("target");
    let error = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect_err("legacy refusal");
    assert_eq!(error.code, "workspace.git-protocol-refused");
    assert_eq!(server.requests.lock().expect("requests").len(), 1);
}

#[test]
fn moved_commit_is_refused_before_pack_request() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let target = tempdir().expect("target");
    let mut requested = server.source(commit);
    requested.commit = "0123456789012345678901234567890123456789".to_owned();
    let error = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &requested,
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect_err("moved commit refusal");
    assert_eq!(error.code, "workspace.git-commit-moved");
    assert_eq!(server.requests.lock().expect("requests").len(), 2);
}

#[test]
fn duplicate_branch_advertisements_are_refused_before_pack_request() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::DuplicateRef);
    let target = tempdir().expect("target");
    let error = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect_err("ambiguous branch");
    assert_eq!(error.code, "workspace.git-commit-moved");
    assert_eq!(server.requests.lock().expect("requests").len(), 2);
}

#[test]
fn transfer_limit_aborts_real_v2_stream_without_disclosing_authority() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let target = tempdir().expect("target");
    let error = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        StorageLimit {
            max_bytes: 1200,
            ..LIMIT
        },
        &Arc::new(AtomicBool::new(false)),
    )
    .expect_err("bounded stream");
    assert_eq!(error.code, "workspace.git-transfer-limit");
    assert!(!error.to_string().contains(AUTHORITY));
}

#[test]
fn redirects_and_untrusted_tls_are_refused() {
    let (source, commit) = repository(0);
    for mode in [Mode::Redirect, Mode::V2] {
        let server = Server::start(source.path(), mode);
        let mut binding = server.binding.clone();
        if matches!(mode, Mode::V2) {
            binding.ca_bundle = None;
        }
        let target = tempdir().expect("target");
        let error = materialize(
            target.path(),
            &server.locator,
            &binding,
            &server.source(commit),
            &Zeroizing::new(AUTHORITY.to_owned()),
            LIMIT,
            &Arc::new(AtomicBool::new(false)),
        )
        .expect_err("trust/redirect refusal");
        assert_eq!(error.code, "workspace.git-fetch-failed");
        assert!(server.requests.lock().expect("requests").len() <= 1);
    }
}

#[test]
fn cancellation_prevents_network_dispatch() {
    let target = tempdir().expect("target");
    let binding =
        GitSourceBinding::new("connector", "https://127.0.0.1:1/", None).expect("binding");
    let locator = url::Url::parse("https://127.0.0.1:1/repository.git").expect("locator");
    let source = GitSource {
        source: "connector".to_owned(),
        locator: locator.to_string(),
        reference: "provider-default".to_owned(),
        commit: "0123456789012345678901234567890123456789".to_owned(),
        depth: 50,
    };
    let error = materialize(
        target.path(),
        &locator,
        &binding,
        &source,
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(true)),
    )
    .expect_err("cancelled");
    assert_eq!(error.code, "workspace.git-fetch-cancelled");
}

#[test]
fn truncated_pack_never_reaches_checkout() {
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::TruncatedPack);
    let target = tempdir().expect("target");
    let error = materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect_err("incomplete pack");
    assert_eq!(error.code, "workspace.git-fetch-failed");
    assert!(!target.path().join("README.md").exists());
    assert!(
        Repository::open(target.path())
            .expect("staging repository")
            .head()
            .is_err()
    );
}

#[test]
fn ambient_git_configuration_cannot_change_fetch_authority_or_transport() {
    const CHILD: &str = "SUBSTRATE_GIT_ISOLATION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let environment = tempdir().expect("ambient fixture");
        let template = environment.path().join("template");
        std::fs::create_dir(&template).expect("template");
        std::fs::write(
            template.join("ambient-template-marker"),
            b"must not be copied",
        )
        .expect("template marker");
        let config = environment.path().join("config");
        std::fs::write(&config, format!("[http]\nproxy = http://127.0.0.1:1\nextraHeader = X-Ambient-Must-Not-Escape: marker\nsslVerify = false\nfollowRedirects = true\n[url \"https://127.0.0.1:1/\"]\ninsteadOf = https://\n[init]\ntemplateDir = {}\n[credential]\nhelper = /nonexistent/git-fixture-helper\n", template.display())).expect("ambient configuration");
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["git::materialization_tests::ambient_git_configuration_cannot_change_fetch_authority_or_transport", "--exact", "--nocapture"])
            .env(CHILD, "1")
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_SYSTEM", &config)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.proxy")
            .env("GIT_CONFIG_VALUE_0", "http://127.0.0.1:1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("GIT_SSL_NO_VERIFY", "true")
            .output().expect("isolated child");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let (source, commit) = repository(0);
    let server = Server::start(source.path(), Mode::V2);
    let target = tempdir().expect("target");
    materialize(
        target.path(),
        &server.locator,
        &server.binding,
        &server.source(commit),
        &Zeroizing::new(AUTHORITY.to_owned()),
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect("isolated fetch");
    assert!(!target.path().join(".git/ambient-template-marker").exists());
    let requests = server.requests.lock().expect("requests");
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| {
        request
            .headers
            .iter()
            .all(|header| !header.contains("X-Ambient-Must-Not-Escape"))
    }));
}

#[test]
#[ignore = "requires an external test-only Connectors TLS proxy fixture"]
fn external_connectors_proxy_v2_fixture() {
    fn input(name: &str) -> String {
        std::env::var(name).expect("external Git fixture input")
    }
    let locator = url::Url::parse(&input("SUBSTRATE_TEST_GIT_LOCATOR")).expect("fixture locator");
    let mut base = locator.clone();
    base.set_path("/");
    let binding = GitSourceBinding::new(
        "connector",
        base.as_str(),
        Some(input("SUBSTRATE_TEST_GIT_CA").into()),
    )
    .expect("fixture binding");
    let source = GitSource {
        source: "connector".to_owned(),
        locator: locator.to_string(),
        reference: input("SUBSTRATE_TEST_GIT_REF"),
        commit: input("SUBSTRATE_TEST_GIT_COMMIT"),
        depth: 50,
    };
    let authority = Zeroizing::new(input("SUBSTRATE_TEST_GIT_AUTHORITY"));
    let target = tempdir().expect("fixture target");
    materialize(
        target.path(),
        &locator,
        &binding,
        &source,
        &authority,
        LIMIT,
        &Arc::new(AtomicBool::new(false)),
    )
    .expect("gix materializes through real Connectors proxy");
    let repository = Repository::open(target.path()).expect("materialized repository");
    assert_eq!(
        repository
            .head()
            .expect("HEAD")
            .target()
            .expect("commit")
            .to_string(),
        source.commit
    );
    let mut history = repository.revwalk().expect("history");
    history.push_head().expect("history start");
    assert_eq!(
        history
            .collect::<Result<Vec<_>, _>>()
            .expect("usable shallow history")
            .len(),
        50
    );
    assert!(repository.tag_names(None).expect("tags").is_empty());
    assert!(
        !std::fs::read_to_string(repository.path().join("config"))
            .expect("local config")
            .contains(authority.as_str())
    );
}
