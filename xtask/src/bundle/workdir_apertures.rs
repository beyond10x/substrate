//! What `0.21.0` exists for (design 26): the working directory a start names, and several egress
//! apertures per run.
//!
//! Every value this check expects comes from `substrate_wire`: the byte bound of a working
//! directory, the most apertures a list may name, the fact name and each new refusal's code, status,
//! class and message. A bound that moves in the crate and not in the bundle, or a refusal the
//! crate emits that no vector answers with its exact message, fails here. The bundle cannot bind
//! these through a `{"$wire": …}` marker: `render_v2.rs` is hashed by every bundle it rendered
//! (AGENTS.md § *The gate*), so the binding lives in this file, which no bundle hashes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

/// The two start inputs that carry an exec start: the exec itself and the session that embeds it.
const STARTS: [(&str, &str); 2] = [
    ("schemas/inputs/exec-start.json", ""),
    ("schemas/inputs/pipe-session-start.json", "/properties/exec"),
];

/// Where the working directory is defined once, for both starts and the applied record.
const WORKDIR_DEFINITION: &str = "/$defs/workdir";

/// The lexical pattern a working directory has: `/`, or one or more `/component` with no empty,
/// `.` or `..` component and no NUL.
const WORKDIR_PATTERN: &str = "^(?:/|(?:/(?!\\.\\.?(?:/|$))[^/\\u0000]+)+)$";

pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_workdir_definition(released, failures);
    check_starts(released, failures);
    check_fact(released, failures);
    check_applied_record(released, failures);
    check_vectors(version, released, failures);
    check_client_visible_changes(released, failures);
}

fn max_apertures() -> Value {
    json!(wire::MAX_EGRESS_APERTURES)
}

fn class_of(class: wire::ErrorClass) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `(code, status, class)` for each row of a refusal table.
fn table(refusals: &[wire::ExecStartRefusal]) -> BTreeSet<(String, u64, String)> {
    refusals
        .iter()
        .map(|refusal| {
            (
                refusal.code.to_owned(),
                u64::from(refusal.status),
                class_of(refusal.class),
            )
        })
        .collect()
}

/// `(code, status, class)` for each row an `x-b10x-refusals` annotation declares.
fn declared(
    annotation: Option<&Value>,
    path: &str,
    failures: &mut Vec<String>,
) -> BTreeSet<(String, u64, String)> {
    let mut rows = BTreeSet::new();
    for row in annotation.and_then(Value::as_array).into_iter().flatten() {
        let (Some(code), Some(status), Some(class)) = (
            row.get("code").and_then(Value::as_str),
            row.get("status").and_then(Value::as_u64),
            row.get("class").and_then(Value::as_str),
        ) else {
            failures.push(format!(
                "{path}: a declared refusal lacks code, status or class"
            ));
            continue;
        };
        rows.insert((code.to_owned(), status, class.to_owned()));
    }
    rows
}

fn compare_tables(
    path: &str,
    what: &str,
    expected: &BTreeSet<(String, u64, String)>,
    published: &BTreeSet<(String, u64, String)>,
    failures: &mut Vec<String>,
) {
    for (code, status, class) in expected.difference(published) {
        failures.push(format!(
            "{path}: {what} does not declare {code} ({status}, {class})"
        ));
    }
    for (code, status, class) in published.difference(expected) {
        failures.push(format!(
            "{path}: {what} declares {code} ({status}, {class}), which is not a refusal the wire \
             crate emits there"
        ));
    }
}

/// The working directory's bound, pattern, capability and refusals are the wire crate's.
fn check_workdir_definition(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/common.json";
    let Some(common) = json_at(released, path, failures) else {
        return;
    };
    let Some(workdir) = common.pointer(WORKDIR_DEFINITION) else {
        failures.push(format!("{path}: no workdir definition"));
        return;
    };
    if workdir.get("maxLength") != Some(&json!(wire::MAX_WORKDIR_BYTES)) {
        failures.push(format!(
            "{path}: workdir maxLength must be the wire bound {}",
            wire::MAX_WORKDIR_BYTES
        ));
    }
    if workdir.get("pattern").and_then(Value::as_str) != Some(WORKDIR_PATTERN) {
        failures.push(format!("{path}: workdir pattern is not {WORKDIR_PATTERN}"));
    }
    if workdir.get("x-b10x-requires-capability") != Some(&json!(wire::EXEC_WORKDIR_FACT)) {
        failures.push(format!(
            "{path}: workdir does not require {}",
            wire::EXEC_WORKDIR_FACT
        ));
    }
    compare_tables(
        path,
        "workdir",
        &table(&wire::WORKDIR_REFUSALS),
        &declared(workdir.get("x-b10x-refusals"), path, failures),
        failures,
    );
    check_pattern_agrees(workdir, path, failures);
}

