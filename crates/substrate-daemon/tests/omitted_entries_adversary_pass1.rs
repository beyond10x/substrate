#![forbid(unsafe_code)]
//! Adversary pass 1 on ADR 0043: the `x-b10x-omitted-entries` header, driven over the router.
//!
//! Each case builds its fixture on the host side of a workspace created through the API, then
//! reads it only through `GET` routes, so the assertions are on the wire and not on host types.

use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use base64::Engine as _;
use serde_json::{Value, json};
use substrate_daemon::{App, Identity, router};
use substrate_host::{HostConfig, HostDriver};
use substrate_store::Store;
use tempfile::TempDir;
use tower::ServiceExt as _;

const HEADER: &str = "x-b10x-omitted-entries";

struct Harness {
    directory: TempDir,
    app: Arc<App>,
}

impl Harness {
    fn open() -> Self {
        let directory = tempfile::tempdir().expect("temporary harness");
        let store = Arc::new(Store::open(directory.path().join("state.db")).expect("state store"));
        let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
            .expect("host driver");
        let app = App::new(store, driver, "dep_omitted_adversary");
        Self { directory, app }
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        request_id: &str,
        body: Body,
    ) -> (StatusCode, HeaderMap, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-request-id", request_id)
            .body(body)
            .expect("request");
        let identity = Identity {
            subject: "local:1000".to_owned(),
            actor: "omitted-adversary".to_owned(),
            principal: None,
        };
        let response = router(Arc::clone(&self.app))
            .layer(Extension(identity))
            .oneshot(request)
            .await
            .expect("router response");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 2_097_152)
            .await
            .expect("response body");
        (
            status,
            headers,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }

    /// Creates an empty workspace and answers its id and its host directory.
    async fn workspace(&self, operation: &str) -> (String, PathBuf) {
        let body = Body::from(
            serde_json::to_vec(&json!({
                "op": operation,
                "input": { "source": "empty", "labels": {} }
            }))
            .expect("mutation JSON"),
        );
        let (status, _, created) = self
            .call(Method::POST, "/v1/workspaces", "req_adv_create", body)
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["result"]["id"]
            .as_str()
            .expect("workspace id")
            .to_owned();
        let host = self.directory.path().join("workspaces").join(&id);
        assert!(
            host.is_dir(),
            "workspace root on the host: {}",
            host.display()
        );
        (id, host)
    }
}

