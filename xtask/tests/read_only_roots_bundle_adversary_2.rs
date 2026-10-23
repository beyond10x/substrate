#![forbid(unsafe_code)]
//! Adversary pass 2 for bundle `0.23.0` (`story:bundle-publishes-read-only-roots`, ADR 0010).
//!
//! Correction round 1 widened the exec `applied` shapes, added five refusal vectors and taught
//! `xtask/src/bundle/read_only_roots.rs` to hold them. Two questions:
//!
//! 1. Does every record `substrate_wire::Exec` can serialize, in every state the exec resource
//!    publishes, validate against `schemas/results/exec-get.json`? The record is round-tripped
//!    through the wire type first, so it is a record the daemon can emit, not a hand-made one.
//! 2. Does `check-bundle 0.23.0` refuse a source whose applied shapes contradict
//!    `AppliedConfinement` again, or whose refusal vectors publish an answer the daemon does not
//!    give? Each mutant is rendered into a scratch tree under `CARGO_TARGET_TMPDIR`; nothing under
//!    `contracts/` or `xtask/bundle-source/` is written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const VERSION: &str = "0.23.0";
const PREDECESSOR: &str = "0.22.0";
const RESOURCE: &str = "documents/schemas/resource.json";
const PATH_ESCAPE: &str = "documents/vectors/http/read-only-root-path-escape-is-refused.json";
const RESOURCE_ROOT: &str = "https://b10x.invalid/substrate-wire";
const SNAPSHOT: &str = "sha256:7777777777777777777777777777777777777777777777777777777777777777";

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

fn network(kind: &str) -> Value {
    let aperture = |name: &str, destination: &str| {
        json!({
            "bytes": { "from_destination": 7, "to_destination": 9 },
            "destination": destination,
            "mechanism": "loopback-forwarder",
            "mode": "aperture",
            "name": name,
        })
    };
    match kind {
        "none" => json!("none"),
        "aperture" => {
            let mut one = aperture("model", "203.0.113.7:443");
            one["max_bytes"] = json!(1_048_576);
            one
        }
        "list" => {
            let mut first = aperture("model", "203.0.113.7:443");
            first["listen"] = json!("127.0.0.1:443");
            let mut second = aperture("search", "198.51.100.9:443");
            second["listen"] = json!("127.0.0.2:443");
            second["max_bytes"] = json!(4096);
            json!([first, second])
        }
        other => panic!("no network {other}"),
    }
}

/// Every optional member `AppliedConfinement` serializes, each at its wire bound.
fn applied(capsule: bool, net: &str) -> Value {
    let roots: Vec<Value> = (1..=substrate_wire::MAX_READ_ONLY_ROOTS)
        .map(|i| json!({ "host_path": format!("/srv/root{i}"), "mount": format!("/opt/root{i}") }))
        .collect();
    let read_only_files: Vec<Value> = (1..=16)
        .map(|i| json!({ "host_path": format!("/etc/file{i}"), "mount": format!("/etc/file{i}") }))
        .collect();
    let slots: Vec<Value> = (0..substrate_wire::MAX_SECRET_SLOTS)
        .map(|i| {
            json!({
                "slot": format!("slot_{i}"),
                "fd": if i == 0 { substrate_wire::MIN_SECRET_SLOT_FD } else { substrate_wire::MAX_SECRET_SLOT_FD - i },
            })
        })
        .collect();
    let mut record = json!({
        "capability_snapshot": SNAPSHOT,
        "cgroup": "substrate/vector",
        "filesystem": if capsule { "workspace-rw-capsule-ro-system-ro" } else { "workspace-rw-system-ro" },
        "network": network(net),
        "profile": "workspace",
        "workspace_access": { "mode": "scoped", "writable_subtrees": ["artifacts"] },
        "read_only_roots": roots,
        "read_only_files": read_only_files,
        "secret_slots": slots,
        "workdir": "/opt/root1/src",
        "writable_roots": [
            { "kind": "host", "name": "cache", "host_path": "/srv/cache", "mount": "/cache" },
            { "kind": "git", "workspace": "ws_second", "host_path": "/srv/ws/second", "mount": "/src" }
        ],
    });
    if capsule {
        record["capsule"] = json!({
            "manifest_sha256": "a".repeat(64),
            "entrypoint": "/runtime/bin/tool",
            "mount": "/runtime",
            "file_count": 3,
            "total_bytes": 4096,
        });
    }
    record
}

fn exec_record(state: &str, capsule: bool, net: &str) -> Value {
    let (exit, refusal) = match state {
        "running" | "expired" => (Value::Null, None),
        "exited" => (json!({ "code": 0, "signal": null }), None),
        "cancelled" => (
            json!({ "code": null, "signal": "KILL" }),
            Some(
                json!({ "class": "exhausted", "code": "exec.aperture-byte-limit", "message": "The declared aperture byte ceiling was reached." }),
            ),
        ),
        "unknown" => (
            Value::Null,
            Some(
                json!({ "class": "refused", "code": "exec.workdir-missing", "message": "The working directory was not there." }),
            ),
        ),
        other => panic!("no state {other}"),
    };
    let mut record = json!({
        "id": "ex_vector",
        "kind": "exec",
        "workspace": "ws_vector",
        "state": state,
        "observed_at": "2026-08-13T12:00:00Z",
        "requested": {
            "capability_snapshot": SNAPSHOT,
            "network": "none",
            "profile": "workspace",
            "require": true
        },
        "applied": applied(capsule, net),
        "exit": exit,
    });
    if let Some(refusal) = refusal {
        record["refusal"] = refusal;
    }
    let wire: substrate_wire::Exec =
        serde_json::from_value(record.clone()).expect("the wire Exec type parses the record");
    let emitted = serde_json::to_value(&wire).expect("serializes");
    assert_eq!(
        emitted, record,
        "{state}: the record is exactly what the wire serializes"
    );
    emitted
}