/// The pattern and the wire crate's lexical rule accept and refuse the same paths. The cases are
/// chosen so that no placed mount decides them: every lexically normal one is beneath
/// `/workspace`, or is `/` itself, which no placed mount contains.
fn check_pattern_agrees(workdir: &Value, path: &str, failures: &mut Vec<String>) {
    let Some(pattern) = workdir.get("pattern").and_then(Value::as_str) else {
        return;
    };
    let Ok(validator) = jsonschema::validator_for(&json!({ "type": "string", "pattern": pattern }))
    else {
        failures.push(format!("{path}: workdir pattern does not compile"));
        return;
    };
    for candidate in [
        "/",
        "/workspace",
        "/workspace/src",
        "/workspace/a b/c",
        "/workspace/..x",
        "/workspace/.hidden",
        "workspace",
        "workspace/src",
        "",
        "/workspace/",
        "//workspace",
        "/workspace//src",
        "/workspace/.",
        "/workspace/./src",
        "/workspace/..",
        "/workspace/../etc",
        "/workspace/a\u{0}b",
    ] {
        let schema_accepts = validator.is_valid(&json!(candidate));
        let wire_accepts = !matches!(
            wire::validate_workdir(&start_in(candidate)),
            Err(wire::WireValidationError::WorkdirInvalid)
        );
        if schema_accepts != wire_accepts {
            failures.push(format!(
                "{path}: workdir pattern {} {candidate:?}, which the wire crate {}",
                if schema_accepts { "accepts" } else { "refuses" },
                if wire_accepts { "accepts" } else { "refuses" },
            ));
        }
    }
}

/// The smallest exec start that names `workdir`.
fn start_in(workdir: &str) -> wire::ExecStartInput {
    serde_json::from_value(json!({
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
        "workdir": workdir
    }))
    .expect("the smallest exec start parses")
}

/// Both starts carry `workdir` as the shared definition and `apertures` with the wire bounds and
/// refusals.
fn check_starts(released: &Tree, failures: &mut Vec<String>) {
    let list_refusals = {
        let mut rows = table(&wire::APERTURE_LIST_REFUSALS);
        rows.insert((
            "exec.aperture-mode-mismatch".to_owned(),
            422,
            "refused".to_owned(),
        ));
        rows
    };
    for (path, exec) in STARTS {
        let Some(input) = json_at(released, path, failures) else {
            continue;
        };
        let Some(start) = input.pointer(exec) else {
            failures.push(format!("{path}: no exec start at {exec}"));
            continue;
        };
        if start.pointer("/properties/workdir/$ref")
            != Some(&json!(format!("../common.json#{WORKDIR_DEFINITION}")))
        {
            failures.push(format!("{path}: workdir is not the shared definition"));
        }
        let Some(apertures) = start.pointer("/properties/sandbox/properties/apertures") else {
            failures.push(format!("{path}: the sandbox has no apertures list"));
            continue;
        };
        for (member, expected) in [
            ("minItems", json!(2)),
            ("maxItems", max_apertures()),
            ("uniqueItems", json!(true)),
        ] {
            if apertures.get(member) != Some(&expected) {
                failures.push(format!("{path}: apertures {member} must be {expected}"));
            }
        }
        let single = start.pointer("/properties/sandbox/properties/aperture");
        if apertures.get("items") != single {
            failures.push(format!(
                "{path}: each name in apertures must be exactly what aperture admits"
            ));
        }
        compare_tables(
            path,
            "apertures",
            &list_refusals,
            &declared(apertures.get("x-b10x-refusals"), path, failures),
            failures,
        );
    }
}

