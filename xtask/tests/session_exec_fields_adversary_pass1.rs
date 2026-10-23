#![forbid(unsafe_code)]
//! Adversary pass 1 for `story:session-start-publishes-exec-fields` (design 27, bundle `0.23.0`).
//!
//! Three questions, each answered by a program rather than by reading:
//!
//! 1. Does `check-bundle 0.23.0` (through `xtask/src/bundle/session_exec_fields.rs`) refuse a
//!    session-start `exec` object that no longer has exec start's contract, when the drift is in
//!    the object's own `required` list or `additionalProperties` rather than in a member schema?
//!    Each mutant is rendered into a scratch tree under `CARGO_TARGET_TMPDIR`; nothing under
//!    `contracts/` or `xtask/bundle-source/` is written.
//! 2. For the three fields this unit published, does the bundle's session-start schema agree with
//!    what the daemon decodes and admits (the wire type plus the wire crate's own shape checks the
//!    daemon runs before any host fact is read)?
//! 3. Acceptance 1 of the story: a field refused before dispatch is refused "with a named, published
//!    code". The unit's own delegated vector expects `exec.scratch-quota-unserved` for `scratch` on a
//!    session; is that code anywhere in the bundle?

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const VERSION: &str = "0.23.0";
const PREDECESSOR: &str = "0.22.0";
const SESSION_START: &str = "documents/schemas/inputs/pipe-session-start.json";
const RESOURCE_ROOT: &str = "https://b10x.invalid/substrate-wire";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn bundle(version: &str) -> PathBuf {
    repo_root().join("contracts/substrate-wire").join(version)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable tree") {
        let entry = entry.expect("entry");
        if entry.file_type().expect("type").is_dir() {
            files(&entry.path(), out);
        } else {
            out.push(entry.path());
        }
    }
}