#[test]
fn every_wire_exec_record_binding_every_applied_member_validates_against_exec_get() {
    let schema = validator(VERSION, "schemas/results/exec-get.json");
    let mut rejected = Vec::new();
    for state in ["running", "exited", "cancelled", "expired", "unknown"] {
        for capsule in [false, true] {
            for net in ["none", "aperture", "list"] {
                let record = exec_record(state, capsule, net);
                let errors: Vec<String> = schema
                    .iter_errors(&record)
                    .map(|error| format!("{} at {}", error, error.instance_path()))
                    .take(3)
                    .collect();
                if !errors.is_empty() {
                    rejected.push(format!("{state}/capsule={capsule}/{net}: {errors:?}"));
                }
            }
        }
    }
    assert!(
        rejected.is_empty(),
        "bundle {VERSION} rejects {} wire exec records:\n{}",
        rejected.len(),
        rejected.join("\n")
    );
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

fn check_bundle_after(document: &str, mutate: impl FnOnce(&mut Value)) -> (bool, String) {
    let scratch = tempfile::Builder::new()
        .prefix("roroot-adversary-2-")
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

/// Applies `edit` to the `properties` of every closed applied-confinement shape in `resource.json`.
fn each_applied_shape(
    value: &mut Value,
    edit: &dyn Fn(&mut serde_json::Map<String, Value>),
) -> usize {
    let mut edited = 0;
    match value {
        Value::Object(object) => {
            let is_applied = object.get("additionalProperties") == Some(&json!(false))
                && object
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|p| {
                        p.contains_key("capability_snapshot") && p.contains_key("cgroup")
                    });
            if is_applied {
                edit(object["properties"].as_object_mut().expect("properties"));
                edited += 1;
            }
            for child in object.values_mut() {
                edited += each_applied_shape(child, edit);
            }
        }
        Value::Array(items) => {
            for child in items {
                edited += each_applied_shape(child, edit);
            }
        }
        _ => {}
    }
    edited
}

#[test]
fn control_the_unmutated_scratch_bundle_checks_green() {
    let (passed, said) = check_bundle_after(RESOURCE, |_| {});
    assert!(
        passed,
        "the unmutated scratch bundle must check green:\n{said}"
    );
}

/// The correction's own subject: a source that forbids `secret_slots` in every applied shape again
/// restores the 0.18.0-to-0.22.0 defect. `check_applied_record` only asks that the member be named.
#[test]
fn check_bundle_refuses_applied_shapes_that_forbid_secret_slots_again() {
    let (passed, said) = check_bundle_after(RESOURCE, |document| {
        let edited = each_applied_shape(document, &|properties| {
            properties.insert("secret_slots".to_owned(), json!(false));
        });
        assert!(edited > 0, "the mutant found no applied shape");
    });
    assert!(
        !passed,
        "check-bundle accepted applied shapes whose secret_slots member is `false`, rejecting \
         every record AppliedConfinement emits for a start that placed a slot:\n{said}"
    );
}

/// `read_only_files` at one item and a secret-slot fd ceiling of 3 contradict the wire's 16 and 63.
#[test]
fn check_bundle_refuses_applied_bounds_that_contradict_the_wire() {
    let (passed, said) = check_bundle_after(RESOURCE, |document| {
        let edited = each_applied_shape(document, &|properties| {
            if let Some(files) = properties.get_mut("read_only_files") {
                files["maxItems"] = json!(1);
            }
            if let Some(slots) = properties.get_mut("secret_slots") {
                slots["items"]["properties"]["fd"]["maximum"] = json!(3);
            }
        });
        assert!(edited > 0, "the mutant found no applied shape");
    });
    assert!(
        !passed,
        "check-bundle accepted applied read_only_files maxItems 1 and secret-slot fd maximum 3, \
         which substrate_wire::AppliedConfinement exceeds:\n{said}"
    );
}

/// The widened expired branch: dropping `workdir` content to `false` there is a narrower mutant.
#[test]
fn check_bundle_refuses_an_expired_applied_shape_that_forbids_writable_roots() {
    let (passed, said) = check_bundle_after(RESOURCE, |document| {
        let branches = document["$defs"]["exec"]["oneOf"]
            .as_array_mut()
            .expect("exec branches");
        let expired = branches
            .iter_mut()
            .find(|branch| branch["properties"]["state"] == json!({ "const": "expired" }))
            .expect("expired branch");
        let edited = each_applied_shape(&mut expired["properties"]["applied"], &|properties| {
            properties.insert("writable_roots".to_owned(), json!(false));
            properties.insert("workdir".to_owned(), json!(false));
        });
        assert!(
            edited > 0,
            "the mutant found no applied shape in the expired branch"
        );
    });
    assert!(
        !passed,
        "check-bundle accepted an expired-state applied shape forbidding writable_roots and \
         workdir, which the compatibility block says this bundle now admits:\n{said}"
    );
}

/// The daemon answers the path-escape start with `A declared read-only root is invalid: {error}`
/// (`check_read_only_roots`). No executable vector runs these, so the checker is the only thing
/// that could hold the published message to the daemon's.
#[test]
fn check_bundle_refuses_a_refusal_vector_whose_message_the_daemon_never_sends() {
    let (passed, said) = check_bundle_after(PATH_ESCAPE, |document| {
        document["expected"]["response"]["body"]["error"]["message"] =
            json!("The root was rejected for reasons this daemon never states.");
    });
    assert!(
        !passed,
        "check-bundle accepted a read-only-root refusal vector whose message is not the daemon's:\n{said}"
    );
}