fn check_fact(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/capability.json";
    let Some(capability) = json_at(released, path, failures) else {
        return;
    };
    let fact = capability
        .pointer("/properties/facts/properties")
        .and_then(|facts| facts.get(wire::EXEC_WORKDIR_FACT));
    if fact.and_then(|fact| fact.get("const")) != Some(&json!(true)) {
        failures.push(format!(
            "{path}: {} must be published as the constant true",
            wire::EXEC_WORKDIR_FACT
        ));
    }
    if fact
        .and_then(|fact| fact.get("x-b10x-absent-means"))
        .is_none()
    {
        failures.push(format!(
            "{path}: {} must say what its absence means",
            wire::EXEC_WORKDIR_FACT
        ));
    }
}

/// The applied network has a third form for a list, each element stating where it listened; every
/// applied record may name its working directory; and a run whose directory vanished after
/// admission can say so.
fn check_applied_record(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/resource.json";
    let Some(resource) = json_at(released, path, failures) else {
        return;
    };
    let forms = resource
        .pointer("/$defs/applied-network/oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let list = forms
        .iter()
        .find(|form| form.get("type") == Some(&json!("array")));
    match list {
        None => failures.push(format!("{path}: applied-network has no list form")),
        Some(list) => {
            if list.get("maxItems") != Some(&max_apertures())
                || list.get("minItems") != Some(&json!(2))
            {
                failures.push(format!(
                    "{path}: the applied list must hold 2 to {} apertures",
                    wire::MAX_EGRESS_APERTURES
                ));
            }
            let required = list
                .pointer("/items/required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !required.contains(&json!("listen")) {
                failures.push(format!(
                    "{path}: every applied aperture of a list must state listen"
                ));
            }
            let listen = format!(
                "^{}\\.[1-{}]:[0-9]{{1,5}}$",
                wire::APERTURE_LOOPBACK_ADDRESS
                    .rsplit_once('.')
                    .map_or("", |(network, _)| network)
                    .replace('.', "\\."),
                wire::MAX_EGRESS_APERTURES
            );
            if list.pointer("/items/properties/listen/pattern") != Some(&json!(listen)) {
                failures.push(format!("{path}: listen pattern is not {listen}"));
            }
        }
    }
    let single = forms
        .iter()
        .find(|form| form.get("type") == Some(&json!("object")));
    if single
        .and_then(|single| single.pointer("/properties/listen"))
        .is_some()
    {
        failures.push(format!(
            "{path}: the single applied aperture keeps its 0.20.0 shape, without listen"
        ));
    }
    let mut applied_shapes = 0;
    for (index, branch) in resource
        .pointer("/$defs/exec/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        for applied in branch
            .pointer("/properties/applied/oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            applied_shapes += 1;
            if applied.pointer("/properties/workdir/$ref")
                != Some(&json!(format!("common.json#{WORKDIR_DEFINITION}")))
            {
                failures.push(format!(
                    "{path}: exec branch {index} has an applied shape without workdir"
                ));
            }
            if applied.pointer("/properties/network/$ref")
                != Some(&json!("#/$defs/applied-network"))
            {
                failures.push(format!(
                    "{path}: exec branch {index} has an applied shape that is not every \
                     applied-network form"
                ));
            }
        }
        if branch
            .pointer("/properties/requested/properties/apertures")
            .is_none()
        {
            failures.push(format!(
                "{path}: exec branch {index} does not echo an apertures list"
            ));
        }
    }
    if applied_shapes == 0 {
        failures.push(format!("{path}: no exec branch has an applied shape"));
    }
    let missing = wire::WORKDIR_REFUSALS[2];
    let admits_missing = resource
        .pointer("/$defs/exec-refusal/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|form| {
            form.pointer("/properties/code/const") == Some(&json!(missing.code))
                && form.pointer("/properties/class/const") == Some(&json!(class_of(missing.class)))
        });
    if !admits_missing {
        failures.push(format!(
            "{path}: exec-refusal does not admit {} for a directory gone after admission",
            missing.code
        ));
    }
}

