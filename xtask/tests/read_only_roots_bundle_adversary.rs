#![forbid(unsafe_code)]
//! Adversary cases for bundle `0.23.0` (`story:bundle-publishes-read-only-roots`, ADR 0010).
//!
//! Three questions the unit's own checks do not ask:
//!
//! 1. ADR 0010 § *Consequences* says the bundle grows a bounded field and "the refusals above need
//!    vectors for path escape, mount collision, a non-directory, and the bound". `0.23.0` publishes
//!    the three codes as annotations and no vector that answers one of them.
//! 2. A real exec record is the wire's `AppliedConfinement`, which serializes `read_only_roots`
//!    beside `read_only_files` and `secret_slots`. The published exec result must accept the record
//!    of a start that named roots and files together.
//! 3. `xtask/src/bundle/read_only_roots.rs` reads the mount annotations one way only: every owned
//!    mount must be listed, but nothing says the list holds only owned mounts, that the
//!    duplicate/overlap rule is still stated, or what the fact's absence is said to mean. Each
//!    mutant below is rendered into a scratch tree and `check-bundle` must refuse it.
//!
//! Nothing under `contracts/` or `xtask/bundle-source/` is written; both are copied into a scratch
//! directory under `CARGO_TARGET_TMPDIR`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const VERSION: &str = "0.23.0";
const PREDECESSOR: &str = "0.22.0";
const COMMON: &str = "documents/schemas/common.json";
const CAPABILITY: &str = "documents/schemas/capability.json";
const APPLIED_VECTOR: &str = "vectors/http/read-only-roots-applied-are-observed.json";
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

/// A validator for `target` against the bundle's own schema registry, as `check-bundle` builds it.
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

/// The exec-start result of the bundle's own roots vector, with one read-only file bound too.
///
/// The start is one the wire crate admits (the file is not on, beneath or above any root), and the
/// host driver records `read_only_files` in the same `AppliedConfinement` it records the roots in
/// (`crates/substrate-host/src/process.rs`, the `AppliedConfinement` literal).
fn roots_and_files_result(version: &str, roots: bool) -> Value {
    let vector = read_json(&bundle(VERSION).join(APPLIED_VECTOR));
    let mut result = vector["expected"]["response"]["body"]["result"].clone();
    let file = json!({ "host_path": "/etc/gitconfig", "mount": "/etc/gitconfig" });
    let applied = result["applied"].as_object_mut().expect("applied record");
    if !roots {
        applied.remove("read_only_roots");
    }
    applied.insert("read_only_files".to_owned(), json!([file]));

    let mut input = vector["action"]["request"]["body"]["input"].clone();
    if !roots {
        input
            .as_object_mut()
            .expect("input")
            .remove("read_only_roots");
    }
    input["read_only_files"] = json!([file]);
    let start: substrate_wire::ExecStartInput =
        serde_json::from_value(input).expect("the wire type parses the start");
    substrate_wire::validate_read_only_roots(&start.read_only_roots).expect("roots admitted");
    substrate_wire::validate_read_only_files(&start.read_only_files, &start.read_only_roots)
        .expect("file admitted beside the roots");
    let record: substrate_wire::AppliedConfinement =
        serde_json::from_value(result["applied"].clone()).expect("the wire record parses");
    assert_eq!(
        serde_json::to_value(&record).expect("serializes"),
        result["applied"],
        "{version}: the record is exactly what the wire serializes"
    );
    result
}

#[test]
fn bundle_publishes_a_vector_for_each_read_only_root_refusal() {
    let root = bundle(VERSION);
    let mut all = Vec::new();
    files(&root.join("vectors"), &mut all);
    let mut answered = std::collections::BTreeSet::new();
    for path in all {
        let vector = read_json(&path);
        if vector.pointer("/action/operation") != Some(&json!("exec.start")) {
            continue;
        }
        if let Some(code) = vector
            .pointer("/expected/response/body/error/code")
            .and_then(Value::as_str)
        {
            answered.insert(code.to_owned());
        }
    }
    let missing: Vec<&str> = substrate_wire::READ_ONLY_ROOT_REFUSAL_CODES
        .into_iter()
        .filter(|code| !answered.contains(*code))
        .collect();
    assert!(
        missing.is_empty(),
        "ADR 0010 requires refusal vectors (path escape, mount collision, non-directory, bound); \
         bundle {VERSION} publishes none for: {missing:?}"
    );
}