fn mkfifo(path: &std::path::Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

fn socket(path: &std::path::Path) {
    drop(std::os::unix::net::UnixListener::bind(path).expect("bind socket"));
}

fn non_utf8(directory: &std::path::Path, raw: &[u8]) -> PathBuf {
    directory.join(std::ffi::OsString::from_vec(raw.to_vec()))
}

/// The header exactly once, as a string; `None` when absent.
fn omitted(headers: &HeaderMap) -> Option<String> {
    let values: Vec<_> = headers.get_all(HEADER).iter().collect();
    assert!(
        values.len() <= 1,
        "the header is sent at most once: {values:?}"
    );
    values
        .first()
        .map(|value| value.to_str().expect("ASCII header").to_owned())
}

fn cursor(index: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(index.to_be_bytes())
}

/// Gap 1 over the wire: every page of one directory, walked by cursor at `limit_items=1`,
/// carries the directory's count, including the empty page at the end cursor. The count mixes a
/// FIFO, a socket, a non-UTF-8 file and a non-UTF-8 directory with entries beneath it.
#[tokio::test(flavor = "multi_thread")]
async fn v1_every_page_by_cursor_carries_the_directory_count_adversary_pass1() {
    let harness = Harness::open();
    let (workspace, host) = harness.workspace("01JADVOMITTEDPAGES00001").await;
    let listing = host.join("paged");
    std::fs::create_dir(&listing).expect("listing");
    for name in ["a.txt", "m.txt", "z.txt"] {
        std::fs::write(listing.join(name), b"x").expect("file");
    }
    mkfifo(&listing.join("b-fifo"));
    socket(&listing.join("c-socket"));
    std::fs::write(non_utf8(&listing, b"d\xff"), b"x").expect("non-UTF-8 file");
    let raw_directory = non_utf8(&listing, b"e\xfe");
    std::fs::create_dir(&raw_directory).expect("non-UTF-8 directory");
    mkfifo(&raw_directory.join("inner-fifo"));

    let mut next: Option<String> = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let uri = match &next {
            None => format!("/v1/workspaces/{workspace}/files/paged?mode=directory&limit_items=1"),
            Some(cursor) => format!(
                "/v1/workspaces/{workspace}/files/paged?mode=directory&limit_items=1&cursor={cursor}"
            ),
        };
        let (status, headers, page) = harness
            .call(Method::GET, &uri, "req_adv_page", Body::empty())
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(
            omitted(&headers).as_deref(),
            Some("4"),
            "page {pages}: the directory's count, not the page's: {page}"
        );
        for item in page["result"]["items"].as_array().expect("items") {
            seen.push(item["name"].as_str().expect("name").to_owned());
        }
        pages += 1;
        match page["result"]["next_cursor"].as_str() {
            Some(cursor) => next = Some(cursor.to_owned()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, ["a.txt", "m.txt", "z.txt"]);

    // The end cursor names an empty page; it is still a page of this directory.
    let (status, headers, page) = harness
        .call(
            Method::GET,
            &format!(
                "/v1/workspaces/{workspace}/files/paged?mode=directory&limit_items=1&cursor={}",
                cursor(3)
            ),
            "req_adv_page_end",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["result"]["items"], json!([]));
    assert_eq!(omitted(&headers).as_deref(), Some("4"));

    // A refused cursor carries no listing and no count.
    let (status, headers, page) = harness
        .call(
            Method::GET,
            &format!(
                "/v1/workspaces/{workspace}/files/paged?mode=directory&limit_items=1&cursor={}",
                cursor(4)
            ),
            "req_adv_page_past",
            Body::empty(),
        )
        .await;
    assert_ne!(status, StatusCode::OK, "{page}");
    assert_eq!(omitted(&headers), None, "a refusal names no omission");
}

/// A directory whose every entry is omitted: the v1 page is empty with no cursor and names them;
/// the tree lists only the directory, is truncated because it omitted them (ADR 0043, as amended),
/// and names them.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_of_only_omitted_entries_is_named_on_both_routes_adversary_pass1() {
    let harness = Harness::open();
    let (workspace, host) = harness.workspace("01JADVOMITTEDONLY000001").await;
    let only = host.join("only");
    std::fs::create_dir(&only).expect("directory");
    mkfifo(&only.join("fifo"));
    socket(&only.join("socket"));

    let (status, headers, page) = harness
        .call(
            Method::GET,
            &format!("/v1/workspaces/{workspace}/files/only?mode=directory&limit_items=10"),
            "req_adv_only_page",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["result"]["items"], json!([]));
    assert_eq!(page["result"]["next_cursor"], Value::Null);
    assert_eq!(omitted(&headers).as_deref(), Some("2"));

    let (status, headers, tree) = harness
        .call(
            Method::GET,
            &format!("/v2/workspaces/{workspace}/tree?limit_items=10&include_hidden=false"),
            "req_adv_only_tree",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tree}");
    assert_eq!(
        tree["result"]["items"],
        json!([{ "path": "only", "kind": "directory", "size": null }])
    );
    assert_eq!(tree["result"]["truncated"], true, "{tree}");
    assert_eq!(omitted(&headers).as_deref(), Some("2"));
}

/// A symbolic link is representable whatever it points at: a link to a FIFO and a dangling link
/// are listed as symlinks and not counted; only the FIFO itself is.
#[tokio::test(flavor = "multi_thread")]
async fn a_symlink_to_a_fifo_is_listed_and_not_counted_adversary_pass1() {
    let harness = Harness::open();
    let (workspace, host) = harness.workspace("01JADVOMITTEDLINKS00001").await;
    let links = host.join("links");
    std::fs::create_dir(&links).expect("directory");
    mkfifo(&links.join("fifo"));
    std::os::unix::fs::symlink("fifo", links.join("to-fifo")).expect("link to FIFO");
    std::os::unix::fs::symlink("absent", links.join("dangling")).expect("dangling link");

    let (status, headers, page) = harness
        .call(
            Method::GET,
            &format!("/v1/workspaces/{workspace}/files/links?mode=directory&limit_items=10"),
            "req_adv_links_page",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let names: Vec<(&str, &str)> = page["result"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["name"].as_str().expect("name"),
                item["kind"].as_str().expect("kind"),
            )
        })
        .collect();
    assert_eq!(names, [("dangling", "symlink"), ("to-fifo", "symlink")]);
    assert_eq!(omitted(&headers).as_deref(), Some("1"));

    let (status, headers, tree) = harness
        .call(
            Method::GET,
            &format!("/v2/workspaces/{workspace}/tree?limit_items=10&include_hidden=false"),
            "req_adv_links_tree",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tree}");
    assert_eq!(tree["result"]["items"].as_array().map(Vec::len), Some(3));
    assert_eq!(omitted(&headers).as_deref(), Some("1"));
}

