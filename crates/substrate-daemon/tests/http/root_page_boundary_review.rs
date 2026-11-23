//! Independent verification of ADR 0052 through the daemon's router
//! (review of story:root-directory-listing-pages-immediate-children).
//!
//! The rule: the workspace's root is the path `.` on the v1 directory page, admitted as the
//! whole path of a well-formed directory query and nowhere else. Every other spelling, every
//! other route and every malformed query refuses by name, and the root page answers what a
//! named directory's page answers for the same children, the same subject and the same cursor.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Harness, mutation, sha256_hex};

const OWNER: &str = "local:1000";
const OMITTED: &str = "x-b10x-omitted-entries";

impl Harness {
    /// Creates an empty workspace for `subject` and answers its id and its host directory.
    async fn review_workspace(&self, subject: &str, operation: &str) -> (String, PathBuf) {
        let (status, created) = self
            .call(
                subject,
                Method::POST,
                "/v1/workspaces",
                "req_root_review_create",
                mutation(operation, json!({ "source": "empty", "labels": {} })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["result"]["id"]
            .as_str()
            .expect("workspace id")
            .to_owned();
        let host = self.directory.path().join("workspaces").join(&id);
        assert!(host.is_dir(), "the workspace has a host directory");
        (id, host)
    }

    async fn page(&self, subject: &str, workspace: &str, path: &str, query: &str) -> Answer {
        let (status, headers, body) = self
            .call_with_headers(
                subject,
                Method::GET,
                &format!("/v1/workspaces/{workspace}/files/{path}{query}"),
                "req_root_review_page",
                Body::empty(),
            )
            .await;
        Answer {
            status,
            omitted: headers
                .get(OMITTED)
                .map(|value| value.to_str().expect("ASCII header").to_owned()),
            body,
        }
    }
}

struct Answer {
    status: StatusCode,
    omitted: Option<String>,
    body: Value,
}

impl Answer {
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or("<no error>")
    }

    /// `(name, kind, size)` of every listed item, in the order answered.
    fn items(&self) -> Vec<(String, String, Value)> {
        self.body["result"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("a directory page: {}", self.body))
            .iter()
            .map(|item| {
                (
                    item["name"].as_str().expect("name").to_owned(),
                    item["kind"].as_str().expect("kind").to_owned(),
                    item["size"].clone(),
                )
            })
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.items().into_iter().map(|(name, _, _)| name).collect()
    }
}

/// Every name directly beneath `directory` on the host, sorted.
fn host_names(directory: &Path) -> BTreeSet<String> {
    std::fs::read_dir(directory)
        .expect("host directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("UTF-8 name")
        })
        .collect()
}

fn mkfifo(path: &Path) {
    nix::unistd::mkfifo(path, nix::sys::stat::Mode::from_bits_truncate(0o600))
        .unwrap_or_else(|error| panic!("mkfifo: {error}"));
}

/// The same children in `directory`: three files, a directory with a file beneath it, Git control
/// data, a FIFO the wire cannot name, a temporary a served write would leave, and a link out.
fn populate(directory: &Path, outside: &Path) {
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(directory.join(name), name.as_bytes()).expect("file");
    }
    std::fs::create_dir(directory.join("d")).expect("directory");
    std::fs::write(directory.join("d/inner.txt"), b"inner").expect("file beneath it");
    std::fs::create_dir(directory.join(".git")).expect("git directory");
    std::fs::write(directory.join(".git/config"), b"control data").expect("git config");
    std::fs::write(directory.join(".substrate-left.tmp"), b"t").expect("temporary");
    mkfifo(&directory.join("a-fifo"));
    std::os::unix::fs::symlink(outside, directory.join("z-link")).expect("link out");
}

const DIRECTORY_QUERY: &str = "?mode=directory&limit_items=10";