#[test]
fn an_exec_record_binding_roots_and_files_validates_against_the_published_result() {
    let result = roots_and_files_result(VERSION, true);
    let schema = validator(VERSION, "schemas/results/exec-start.json");
    let errors: Vec<String> = schema
        .iter_errors(&result)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "bundle {VERSION} rejects the wire record of a start naming roots and a file: {errors:?}"
    );
}

/// Origin probe: the same record without roots, against the predecessor. Run with `--ignored`.
#[test]
#[ignore = "origin probe against the frozen predecessor; run explicitly"]
fn origin_probe_predecessor_rejects_an_exec_record_binding_a_file() {
    let result = roots_and_files_result(PREDECESSOR, false);
    let schema = validator(PREDECESSOR, "schemas/results/exec-start.json");
    assert!(
        schema.is_valid(&result),
        "bundle {PREDECESSOR} already rejected the wire record of a start naming a file"
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

/// Renders `0.23.0` from a mutated copy of its authored `document` and runs `check-bundle` on it.
fn check_bundle_after(document: &str, mutate: impl FnOnce(&mut Value)) -> (bool, String) {
    let scratch = tempfile::Builder::new()
        .prefix("roroot-adversary-")
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

/// Control: the unmutated source renders and checks green in the scratch layout, so a green
/// mutant below is the checker accepting the mutation and not a broken harness.
#[test]
fn control_the_unmutated_scratch_bundle_checks_green() {
    let (passed, said) = check_bundle_after(COMMON, |_| {});
    assert!(
        passed,
        "the unmutated scratch bundle must check green:\n{said}"
    );
}

/// The wire admits `/opt` as a root mount; a bundle that publishes it as reserved tells a client
/// a start is refused that the wire admits.
#[test]
fn check_bundle_refuses_a_reserved_mount_the_wire_does_not_reserve() {
    let (passed, said) = check_bundle_after(COMMON, |document| {
        document["$defs"]["read-only-root"]["properties"]["mount"]
            ["x-b10x-not-equal-beneath-or-above"]
            .as_array_mut()
            .expect("reserved list")
            .insert(0, json!("/opt"));
    });
    assert!(
        !passed,
        "check-bundle accepted a read-only-root mount reserving /opt, which \
         substrate_wire::validate_read_only_roots admits:\n{said}"
    );
}

/// `validate_read_only_roots` refuses a duplicate and an overlapping mount; a bundle that drops
/// both statements of that rule publishes a list the wire refuses as admissible.
#[test]
fn check_bundle_refuses_a_schema_that_drops_the_duplicate_and_overlap_rule() {
    let (passed, said) = check_bundle_after(COMMON, |document| {
        document["$defs"]["read-only-roots"]
            .as_object_mut()
            .expect("list")
            .remove("x-b10x-unique");
        document["$defs"]["read-only-root"]["properties"]["mount"]
            ["x-b10x-not-equal-beneath-or-above"]
            .as_array_mut()
            .expect("reserved list")
            .retain(|entry| entry != "every-other-declared-read-only-root-mount");
    });
    assert!(
        !passed,
        "check-bundle accepted a read-only-roots schema stating neither the unique-mount nor the \
         overlapping-mount rule the wire enforces:\n{said}"
    );
}

/// The fact's absence means `exec.sandbox-unavailable` (501) in this daemon; a bundle that says a
/// root is then unserved by a name nobody raises is accepted while the annotation merely exists.
#[test]
fn check_bundle_refuses_a_fact_whose_absence_names_a_refusal_nobody_raises() {
    let (passed, said) = check_bundle_after(CAPABILITY, |document| {
        document["properties"]["facts"]["properties"]["exec.read-only-roots"]["x-b10x-absent-means"] =
            json!("every-declared-read-only-root-is-unserved-by-name");
    });
    assert!(
        !passed,
        "check-bundle accepted exec.read-only-roots absence described as an unserved refusal \
         the daemon never raises:\n{said}"
    );
}