/// Gap 2, both sides of decision 4. Limit reached with an unwalked FIFO left: `truncated` and a
/// lower bound (here absent, because the walked part omitted nothing). Limit met exactly with only
/// omitted entries left: the count is exact, and `truncated` is still true because an omission sets
/// it (ADR 0043, as amended); the header is what tells the two causes apart.
#[tokio::test(flavor = "multi_thread")]
async fn the_tree_count_is_exact_unless_the_limit_is_reached_adversary_pass1() {
    let harness = Harness::open();
    let (workspace, host) = harness.workspace("01JADVOMITTEDBOUND00001").await;
    std::fs::create_dir(host.join("a")).expect("a");
    std::fs::write(host.join("a/one.txt"), b"1").expect("file");
    mkfifo(&host.join("a/two-fifo"));
    std::fs::create_dir(host.join("z")).expect("z");
    mkfifo(&host.join("z/fifo"));
    socket(&host.join("z/socket"));

    let tree = |limit: u32| {
        format!("/v2/workspaces/{workspace}/tree?limit_items={limit}&include_hidden=false")
    };
    // Walk: a, a/one.txt, (a/two-fifo omitted), z, (z/fifo, z/socket omitted).
    let expectations: [(u32, bool, Option<&str>, usize); 4] = [
        (1, true, None, 1),
        (2, true, Some("1"), 2),
        (3, true, Some("3"), 3),
        (100, true, Some("3"), 3),
    ];
    for (limit, truncated, count, items) in expectations {
        let (status, headers, body) = harness
            .call(Method::GET, &tree(limit), "req_adv_bound", Body::empty())
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["result"]["items"].as_array().map(Vec::len),
            Some(items),
            "limit {limit}: {body}"
        );
        assert_eq!(
            body["result"]["truncated"], truncated,
            "limit {limit}: {body}"
        );
        assert_eq!(omitted(&headers).as_deref(), count, "limit {limit}");
    }
}

/// Decision 2's exclusions over the wire: a `.git` FIFO is never counted; a hidden FIFO counts on
/// the v1 page, counts on the tree with `include_hidden=true`, and not with `false`.
#[tokio::test(flavor = "multi_thread")]
async fn git_and_hidden_exclusions_hold_on_the_wire_adversary_pass1() {
    let harness = Harness::open();
    let (workspace, host) = harness.workspace("01JADVOMITTEDHIDDEN0001").await;
    let directory = host.join("d");
    std::fs::create_dir(&directory).expect("d");
    std::fs::write(directory.join("f.txt"), b"f").expect("file");
    mkfifo(&directory.join(".git"));
    mkfifo(&directory.join(".hidden-fifo"));

    let (status, headers, page) = harness
        .call(
            Method::GET,
            &format!("/v1/workspaces/{workspace}/files/d?mode=directory&limit_items=10"),
            "req_adv_hidden_page",
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        omitted(&headers).as_deref(),
        Some("1"),
        "the hidden FIFO, not .git"
    );

    for (include_hidden, truncated, count) in [(false, false, None), (true, true, Some("1"))] {
        let (status, headers, tree) = harness
            .call(
                Method::GET,
                &format!(
                    "/v2/workspaces/{workspace}/tree?limit_items=10&include_hidden={include_hidden}"
                ),
                "req_adv_hidden_tree",
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{tree}");
        assert_eq!(
            tree["result"]["truncated"], truncated,
            "include_hidden={include_hidden}"
        );
        assert_eq!(
            omitted(&headers).as_deref(),
            count,
            "include_hidden={include_hidden}"
        );
    }
}