/// Every new refusal is answered by a vector with the wire's exact status, class and message, and
/// each success shape is shown by a vector whose result validates against the exec result schema.
fn check_vectors(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let validator = exec_result_validator(version, released, failures);
    let expected: BTreeMap<&str, &wire::ExecStartRefusal> = wire::WORKDIR_REFUSALS
        .iter()
        .chain(wire::APERTURE_LIST_REFUSALS.iter())
        .map(|refusal| (refusal.code, refusal))
        .collect();
    let mut answered: BTreeSet<&str> = BTreeSet::new();
    let (mut with_workdir, mut with_list) = (false, false);
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if vector.pointer("/action/operation") != Some(&json!("exec.start")) {
            continue;
        }
        let Some(response) = vector.pointer("/expected/response") else {
            continue;
        };
        if let Some(error) = response.pointer("/body/error") {
            let code = error.get("code").and_then(Value::as_str).unwrap_or("");
            let Some((code, refusal)) = expected.get_key_value(code) else {
                continue;
            };
            let observed = (
                response.get("status").and_then(Value::as_u64),
                error.get("class").and_then(Value::as_str),
                error.get("message").and_then(Value::as_str),
            );
            let class = class_of(refusal.class);
            if observed
                != (
                    Some(u64::from(refusal.status)),
                    Some(class.as_str()),
                    Some(refusal.message),
                )
            {
                failures.push(format!(
                    "{path}: {code} must answer {} {class} with the wire message",
                    refusal.status
                ));
                continue;
            }
            answered.insert(code);
        } else if let Some(result) = response.pointer("/body/result") {
            let names_workdir = result.pointer("/applied/workdir").is_some();
            let names_list = result
                .pointer("/applied/network")
                .is_some_and(Value::is_array);
            if !names_workdir && !names_list {
                continue;
            }
            with_workdir |= names_workdir;
            with_list |= names_list;
            if let Some(validator) = &validator
                && let Err(error) = validator.validate(result)
            {
                failures.push(format!(
                    "{path}: the exec.start result does not validate against its result schema: \
                     {error}"
                ));
            }
        }
    }
    for code in expected.keys() {
        if !answered.contains(code) {
            failures.push(format!("vectors: no exec.start vector answers {code}"));
        }
    }
    if !with_workdir {
        failures.push("vectors: no exec.start vector shows an applied workdir".to_owned());
    }
    if !with_list {
        failures.push("vectors: no exec.start vector shows an applied aperture list".to_owned());
    }
}

/// The exec result schema, compiled against the bundle's own schemas.
fn exec_result_validator(
    version: &str,
    released: &Tree,
    failures: &mut Vec<String>,
) -> Option<jsonschema::Validator> {
    let mut registry = jsonschema::Registry::new();
    for (path, bytes) in released {
        if !path.starts_with("schemas/") {
            continue;
        }
        let Ok(mut document) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if let Some(object) = document.as_object_mut() {
            object.remove("$id");
        }
        match registry.add(super::resource_uri(version, path), document) {
            Ok(next) => registry = next,
            Err(error) => {
                failures.push(format!("{path}: cannot register: {error}"));
                return None;
            }
        }
    }
    let registry = registry
        .prepare()
        .map_err(|error| failures.push(format!("bundle schemas do not form a registry: {error}")))
        .ok()?;
    let target = "schemas/results/exec-start.json";
    let reference = json!({ "$ref": super::resource_uri(version, target) });
    jsonschema::draft202012::options()
        .with_registry(&registry)
        .build(&reference)
        .map_err(|error| failures.push(format!("{target}: does not compile: {error}")))
        .ok()
}