/// ADR 0052 decision 2: `.` is the whole path or the path is refused. A spelling a client or a
/// proxy could send in place of the bare dot does not reach a listing.
#[tokio::test(flavor = "multi_thread")]
async fn the_bare_dot_is_the_only_spelling_that_reaches_the_root_page() {
    let harness = Harness::open();
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("beyond.txt"), b"outside").expect("outside file");
    let (workspace, host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWSPELLING001")
        .await;
    populate(&host, outside.path());

    let root = harness.page(OWNER, &workspace, ".", DIRECTORY_QUERY).await;
    assert_eq!(root.status, StatusCode::OK, "{}", root.body);
    assert_eq!(root.body["result"]["path"], ".");
    assert_eq!(
        root.names(),
        [
            ".substrate-left.tmp",
            "a.txt",
            "b.txt",
            "c.txt",
            "d",
            "z-link"
        ]
    );

    // The percent-encoded dot is the same one segment once the route decodes it; it is the bare
    // dot and answers the same page.
    for encoded in ["%2E", "%2e"] {
        let page = harness
            .page(OWNER, &workspace, encoded, DIRECTORY_QUERY)
            .await;
        assert_eq!(page.status, StatusCode::OK, "{encoded}: {}", page.body);
        assert_eq!(page.body["result"]["path"], ".", "{encoded}");
        assert_eq!(page.names(), root.names(), "{encoded}");
    }

    // Every longer spelling is refused by name before anything is opened.
    for spelling in [
        "./",
        ".//",
        "%2e%2f",
        "%2E%2F",
        ".%2F",
        "./a.txt",
        "./d",
        "%2e/d",
        "d/.",
        "d/%2e",
        "d/./inner.txt",
        "./.",
        "..",
        "%2e%2e",
        "./..",
        "d/..",
        ".%2F..",
        ".%00",
        "%00.",
        ".%5C",
    ] {
        let answer = harness
            .page(OWNER, &workspace, spelling, DIRECTORY_QUERY)
            .await;
        assert_eq!(
            (answer.status, answer.code()),
            (StatusCode::UNPROCESSABLE_ENTITY, "workspace.path-escape"),
            "{spelling:?} must be refused by name: {}",
            answer.body
        );
        assert!(
            answer.body["result"].is_null(),
            "{spelling:?} lists nothing"
        );
        assert_eq!(answer.omitted, None, "{spelling:?} names no omission");
    }

    // A doubly encoded dot and a dot with a space are ordinary names that are not there.
    for name in ["%252e", ".%20", "%20."] {
        let answer = harness.page(OWNER, &workspace, name, DIRECTORY_QUERY).await;
        assert_eq!(
            (answer.status, answer.code()),
            (StatusCode::NOT_FOUND, "resource.not-found"),
            "{name:?}: {}",
            answer.body
        );
    }

    // The empty path stays unrouted, with and without the trailing slash.
    for uri in [
        format!("/v1/workspaces/{workspace}/files/{DIRECTORY_QUERY}"),
        format!("/v1/workspaces/{workspace}/files{DIRECTORY_QUERY}"),
    ] {
        let (status, body) = harness
            .call(
                OWNER,
                Method::GET,
                &uri,
                "req_root_review_empty",
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "resource.not-found", "{uri}");
        assert!(body["result"].is_null(), "{uri} lists nothing");
    }
}