fn validator(version: &str, target: &str) -> jsonschema::Validator {
    let root = bundle(version);
    let mut all = Vec::new();
    files(&root.join("schemas"), &mut all);
    let mut registry = jsonschema::Registry::new();
    for path in all {
        let relative = path
            .strip_prefix(&root)
            .expect("beneath the bundle")
            .to_str()
            .expect("UTF-8")
            .to_owned();
        let mut document = read_json(&path);
        if let Some(object) = document.as_object_mut() {
            object.remove("$id");
        }
        registry = registry
            .add(format!("{RESOURCE_ROOT}/{version}/{relative}"), document)
            .expect("registers");
    }
    let registry = registry.prepare().expect("a registry");
    jsonschema::draft202012::options()
        .with_registry(&registry)
        .build(&json!({ "$ref": format!("{RESOURCE_ROOT}/{version}/{target}") }))
        .expect("compiles")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("scratch directory");
    for entry in fs::read_dir(from).expect("readable tree") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// Renders `0.23.0` from a mutated copy of its source and runs `check-bundle` on the result.
fn check_bundle_after(document: &str, mutate: impl FnOnce(&mut Value)) -> (bool, String) {
    let scratch = tempfile::Builder::new()
        .prefix("session-exec-adversary-1-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("scratch tree");
    let source = scratch.path().join("source");
    let contracts = scratch.path().join("contracts");
    copy_tree(
        &repo_root().join("xtask/bundle-source").join(VERSION),
        &source.join(VERSION),
    );
    copy_tree(&bundle(PREDECESSOR), &contracts.join(PREDECESSOR));

    let authored = source.join(VERSION).join(document);
    let mut value = read_json(&authored);
    mutate(&mut value);
    fs::write(
        &authored,
        serde_json::to_vec_pretty(&value).expect("serialize mutant"),
    )
    .expect("write mutant");

    let rendered = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["render-bundle", VERSION])
        .arg("--source")
        .arg(&source)
        .arg("--contracts-root")
        .arg(&contracts)
        .arg("--out")
        .arg(contracts.join(VERSION))
        .arg("--force")
        .output()
        .expect("run render-bundle");
    assert!(
        rendered.status.success(),
        "the mutant has to render:\n{}{}",
        String::from_utf8_lossy(&rendered.stdout),
        String::from_utf8_lossy(&rendered.stderr)
    );
    let checked = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check-bundle", VERSION])
        .arg("--source")
        .arg(&source)
        .arg("--contracts-root")
        .arg(&contracts)
        .output()
        .expect("run check-bundle");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    (checked.status.success(), said)
}

fn session_exec(document: &mut Value) -> &mut serde_json::Map<String, Value> {
    document["properties"]["exec"]
        .as_object_mut()
        .expect("the session start has an exec object")
}

#[test]
fn control_the_unmutated_scratch_bundle_checks_green() {
    let (passed, said) = check_bundle_after(SESSION_START, |_| {});
    assert!(
        passed,
        "the unmutated scratch bundle must check green:\n{said}"
    );
}

/// Design 27: "the session exec carries exactly exec start's fields with the same schemas". A
/// session schema that *requires* one of the optional exec-start fields refuses every session
/// start that omits it, which the daemon admits. `check_members` compares only `properties`.
#[test]
fn check_bundle_refuses_a_session_exec_that_requires_an_optional_exec_field() {
    let (passed, said) = check_bundle_after(SESSION_START, |document| {
        session_exec(document)["required"]
            .as_array_mut()
            .expect("required list")
            .push(json!("workspace_access"));
    });
    assert!(
        !passed,
        "check-bundle 0.23.0 accepted a session-start exec that requires workspace_access, which \
         exec start leaves optional and the daemon defaults to read-write; the class check does \
         not compare required lists:\n{said}"
    );
}

/// The converse: a session schema that stops requiring `argv` admits a body the daemon refuses
/// `request.schema-invalid` (serde: missing field).
#[test]
fn check_bundle_refuses_a_session_exec_that_drops_a_required_exec_field() {
    let (passed, said) = check_bundle_after(SESSION_START, |document| {
        session_exec(document)["required"]
            .as_array_mut()
            .expect("required list")
            .retain(|name| name != "argv");
    });
    assert!(
        !passed,
        "check-bundle 0.23.0 accepted a session-start exec that no longer requires argv, which \
         exec start requires and the daemon cannot decode without:\n{said}"
    );
}

/// An open session exec object admits any member, which the daemon's `deny_unknown_fields`
/// `ExecStartInput` refuses. `check_members` never reads `additionalProperties`.
#[test]
fn check_bundle_refuses_a_session_exec_that_admits_unknown_members() {
    let (passed, said) = check_bundle_after(SESSION_START, |document| {
        session_exec(document).insert("additionalProperties".to_owned(), json!(true));
    });
    assert!(
        !passed,
        "check-bundle 0.23.0 accepted a session-start exec with additionalProperties true, while \
         exec start is closed and the daemon refuses an unknown member:\n{said}"
    );
}

/// A schema-valid session start with none of the three fields.
fn session_body() -> Value {
    json!({
        "exec": {
            "workspace": "ws_check",
            "argv": ["/usr/bin/true"],
            "env": { "allow": [], "set": {} },
            "sandbox": {
                "capability_snapshot": format!("sha256:{}", "0".repeat(64)),
                "profile": "workspace",
                "network": "none",
                "require": true
            },
            "limits": {
                "timeout_ms": 1000,
                "output_bytes": 1024,
                "processes": 1,
                "memory_bytes": 1_048_576,
                "cpu_millis": 1000
            },
            "wait": false,
            "lease_ttl_ms": 60_000
        },
        "input_limit_bytes": 1024,
        "frame_limit_bytes": 1024,
        "queued_frames": 1
    })
}

/// What the daemon does with a session start's three published fields before it reads any host
/// fact: `decode_mutation` decodes the wire type (`serde_json::from_value`, anything else is
/// `request.schema-invalid`), and `validate_exec_input`'s shape step checks
/// `StorageLimit::within_contract_bounds`; the driver's `admit_workspace_access` runs
/// `validate_workspace_access`. `true` means the daemon carries the request past its shape checks.
fn daemon_admits_shape(body: &Value) -> bool {
    let Ok(start) = serde_json::from_value::<substrate_wire::PipeSessionStartInput>(body.clone())
    else {
        return false;
    };
    start
        .exec
        .scratch
        .is_none_or(substrate_wire::StorageLimit::within_contract_bounds)
        && substrate_wire::validate_workspace_access(&start.exec.workspace_access).is_ok()
}

/// The published session-start schema and the daemon's decoding agree on every value of the three
/// fields: a body the schema refuses is not carried past the daemon's shape checks, and the reverse.
#[test]
fn the_published_session_fields_agree_with_what_the_daemon_decodes() {
    let schema = validator(VERSION, "schemas/inputs/pipe-session-start.json");
    let base = session_body();
    assert!(
        schema.is_valid(&base),
        "control: the base body is schema-valid"
    );
    assert!(
        daemon_admits_shape(&base),
        "control: the daemon admits the base body"
    );

    let cases: [(&str, Value); 12] = [
        (
            "scratch",
            json!({ "max_bytes": 1_048_576, "max_inodes": 16 }),
        ),
        ("scratch", Value::Null),
        (
            "scratch",
            json!({ "max_bytes": 1_048_577, "max_inodes": 16 }),
        ),
        (
            "scratch",
            json!({ "max_bytes": 1_048_576, "max_inodes": 15 }),
        ),
        ("measurements", json!([])),
        ("measurements", json!(["resource-usage"])),
        ("measurements", json!(["resource-usage", "resource-usage"])),
        ("measurements", Value::Null),
        ("workspace_access", json!({ "mode": "read-only" })),
        (
            "workspace_access",
            json!({ "mode": "scoped", "writable_subtrees": ["a", "b"] }),
        ),
        (
            "workspace_access",
            json!({ "mode": "scoped", "writable_subtrees": [] }),
        ),
        ("workspace_access", Value::Null),
    ];
    let mut disagreements = Vec::new();
    for (field, value) in cases {
        let mut body = base.clone();
        body["exec"][field] = value.clone();
        let published = schema.is_valid(&body);
        let decoded = daemon_admits_shape(&body);
        if published != decoded {
            disagreements.push(format!(
                "exec.{field} = {value}: schema {}, daemon {}",
                if published { "admits" } else { "refuses" },
                if decoded { "admits" } else { "refuses" },
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "the 0.23.0 session-start schema and the daemon disagree:\n{}",
        disagreements.join("\n")
    );
}

/// Story acceptance 1: each field is "accepted and honoured, or refused before dispatch with a
/// named, published code". The unit's own delegated vector
/// (`check_confined_session_exec_fields`, `runtime_vectors.rs`) expects a session naming `scratch`
/// to be refused `exec.scratch-quota-unserved` (501) before dispatch on every host without project
/// quotas, which is every host the gate and the delegated lane run on. The code has to be in the
/// bundle for that branch of the acceptance to hold.
#[test]
fn the_refusal_a_session_gets_for_scratch_is_published_in_the_bundle() {
    let mut all = Vec::new();
    files(&bundle(VERSION), &mut all);
    let published: Vec<String> = all
        .iter()
        .filter(|path| {
            fs::read_to_string(path).is_ok_and(|text| text.contains("exec.scratch-quota-unserved"))
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        !published.is_empty(),
        "no file of contracts/substrate-wire/{VERSION} names exec.scratch-quota-unserved, the \
         refusal the unit's own session vector expects for scratch"
    );
}