fn check_client_visible_changes(released: &Tree, failures: &mut Vec<String>) {
    let path = "compatibility.json";
    let Some(compatibility) = json_at(released, path, failures) else {
        return;
    };
    let changes: Vec<&str> = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let named = [wire::EXEC_WORKDIR_FACT, "listen", "0.20.0"]
        .into_iter()
        .chain(
            wire::WORKDIR_REFUSALS
                .iter()
                .chain(wire::APERTURE_LIST_REFUSALS.iter())
                .map(|refusal| refusal.code),
        );
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::repo;

    fn released(version: &str) -> super::Tree {
        super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire")
                .join(version),
        )
        .expect("the bundle reads")
    }

    fn failures_of(tree: &super::Tree) -> Vec<String> {
        let mut failures = Vec::new();
        super::super::check_additions("0.21.0", tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released("0.21.0");
        let bytes = tree.get(path).expect("the document is in the bundle");
        let mut document: serde_json::Value = serde_json::from_slice(bytes).expect("parses");
        edit(&mut document);
        tree.insert(
            path.to_owned(),
            serde_json::to_vec(&document).expect("serializes"),
        );
        tree
    }

    fn assert_reported(failures: &[String], named: &str) {
        assert!(
            failures.iter().any(|failure| failure.contains(named)),
            "not reported: {named}: {failures:?}"
        );
    }

    #[test]
    fn the_successor_carries_the_working_directory_and_the_aperture_list() {
        let failures = failures_of(&released("0.21.0"));
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing about the tree that
    /// carries it.
    #[test]
    fn the_predecessor_carries_none_of_it() {
        let failures = failures_of(&released("0.20.0"));
        for named in [
            "schemas/common.json: no workdir definition",
            "schemas/inputs/exec-start.json: workdir is not the shared definition",
            "schemas/inputs/pipe-session-start.json: the sandbox has no apertures list",
            "exec.workdir must be published as the constant true",
            "applied-network has no list form",
            "exec-refusal does not admit exec.workdir-missing",
            "no exec.start vector answers exec.workdir-unserved",
            "no exec.start vector answers exec.aperture-listed-twice",
            "no exec.start vector shows an applied aperture list",
            "client_visible_changes must state exec.workdir",
        ] {
            assert_reported(&failures, named);
        }
    }

    /// Each bound is the wire constant: one moved in the bundle alone is reported by name.
    #[test]
    fn a_bound_that_is_not_the_wire_constant_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["workdir"]["maxLength"] = 4095.into();
        });
        assert_reported(
            &failures_of(&tree),
            "workdir maxLength must be the wire bound 4096",
        );
        let tree = edited("schemas/inputs/exec-start.json", |document| {
            document["properties"]["sandbox"]["properties"]["apertures"]["maxItems"] = 5.into();
        });
        assert_reported(&failures_of(&tree), "apertures maxItems must be 4");
    }

    /// A pattern that admits what the wire crate refuses is reported with the path it disagrees
    /// on.
    #[test]
    fn a_pattern_looser_than_the_wire_rule_is_reported() {
        let tree = edited("schemas/common.json", |document| {
            document["$defs"]["workdir"]["pattern"] = "^/.*$".into();
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "workdir pattern is not");
        assert_reported(
            &failures,
            "workdir pattern accepts \"/workspace/../etc\", which the wire crate refuses",
        );
    }

    /// The refusal declarations are exactly the wire table, status and class included.
    #[test]
    fn refusal_declarations_are_exactly_the_wire_table() {
        let tree = edited("schemas/common.json", |document| {
            let refusals = document["$defs"]["workdir"]["x-b10x-refusals"]
                .as_array_mut()
                .expect("array");
            refusals.retain(|refusal| refusal["code"] != "exec.workdir-unserved");
            refusals.push(serde_json::json!({
                "class": "refused",
                "code": "exec.workdir-unserved",
                "status": 422
            }));
        });
        let failures = failures_of(&tree);
        assert_reported(
            &failures,
            "workdir does not declare exec.workdir-unserved (501, unserved)",
        );
        assert_reported(
            &failures,
            "workdir declares exec.workdir-unserved (422, refused)",
        );
    }

    /// Every refusal is answered by a vector with the wire's status, class and message.
    #[test]
    fn a_refusal_no_vector_answers_or_answers_wrongly_is_reported() {
        let mut tree = released("0.21.0");
        tree.remove("vectors/http/apertures-list-of-five-is-refused.json");
        assert_reported(
            &failures_of(&tree),
            "no exec.start vector answers exec.apertures-too-many",
        );
        let tree = edited("vectors/http/workdir-absent-is-refused.json", |document| {
            document["expected"]["response"]["body"]["error"]["message"] =
                "The directory /workspace/absent does not exist.".into();
        });
        assert_reported(
            &failures_of(&tree),
            "exec.workdir-missing must answer 422 refused with the wire message",
        );
    }

    /// A success result is held to the exec result schema: an applied list without listen does
    /// not validate.
    #[test]
    fn a_vector_result_outside_its_schema_is_reported() {
        let tree = edited(
            "vectors/http/apertures-applied-are-observed.json",
            |document| {
                let network =
                    &mut document["expected"]["response"]["body"]["result"]["applied"]["network"];
                network[1]
                    .as_object_mut()
                    .expect("an applied aperture")
                    .remove("listen");
            },
        );
        assert_reported(
            &failures_of(&tree),
            "the exec.start result does not validate against its result schema",
        );
    }
}