/// ADR 0052 decision 2: only a well-formed directory query makes `.` an address. With any other
/// query on the v1 read route the path is refused first, by the name it always had.
#[tokio::test(flavor = "multi_thread")]
async fn dot_with_any_query_but_a_well_formed_directory_page_is_a_path_refusal() {
    let harness = Harness::open();
    let (workspace, host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWQUERYSHAPE1")
        .await;
    std::fs::write(host.join("a.txt"), b"a").expect("file");

    for query in [
        "",
        "?",
        "?mode=file&offset=0&limit_bytes=16",
        "?mode=directory",
        "?limit_items=10",
        "?mode=directory&limit_items=10&offset=0",
        "?mode=directory&limit_items=10&limit_bytes=16",
        "?mode=directory&limit_items=10&unknown=1",
        "?mode=directory&limit_items=10&mode=file",
        "?mode=file&mode=directory&limit_items=10",
        "?mode=Directory&limit_items=10",
        "?mode=directory&limit_items=ten",
        "?mode=directory&limit_items=-1",
        "?mode=directory&limit_items=4294967296",
        "?mode=file&offset=0&limit_bytes=16&limit_items=10",
    ] {
        for dot in [".", "%2E"] {
            let answer = harness.page(OWNER, &workspace, dot, query).await;
            assert_eq!(
                (answer.status, answer.code()),
                (StatusCode::UNPROCESSABLE_ENTITY, "workspace.path-escape"),
                "{dot}{query}: {}",
                answer.body
            );
            assert!(
                answer.body["result"].is_null(),
                "{dot}{query} lists nothing"
            );
        }
        // The same query on a plain path is the query's own refusal, or the file's answer.
        let plain = harness.page(OWNER, &workspace, "a.txt", query).await;
        if query.starts_with("?mode=file&offset=0&limit_bytes=16") && !query.contains("limit_items")
        {
            assert_eq!(plain.status, StatusCode::OK, "a.txt{query}: {}", plain.body);
        } else {
            assert_eq!(
                (plain.status, plain.code()),
                (StatusCode::UNPROCESSABLE_ENTITY, "request.schema-invalid"),
                "a.txt{query}: {}",
                plain.body
            );
        }
    }
}

/// ADR 0052 decision 2: a read in file mode, a write, a delete, every v2 file route and the Git
/// baseline read refuse `.`, in either spelling, and none of them changes the root.
#[tokio::test(flavor = "multi_thread")]
async fn dot_is_refused_by_name_on_every_other_file_operation_and_changes_nothing() {
    let harness = Harness::open();
    let (workspace, host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWOTHEROPS001")
        .await;
    std::fs::write(host.join("a.txt"), b"a").expect("file");
    let before = host_names(&host);
    let digest = sha256_hex(b"a");

    let mut operation = 0_u32;
    for dot in [".", "%2E"] {
        let calls: Vec<(Method, String, Option<Value>)> = vec![
            (
                Method::GET,
                format!("/v1/workspaces/{workspace}/files/{dot}?mode=file&offset=0&limit_bytes=16"),
                None,
            ),
            (
                Method::PUT,
                format!("/v1/workspaces/{workspace}/files/{dot}"),
                Some(json!({ "content": { "encoding": "base64", "data": "eA==" } })),
            ),
            (
                Method::DELETE,
                format!("/v1/workspaces/{workspace}/files/{dot}"),
                Some(json!({})),
            ),
            (
                Method::GET,
                format!("/v2/workspaces/{workspace}/files/{dot}?mode=file&offset=0&limit_bytes=16"),
                None,
            ),
            (
                Method::GET,
                format!("/v2/workspaces/{workspace}/files/{dot}{DIRECTORY_QUERY}"),
                None,
            ),
            (
                Method::GET,
                format!("/v2/workspaces/{workspace}/git/baseline/{dot}?max_bytes=16"),
                None,
            ),
            (
                Method::PUT,
                format!("/v2/workspaces/{workspace}/files/{dot}"),
                Some(json!({
                    "content": { "encoding": "base64", "data": "eA==" },
                    "expected": { "state": "absent" },
                    "create_parents": true
                })),
            ),
            (
                Method::POST,
                format!("/v2/workspaces/{workspace}/file-edits/{dot}"),
                Some(json!({
                    "expected_sha256": digest,
                    "old_text": "a",
                    "new_text": "b",
                    "match_policy": "exact"
                })),
            ),
            (
                Method::POST,
                format!("/v2/workspaces/{workspace}/file-patches/{dot}"),
                Some(json!({
                    "expected_sha256": digest,
                    "edits": [
                        { "kind": "replace_range", "start_line": 1, "end_line": 1, "text": "b" }
                    ]
                })),
            ),
        ];
        for (method, uri, input) in calls {
            operation += 1;
            let body = input.map_or_else(Body::empty, |input| {
                mutation(&format!("01JROOTREVIEWREFUSED{operation:04}"), input)
            });
            let (status, answer) = harness
                .call(OWNER, method.clone(), &uri, "req_root_review_other", body)
                .await;
            assert_eq!(
                (status, answer["error"]["code"].as_str()),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Some("workspace.path-escape")
                ),
                "{method} {uri}: {answer}"
            );
            assert!(
                answer["result"].is_null(),
                "{method} {uri} answers no result"
            );
        }
    }
    assert_eq!(host_names(&host), before, "no refused call left an entry");
    assert_eq!(std::fs::read(host.join("a.txt")).expect("file"), b"a");
}

/// ADR 0052 decision 3: nothing else about the page changes. For the same children the root page
/// and a named directory's page answer the same items, kinds, sizes, order, cursors and omission
/// count: `.git` is neither listed nor counted, the FIFO is counted on every page, and the link is
/// listed as a link and not followed.
#[tokio::test(flavor = "multi_thread")]
async fn the_root_page_answers_what_a_named_directory_with_the_same_children_answers() {
    let harness = Harness::open();
    let outside = tempfile::tempdir().expect("outside");
    std::fs::write(outside.path().join("beyond.txt"), b"outside").expect("outside file");
    let (root_workspace, root_host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWSAMEPAGE001")
        .await;
    populate(&root_host, outside.path());
    let (named_workspace, named_host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWSAMEPAGE002")
        .await;
    std::fs::create_dir(named_host.join("named")).expect("named directory");
    populate(&named_host.join("named"), outside.path());

    let mut cursor: Option<String> = None;
    let mut listed = Vec::new();
    for _ in 0..8 {
        let query = format!(
            "?mode=directory&limit_items=2{}",
            cursor
                .as_ref()
                .map_or_else(String::new, |cursor| format!("&cursor={cursor}"))
        );
        let root = harness.page(OWNER, &root_workspace, ".", &query).await;
        let named = harness.page(OWNER, &named_workspace, "named", &query).await;
        assert_eq!(root.status, StatusCode::OK, "{}", root.body);
        assert_eq!(named.status, StatusCode::OK, "{}", named.body);
        assert_eq!(root.items(), named.items(), "the same page for {query}");
        assert_eq!(root.omitted, named.omitted, "the same omission count");
        assert_eq!(
            root.omitted.as_deref(),
            Some("1"),
            "the FIFO, and not `.git`"
        );
        assert_eq!(
            root.body["result"]["next_cursor"], named.body["result"]["next_cursor"],
            "the same cursor for {query}"
        );
        assert_eq!(root.body["result"]["path"], ".");
        assert_eq!(root.body["result"]["workspace"], root_workspace.as_str());
        listed.extend(root.items());
        cursor = root.body["result"]["next_cursor"]
            .as_str()
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(cursor, None, "the root page reaches an end");
    let kinds: Vec<(&str, &str)> = listed
        .iter()
        .map(|(name, kind, _)| (name.as_str(), kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (".substrate-left.tmp", "file"),
            ("a.txt", "file"),
            ("b.txt", "file"),
            ("c.txt", "file"),
            ("d", "directory"),
            ("z-link", "symlink"),
        ],
        "immediate children only, each once, in name order"
    );

    // Nothing beyond the link is reachable from the root page's entries.
    for through in ["z-link", "z-link/beyond.txt"] {
        let answer = harness
            .page(OWNER, &root_workspace, through, DIRECTORY_QUERY)
            .await;
        assert_eq!(
            (answer.status, answer.code()),
            (StatusCode::UNPROCESSABLE_ENTITY, "workspace.path-escape"),
            "{through}: {}",
            answer.body
        );
    }
    let git = harness
        .page(OWNER, &root_workspace, ".git", DIRECTORY_QUERY)
        .await;
    assert_eq!(git.status, StatusCode::NOT_FOUND, "{}", git.body);
}

/// A cursor names a position, and the directory it is applied to is the one the request names:
/// a cursor the root page gave, sent with another directory or another workspace, answers only
/// that directory's own children or `workspace.cursor-invalid`, and the reverse holds for `.`.
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_only_ever_pages_the_directory_the_request_names() {
    let harness = Harness::open();
    let (workspace, host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWCURSOR00001")
        .await;
    for name in ["r1", "r2", "r3", "r4", "r5"] {
        std::fs::write(host.join(name), b"root").expect("root file");
    }
    std::fs::create_dir(host.join("sub")).expect("sub");
    for name in ["s1", "s2", "s3"] {
        std::fs::write(host.join("sub").join(name), b"sub").expect("sub file");
    }
    let (other, other_host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWCURSOR00002")
        .await;
    std::fs::write(other_host.join("o1"), b"other").expect("other file");

    let page = |path: &'static str, workspace: String, limit: u32, cursor: Option<String>| {
        let harness = &harness;
        async move {
            let cursor = cursor.map_or_else(String::new, |cursor| format!("&cursor={cursor}"));
            harness
                .page(
                    OWNER,
                    &workspace,
                    path,
                    &format!("?mode=directory&limit_items={limit}{cursor}"),
                )
                .await
        }
    };
    let root_first = page(".", workspace.clone(), 2, None).await;
    assert_eq!(root_first.names(), ["r1", "r2"]);
    let root_cursor = root_first.body["result"]["next_cursor"]
        .as_str()
        .expect("a second root page")
        .to_owned();
    let root_fourth = page(".", workspace.clone(), 4, None).await;
    let deep_cursor = root_fourth.body["result"]["next_cursor"]
        .as_str()
        .expect("a root page after four")
        .to_owned();

    // The root's cursor on a named directory: that directory's own children, never the root's.
    let replayed = page("sub", workspace.clone(), 10, Some(root_cursor.clone())).await;
    assert_eq!(replayed.status, StatusCode::OK, "{}", replayed.body);
    assert_eq!(replayed.body["result"]["path"], "sub");
    assert!(
        replayed.names().iter().all(|name| name.starts_with('s')),
        "only `sub`'s children: {:?}",
        replayed.names()
    );
    // A position past that directory's end is refused by name.
    let past = page("sub", workspace.clone(), 10, Some(deep_cursor.clone())).await;
    assert_eq!(
        (past.status, past.code()),
        (StatusCode::UNPROCESSABLE_ENTITY, "workspace.cursor-invalid"),
        "{}",
        past.body
    );
    // The root's cursor on another workspace's root: that workspace's children or the refusal.
    let elsewhere = page(".", other.clone(), 10, Some(root_cursor)).await;
    assert_eq!(
        (elsewhere.status, elsewhere.code()),
        (StatusCode::UNPROCESSABLE_ENTITY, "workspace.cursor-invalid"),
        "{}",
        elsewhere.body
    );
    // A named directory's cursor on the root: the root's own children from that position.
    let sub_first = page("sub", workspace.clone(), 1, None).await;
    let sub_cursor = sub_first.body["result"]["next_cursor"]
        .as_str()
        .expect("a second page of `sub`")
        .to_owned();
    let on_root = page(".", workspace.clone(), 10, Some(sub_cursor)).await;
    assert_eq!(on_root.status, StatusCode::OK, "{}", on_root.body);
    assert_eq!(on_root.names(), ["r2", "r3", "r4", "r5", "sub"]);
}

/// The root page is admitted like a named directory's page: a subject that does not hold the
/// workspace gets the answer it gets for any path in it, and learns nothing from `.`.
#[tokio::test(flavor = "multi_thread")]
async fn another_subject_gets_the_named_directory_answer_for_the_root_page() {
    let harness = Harness::open();
    let (workspace, host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWSUBJECT0001")
        .await;
    std::fs::create_dir(host.join("d")).expect("directory");
    std::fs::write(host.join("secret.txt"), b"secret").expect("file");

    let owner = harness.page(OWNER, &workspace, ".", DIRECTORY_QUERY).await;
    assert_eq!(owner.names(), ["d", "secret.txt"]);

    let stranger_root = harness
        .page("local:2000", &workspace, ".", DIRECTORY_QUERY)
        .await;
    let stranger_named = harness
        .page("local:2000", &workspace, "d", DIRECTORY_QUERY)
        .await;
    assert_eq!(
        (stranger_root.status, stranger_root.code()),
        (StatusCode::NOT_FOUND, "resource.not-found"),
        "{}",
        stranger_root.body
    );
    assert_eq!(
        (stranger_root.status, stranger_root.code()),
        (stranger_named.status, stranger_named.code())
    );
    assert!(stranger_root.body["result"].is_null());
    assert_eq!(stranger_root.omitted, None);
}

/// The safety envelope's `openat2` rule holds for `.`: a workspace root that has become a link is
/// not followed, and the root page answers what a named directory's page answers for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_that_became_a_link_is_refused_and_never_followed() {
    let harness = Harness::open();
    let outside = tempfile::tempdir().expect("outside");
    std::fs::create_dir(outside.path().join("d")).expect("outside directory");
    std::fs::write(outside.path().join("beyond.txt"), b"outside").expect("outside file");
    let (workspace, host) = harness
        .review_workspace(OWNER, "01JROOTREVIEWLINKEDROOT1")
        .await;
    std::fs::remove_dir(&host).expect("the empty root");
    std::os::unix::fs::symlink(outside.path(), &host).expect("a link where the root was");

    let root = harness.page(OWNER, &workspace, ".", DIRECTORY_QUERY).await;
    let named = harness.page(OWNER, &workspace, "d", DIRECTORY_QUERY).await;
    assert_ne!(root.status, StatusCode::OK, "{}", root.body);
    assert!(root.body["result"].is_null(), "nothing outside is listed");
    assert_eq!(
        (root.status, root.code()),
        (StatusCode::UNPROCESSABLE_ENTITY, "workspace.path-escape"),
        "{}",
        root.body
    );
    assert_eq!(
        (root.status, root.code()),
        (named.status, named.code()),
        "the root page and a named directory's page agree"
    );
}
