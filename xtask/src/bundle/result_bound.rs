//! What `0.28.0` exists for (ADR 0053 decision 11): the bound on a success result the operation
//! ledger records is published, and the five kinds that decide against it say so.
//!
//! The bundle adds no route. It publishes the fact `operation.result-max-json-bytes` at
//! `OPERATION_RESULT_MAX_JSON_BYTES`, the refusal `operation.result-limit` on the inputs of the
//! five decided kinds with one vector each against a machine that states a lower value, the
//! error `operation.result-unrecorded` with one vector of a completed row, a coverage
//! requirement for the bound, and a `maxLength` on every result member a request does not set.
//!
//! The bundle states every value as a literal, because `render_v2.rs` is hashed by every bundle
//! it rendered and cannot learn a new `{"$wire": …}` binding (AGENTS.md § *The gate*). This
//! module binds each literal to the wire crate, which no bundle hashes. Where the wire crate
//! holds no constant (the class, status, address and message of the two codes, which ADR 0053
//! decisions 2 and 8 state), the ADR's value is named here once.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const CAPABILITY: &str = "schemas/capability.json";
const COMMON: &str = "schemas/common.json";
const RESOURCE: &str = "schemas/resource.json";
const VECTOR: &str = "schemas/vector.json";
const COVERAGE: &str = "coverage.json";
const PROBE: &str = "vectors/http/machine-probe.json";

/// The fact that publishes the bound (ADR 0053 decision 4).
pub(super) const FACT: &str = "operation.result-max-json-bytes";
/// The fact the bound stands beside in every machine shape and every snapshot.
const BESIDE: &str = "operation.ledger-subject-max-rows";
/// The coverage requirement of the bound, beside `security.bounds.input`.
pub(super) const REQUIREMENT: &str = "security.bounds.result";

/// The first bundle that publishes the bound; every later one keeps it.
const FIRST: (u64, u64, u64) = (0, 28, 0);
/// The routes of `0.27.0`, which `0.28.0` preserves and adds none to.
const ROUTES_OF_0_27_0: u64 = 45;

/// One code as ADR 0053 publishes it.
#[derive(Clone, Copy, Debug)]
struct PublishedError {
    code: &'static str,
    class: wire::ErrorClass,
    status: u16,
    address: &'static str,
    message: &'static str,
}

/// ADR 0053 decision 2.
const RESULT_LIMIT: PublishedError = PublishedError {
    code: wire::OPERATION_RESULT_LIMIT,
    class: wire::ErrorClass::Exhausted,
    status: 429,
    address: "result",
    message: "The result of this operation exceeds what the operation ledger can record.",
};

/// ADR 0053 decision 8.
const RESULT_UNRECORDED: PublishedError = PublishedError {
    code: wire::OPERATION_RESULT_UNRECORDED,
    class: wire::ErrorClass::Failed,
    status: 500,
    address: "result",
    message: "The operation took effect, but its result exceeds what the operation ledger can \
              record. Read the resource.",
};

/// The five kinds decided against the bound (ADR 0053 decision 10), and each one's input.
const DECIDED: [(&str, &str); 5] = [
    ("exec.start", "schemas/inputs/exec-start.json"),
    ("session.start", "schemas/inputs/pipe-session-start.json"),
    ("process.start", "schemas/inputs/process-start.json"),
    (
        "workspace.file.write",
        "schemas/inputs/workspace-file-write.json",
    ),
    (
        "workspace.file.delete",
        "schemas/inputs/workspace-file-delete.json",
    ),
];

/// The identifiers the daemon mints, each published at [`wire::MAX_RESOURCE_ID_CHARS`].
const MINTED: [&str; 5] = [
    "workspace-id",
    "exec-id",
    "session-id",
    "snapshot-id",
    "process-id",
];

/// Every result member with a published length the bundle holds to a wire constant (ADR 0053
/// decisions 7 and 11): the file, the member, its bound and the constant's name.
fn member_bounds() -> Vec<(&'static str, String, usize, &'static str)> {
    let mut bounds: Vec<(&'static str, String, usize, &'static str)> = MINTED
        .iter()
        .map(|name| {
            (
                COMMON,
                format!("/$defs/{name}/maxLength"),
                wire::MAX_RESOURCE_ID_CHARS,
                "MAX_RESOURCE_ID_CHARS",
            )
        })
        .collect();
    bounds.push((
        COMMON,
        "/$defs/host-absolute-path/maxLength".to_owned(),
        wire::MAX_READ_ONLY_FILE_PATH_BYTES,
        "MAX_READ_ONLY_FILE_PATH_BYTES",
    ));
    for alternative in 0..2 {
        bounds.push((
            RESOURCE,
            format!("/$defs/exec-refusal/oneOf/{alternative}/properties/code/maxLength"),
            wire::MAX_EXEC_REFUSAL_CODE_CHARS,
            "MAX_EXEC_REFUSAL_CODE_CHARS",
        ));
        bounds.push((
            RESOURCE,
            format!("/$defs/exec-refusal/oneOf/{alternative}/properties/message/maxLength"),
            wire::MAX_EXEC_REFUSAL_MESSAGE_CHARS,
            "MAX_EXEC_REFUSAL_MESSAGE_CHARS",
        ));
        bounds.push((
            RESOURCE,
            format!("/$defs/confinement-applied/oneOf/{alternative}/properties/cgroup/maxLength"),
            wire::MAX_APPLIED_CGROUP_CHARS,
            "MAX_APPLIED_CGROUP_CHARS",
        ));
    }
    for aperture in [
        "/$defs/applied-network/oneOf/1/properties",
        "/$defs/applied-network/oneOf/2/items/properties",
    ] {
        bounds.push((
            RESOURCE,
            format!("{aperture}/name/maxLength"),
            wire::MAX_APERTURE_NAME_CHARS,
            "MAX_APERTURE_NAME_CHARS",
        ));
        bounds.push((
            RESOURCE,
            format!("{aperture}/destination/maxLength"),
            wire::MAX_APERTURE_DESTINATION_CHARS,
            "MAX_APERTURE_DESTINATION_CHARS",
        ));
    }
    for (pointer, bound, constant) in [
        (
            "/$defs/applied-network/oneOf/2/items/properties/listen/maxLength",
            wire::MAX_APERTURE_LISTEN_CHARS,
            "MAX_APERTURE_LISTEN_CHARS",
        ),
        (
            "/$defs/exec-usage/oneOf/2/properties/code/maxLength",
            wire::MAX_EXEC_USAGE_CODE_CHARS,
            "MAX_EXEC_USAGE_CODE_CHARS",
        ),
        (
            "/$defs/exec-usage/oneOf/2/properties/message/maxLength",
            wire::MAX_EXEC_USAGE_MESSAGE_CHARS,
            "MAX_EXEC_USAGE_MESSAGE_CHARS",
        ),
        (
            "/$defs/process-exit/properties/signal/maxLength",
            wire::MAX_PROCESS_EXIT_SIGNAL_CHARS,
            "MAX_PROCESS_EXIT_SIGNAL_CHARS",
        ),
        (
            "/$defs/process/properties/argv/items/maxLength",
            wire::MAX_PROCESS_ARGV_BYTES,
            "MAX_PROCESS_ARGV_BYTES",
        ),
        (
            "/$defs/process/properties/cwd/maxLength",
            wire::MAX_PROCESS_CWD_BYTES,
            "MAX_PROCESS_CWD_BYTES",
        ),
        (
            "/$defs/process/properties/env_names/items/maxLength",
            wire::MAX_PROCESS_ENV_BYTES,
            "MAX_PROCESS_ENV_BYTES",
        ),
    ] {
        bounds.push((RESOURCE, pointer.to_owned(), bound, constant));
    }
    bounds
}

pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => (major, minor, patch) >= FIRST,
        _ => false,
    }
}

/// The class check, for `0.28.0` and every later bundle.
pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_fact(released, failures);
    check_vector_schema(released, failures);
    check_machines(released, failures);
    check_snapshot(version, released, failures);
    check_input_refusals(released, failures);
    check_refusal_vectors(released, failures);
    check_unrecorded_vector(released, failures);
    check_member_bounds(released, failures);
    check_coverage(released, failures);
}

/// What only `0.28.0` states: its predecessor, its route counts and its client-visible changes.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
    if let Some(bundle) = json_at(released, "bundle.json", failures) {
        let routes = json_at(released, "operations.json", failures)
            .and_then(|operations| {
                operations
                    .get("operations")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or_default() as u64;
        for (member, expected) in [
            ("predecessor", json!("0.27.0")),
            ("kind", json!("additive-v1")),
            ("adds_routes", json!(0)),
            ("preserves_routes", json!(ROUTES_OF_0_27_0)),
        ] {
            let stated = bundle.pointer(&format!("/compatibility/{member}"));
            if stated != Some(&expected) {
                failures.push(format!(
                    "bundle.json: compatibility.{member} is {}, and 0.28.0 states {expected}",
                    stated.unwrap_or(&Value::Null)
                ));
            }
        }
        if routes != ROUTES_OF_0_27_0 {
            failures.push(format!(
                "operations.json: {routes} routes, and 0.28.0 serves the {ROUTES_OF_0_27_0} of 0.27.0"
            ));
        }
    }
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
    let bound = wire::OPERATION_RESULT_MAX_JSON_BYTES.to_string();
    let routes = format!("the {ROUTES_OF_0_27_0} routes of 0.27.0");
    let limit_status = format!("{} ({})", RESULT_LIMIT.code, RESULT_LIMIT.status);
    let unrecorded_status = format!("{} ({})", RESULT_UNRECORDED.code, RESULT_UNRECORDED.status);
    let named = [
        "ADR 0053",
        routes.as_str(),
        FACT,
        bound.as_str(),
        limit_status.as_str(),
        unrecorded_status.as_str(),
    ];
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

/// Calls `each` with every object in `value` and where it is, depth first.
fn visit<'a>(value: &'a Value, path: &str, each: &mut impl FnMut(&str, &'a Value)) {
    match value {
        Value::Object(object) => {
            each(path, value);
            for (key, child) in object {
                visit(child, &format!("{path}/{key}"), each);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                visit(child, &format!("{path}/{index}"), each);
            }
        }
        _ => {}
    }
}

fn the_bound() -> Value {
    json!(wire::OPERATION_RESULT_MAX_JSON_BYTES)
}

/// The fact is required, is the wire constant, and its description states the counting rule of
/// decision 5.
fn check_fact(released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let Some(fact) = capability.pointer(&format!("/properties/facts/properties/{FACT}")) else {
        failures.push(format!("{CAPABILITY}: {FACT} is not declared"));
        return;
    };
    if fact.get("const") != Some(&the_bound()) {
        failures.push(format!(
            "{CAPABILITY}: {FACT} const is {}, and substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES \
             is {}",
            fact.get("const").unwrap_or(&Value::Null),
            wire::OPERATION_RESULT_MAX_JSON_BYTES
        ));
    }
    let required = capability
        .pointer("/properties/facts/required")
        .and_then(Value::as_array);
    if !required.is_some_and(|required| required.contains(&json!(FACT))) {
        failures.push(format!("{CAPABILITY}: {FACT} is not a required fact"));
    }
    let description = fact
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for rule in [
        RESULT_LIMIT.code,
        "as sent",
        "6 bytes for each character of its maxLength",
        "longest decimal form",
        "every optional member as present",
        "never refused",
    ] {
        if !description.contains(rule) {
            failures.push(format!(
                "{CAPABILITY}: {FACT} description does not state the counting rule of ADR 0053 \
                 decision 5: {rule:?}"
            ));
        }
    }
}

/// Every machine shape of the vector schema that states the ledger's bounds states this one, as
/// an integer, and requires it, so a vector's machine may state a lower value.
fn check_vector_schema(released: &Tree, failures: &mut Vec<String>) {
    let Some(vector) = json_at(released, VECTOR, failures) else {
        return;
    };
    let mut shapes = 0;
    visit(&vector, "", &mut |path, value| {
        let Some(properties) = value.get("properties") else {
            return;
        };
        if properties.get(BESIDE).is_none() {
            return;
        }
        shapes += 1;
        if properties.pointer(&format!("/{FACT}/type")) != Some(&json!("integer")) {
            failures.push(format!(
                "{VECTOR}: the machine facts at {path} do not state {FACT} as an integer"
            ));
        }
        let required = value.get("required").and_then(Value::as_array);
        if !required.is_some_and(|required| required.contains(&json!(FACT))) {
            failures.push(format!(
                "{VECTOR}: the machine facts at {path} do not require {FACT}"
            ));
        }
    });
    if shapes == 0 {
        failures.push(format!("{VECTOR}: no machine shape states {BESIDE}"));
    }
}

/// The vectors that publish `operation.result-limit`.
fn is_refusal_vector(vector: &Value) -> bool {
    vector.pointer("/expected/response/body/error/code") == Some(&json!(RESULT_LIMIT.code))
}

/// Every vector's machine states the fact: at the bound, or, in a vector of the refusal, lower.
fn check_machines(released: &Tree, failures: &mut Vec<String>) {
    let mut machines = 0;
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let refusal = is_refusal_vector(&vector);
        for machine in vector
            .get("setup")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|setup| setup.get("kind") == Some(&json!("machine")))
        {
            machines += 1;
            let stated = machine.pointer(&format!("/state/facts/{FACT}"));
            match stated.and_then(Value::as_u64) {
                None => failures.push(format!("{path}: a machine does not state {FACT}")),
                Some(value) if refusal => {
                    if value == 0 || value >= wire::OPERATION_RESULT_MAX_JSON_BYTES {
                        failures.push(format!(
                            "{path}: the machine of a {} vector states {FACT} {value}, not a value \
                             lower than substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES",
                            RESULT_LIMIT.code
                        ));
                    }
                }
                Some(value) => {
                    if value != wire::OPERATION_RESULT_MAX_JSON_BYTES {
                        failures.push(format!(
                            "{path}: a machine states {FACT} {value}, not \
                             substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES ({})",
                            wire::OPERATION_RESULT_MAX_JSON_BYTES
                        ));
                    }
                }
            }
        }
    }
    if machines == 0 {
        failures.push("vectors: no vector sets up a machine".to_owned());
    }
    if let Some(probe) = json_at(released, PROBE, failures)
        && probe.pointer(&format!("/expected/response/body/result/facts/{FACT}"))
            != Some(&the_bound())
    {
        failures.push(format!(
            "{PROBE}: the probed snapshot does not state {FACT} {}",
            wire::OPERATION_RESULT_MAX_JSON_BYTES
        ));
    }
}

/// The probed snapshot validates against the capability schema; with the fact absent or at
/// another value it does not.
fn check_snapshot(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let Some(probe) = json_at(released, PROBE, failures) else {
        return;
    };
    let Some(snapshot) = probe.pointer("/expected/response/body/result") else {
        failures.push(format!("{PROBE}: no capability snapshot result"));
        return;
    };
    let Some(schema) = super::read_only_roots::validator(version, released, CAPABILITY, failures)
    else {
        return;
    };
    if let Err(error) = schema.validate(snapshot) {
        failures.push(format!(
            "{CAPABILITY}: rejects the probed snapshot: {error} at {}",
            error.instance_path()
        ));
    }
    let mut without = snapshot.clone();
    if let Some(facts) = without.get_mut("facts").and_then(Value::as_object_mut) {
        facts.remove(FACT);
    }
    if schema.is_valid(&without) {
        failures.push(format!(
            "{CAPABILITY}: accepts a snapshot that does not state {FACT}"
        ));
    }
    let mut lower = snapshot.clone();
    lower["facts"][FACT] = json!(wire::OPERATION_RESULT_MAX_JSON_BYTES - 1);
    if schema.is_valid(&lower) {
        failures.push(format!(
            "{CAPABILITY}: accepts a snapshot stating {FACT} other than \
             substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES"
        ));
    }
}

fn refusal_entry() -> Value {
    json!({
        "class": RESULT_LIMIT.class,
        "code": RESULT_LIMIT.code,
        "status": RESULT_LIMIT.status,
    })
}

/// The refusal is named in `refusals` in the list's own form: a row of class, code and status,
/// or the code alone where every entry of the list is a code (the session start's own list).
fn names_the_refusal(refusals: &[Value]) -> bool {
    if refusals.iter().all(Value::is_string) {
        refusals.contains(&json!(RESULT_LIMIT.code))
    } else {
        refusals.contains(&refusal_entry())
    }
}

/// The input of each decided kind names the refusal in its own `x-b10x-refusals`; no other input
/// does (decision 9: a mutation of an existing resource is never refused for this cause).
fn check_input_refusals(released: &Tree, failures: &mut Vec<String>) {
    let entry = refusal_entry();
    for (kind, path) in DECIDED {
        let Some(input) = json_at(released, path, failures) else {
            continue;
        };
        let listed = input
            .get("x-b10x-refusals")
            .and_then(Value::as_array)
            .is_some_and(|refusals| names_the_refusal(refusals));
        if !listed {
            failures.push(format!(
                "{path}: the {kind} input does not name {entry} in its x-b10x-refusals"
            ));
        }
    }
    let decided: BTreeSet<&str> = DECIDED.iter().map(|(_, path)| *path).collect();
    for (path, bytes) in released {
        if !path.starts_with("schemas/inputs/") || decided.contains(path.as_str()) {
            continue;
        }
        if String::from_utf8_lossy(bytes).contains(RESULT_LIMIT.code) {
            failures.push(format!(
                "{path}: names {}, which no input but the five decided kinds' raises",
                RESULT_LIMIT.code
            ));
        }
    }
}

/// The widest result of the request `vector` makes, by the wire crate's width function for its
/// kind (ADR 0053 decision 6).
fn widest_result(kind: &str, vector: &Value) -> Result<usize, String> {
    let input = vector
        .pointer("/action/request/body/input")
        .cloned()
        .unwrap_or(Value::Null);
    let parsed = |error: serde_json::Error| format!("the input does not parse: {error}");
    match kind {
        "exec.start" => serde_json::from_value::<wire::ExecStartInput>(input)
            .map(|input| wire::Exec::widest_json_bytes(&input))
            .map_err(parsed),
        "session.start" => serde_json::from_value::<wire::PipeSessionStartInput>(input)
            .map(|input| wire::Exec::widest_json_bytes(&input.exec))
            .map_err(parsed),
        "process.start" => {
            let strings = |member: &str| -> Vec<String> {
                input
                    .get(member)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            };
            let text = |member: &str| {
                input
                    .get(member)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let env_names: Vec<String> = input
                .get("env")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(name, _)| name.clone())
                .collect();
            let owner = serde_json::from_value::<wire::ProcessOwner>(
                input.get("owner").cloned().unwrap_or(Value::Null),
            )
            .map_err(parsed)?;
            Ok(wire::ProcessRecord::widest_json_bytes(
                &strings("argv"),
                &text("cwd"),
                &env_names,
                &owner,
                &text("reason"),
            ))
        }
        "workspace.file.write" | "workspace.file.delete" => {
            let path = vector
                .pointer("/action/request/path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some((workspace, file)) = path
                .strip_prefix("/v1/workspaces/")
                .and_then(|rest| rest.split_once("/files/"))
            else {
                return Err(format!("{path} is not a v1 file address"));
            };
            Ok(if kind == "workspace.file.write" {
                wire::FileObservation::widest_json_bytes(workspace, file)
            } else {
                wire::FileAbsence::widest_json_bytes(workspace, file)
            })
        }
        other => Err(format!("{other} is not a decided kind")),
    }
}

/// `error` is the published form of `code`.
fn check_error(
    path: &str,
    error: Option<&Value>,
    code: PublishedError,
    failures: &mut Vec<String>,
) {
    let class = serde_json::to_value(code.class).unwrap_or(Value::Null);
    for (member, expected) in [
        ("code", json!(code.code)),
        ("class", class),
        ("address", json!(code.address)),
        ("message", json!(code.message)),
        ("retriable", json!(false)),
    ] {
        let stated = error.and_then(|error| error.get(member));
        if stated != Some(&expected) {
            failures.push(format!(
                "{path}: {} {member} is {}, and ADR 0053 publishes {expected}",
                code.code,
                stated.unwrap_or(&Value::Null)
            ));
        }
    }
}

fn postcondition(vector: &Value, actual: &str) -> Option<Value> {
    vector
        .get("postconditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|condition| condition.get("actual") == Some(&json!(actual)))
        .and_then(|condition| condition.get("expected").cloned())
}

/// One vector of the refusal per decided kind: the published code, class, status, address and
/// message, recorded as refused with no driver dispatched, against a machine whose lower value
/// the request's widest result exceeds while it stays within the published bound. No vector of
/// another kind publishes the refusal.
fn check_refusal_vectors(released: &Tree, failures: &mut Vec<String>) {
    let mut published: BTreeSet<&str> = BTreeSet::new();
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if !is_refusal_vector(&vector) {
            continue;
        }
        let kind = vector
            .pointer("/action/operation")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some((kind, _)) = DECIDED.iter().find(|(decided, _)| *decided == kind) else {
            failures.push(format!(
                "{path}: {kind} publishes {}, which only the five decided kinds raise",
                RESULT_LIMIT.code
            ));
            continue;
        };
        published.insert(kind);
        let error = vector.pointer("/expected/response/body/error");
        check_error(path, error, RESULT_LIMIT, failures);
        let operation = vector.pointer("/action/request/body/op");
        if error.and_then(|error| error.get("operation")) != operation {
            failures.push(format!(
                "{path}: the refusal does not name the request's operation"
            ));
        }
        let status = vector.pointer("/expected/response/status");
        if status != Some(&json!(RESULT_LIMIT.status)) {
            failures.push(format!(
                "{path}: {} answers {}, and ADR 0053 publishes {}",
                RESULT_LIMIT.code,
                status.unwrap_or(&Value::Null),
                RESULT_LIMIT.status
            ));
        }
        for (actual, expected) in [
            ("/driver/dispatch_count", json!(0)),
            ("/probes/operation_state", json!("refused")),
        ] {
            if postcondition(&vector, actual) != Some(expected.clone()) {
                failures.push(format!(
                    "{path}: the refusal does not hold {actual} at {expected}"
                ));
            }
        }
        let lowered = vector
            .get("setup")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|setup| setup.pointer(&format!("/state/facts/{FACT}")))
            .and_then(Value::as_u64);
        let Some(lowered) = lowered else {
            failures.push(format!("{path}: no machine states the lower {FACT}"));
            continue;
        };
        match widest_result(kind, &vector) {
            Err(error) => failures.push(format!("{path}: {error}")),
            Ok(widest) => {
                let widest = widest as u64;
                if widest <= lowered {
                    failures.push(format!(
                        "{path}: the widest result of the request is {widest} bytes, within the \
                         machine's {FACT} {lowered}, so nothing is refused"
                    ));
                }
                if widest > wire::OPERATION_RESULT_MAX_JSON_BYTES {
                    failures.push(format!(
                        "{path}: the widest result of the request is {widest} bytes, over \
                         substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES, so the refusal does not \
                         show the machine's lower {FACT}"
                    ));
                }
            }
        }
    }
    for (kind, _) in DECIDED {
        if !published.contains(kind) {
            failures.push(format!(
                "vectors: no {kind} vector publishes {}",
                RESULT_LIMIT.code
            ));
        }
    }
}

/// One vector reads a completed row that ended with `operation.result-unrecorded`: terminal, an
/// error outcome in the published form, the resource named, and the row the setup stored.
fn check_unrecorded_vector(released: &Tree, failures: &mut Vec<String>) {
    let mut found = 0;
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let record = vector.pointer("/expected/response/body/result");
        let error = record.and_then(|record| record.pointer("/outcome/error"));
        if error.and_then(|error| error.get("code")) != Some(&json!(RESULT_UNRECORDED.code)) {
            if String::from_utf8_lossy(bytes).contains(RESULT_UNRECORDED.code) {
                failures.push(format!(
                    "{path}: names {} outside the outcome of a completed row",
                    RESULT_UNRECORDED.code
                ));
            }
            continue;
        }
        found += 1;
        check_error(path, error, RESULT_UNRECORDED, failures);
        if vector.pointer("/action/operation") != Some(&json!("operation.get")) {
            failures.push(format!(
                "{path}: the completed row is not read by operation.get"
            ));
        }
        for (pointer, expected) in [
            ("/state", json!("terminal")),
            ("/outcome/kind", json!("error")),
        ] {
            if record.and_then(|record| record.pointer(pointer)) != Some(&expected) {
                failures.push(format!("{path}: the row's {pointer} is not {expected}"));
            }
        }
        if record
            .and_then(|record| record.get("resource"))
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            failures.push(format!("{path}: the row does not name its resource"));
        }
        let stored = vector
            .get("setup")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|setup| setup.pointer("/state/record"));
        if stored != record {
            failures.push(format!(
                "{path}: the row read is not the row the setup stored"
            ));
        }
    }
    if found != 1 {
        failures.push(format!(
            "vectors: {found} vectors read a row ended with {}, and the bundle publishes one",
            RESULT_UNRECORDED.code
        ));
    }
}

/// Each published length of decisions 7 and 11 is its wire constant, and every lease states the
/// actor and principal bounds.
fn check_member_bounds(released: &Tree, failures: &mut Vec<String>) {
    let common = json_at(released, COMMON, failures);
    let resource = json_at(released, RESOURCE, failures);
    for (file, pointer, bound, constant) in member_bounds() {
        let document = if file == COMMON { &common } else { &resource };
        let Some(document) = document else {
            continue;
        };
        let stated = document.pointer(&pointer);
        if stated != Some(&json!(bound)) {
            failures.push(format!(
                "{file}: {pointer} is {}, and substrate_wire::{constant} is {bound}",
                stated.unwrap_or(&Value::Null)
            ));
        }
    }
    let Some(resource) = resource else {
        return;
    };
    let mut leases = 0;
    visit(&resource, "", &mut |path, value| {
        let Some(properties) = value.get("properties") else {
            return;
        };
        if properties.get("actor").is_none() {
            return;
        }
        leases += 1;
        for (member, bound, constant) in [
            (
                "actor",
                wire::MAX_LEASE_ACTOR_CHARS,
                "MAX_LEASE_ACTOR_CHARS",
            ),
            (
                "principal",
                wire::MAX_LEASE_PRINCIPAL_CHARS,
                "MAX_LEASE_PRINCIPAL_CHARS",
            ),
        ] {
            let stated = properties.pointer(&format!("/{member}/maxLength"));
            if stated != Some(&json!(bound)) {
                failures.push(format!(
                    "{RESOURCE}: {path}/properties/{member}/maxLength is {}, and \
                     substrate_wire::{constant} is {bound}",
                    stated.unwrap_or(&Value::Null)
                ));
            }
        }
    });
    if leases == 0 {
        failures.push(format!("{RESOURCE}: no lease states an actor"));
    }
}

/// The bound has its coverage requirement, and every vector of the refusal is its evidence.
fn check_coverage(released: &Tree, failures: &mut Vec<String>) {
    let Some(coverage) = json_at(released, COVERAGE, failures) else {
        return;
    };
    let requirement = coverage
        .get("requirements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|requirement| requirement.get("id") == Some(&json!(REQUIREMENT)));
    let Some(requirement) = requirement else {
        failures.push(format!("{COVERAGE}: no requirement {REQUIREMENT}"));
        return;
    };
    if !requirement
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source.contains("ADR 0053"))
    {
        failures.push(format!("{COVERAGE}: {REQUIREMENT} does not cite ADR 0053"));
    }
    let evidence: BTreeSet<&str> = requirement
        .get("evidence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|evidence| evidence.get("id").and_then(Value::as_str))
        .collect();
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        if !is_refusal_vector(&vector) {
            continue;
        }
        let id = vector.get("id").and_then(Value::as_str).unwrap_or_default();
        if !evidence.contains(id) {
            failures.push(format!(
                "{COVERAGE}: {REQUIREMENT} does not list the vector {id} ({path})"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use substrate_wire as wire;

    use crate::repo;

    const VERSION: &str = "0.28.0";
    const EXEC: &str = "vectors/http/exec-start-result-over-the-ledger-bound-is-refused.json";
    const PROCESS: &str = "vectors/http/process-start-result-over-the-ledger-bound-is-refused.json";
    const FILE_DELETE: &str =
        "vectors/http/file-delete-result-over-the-ledger-bound-is-refused.json";
    const UNRECORDED: &str = "vectors/http/operation-result-unrecorded-names-its-resource.json";

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
        super::check(VERSION, tree, &mut failures);
        super::check_introduction(tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released(VERSION);
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
            "expected a failure naming {named:?}, got {failures:#?}"
        );
    }

    #[test]
    fn the_class_check_starts_at_0_28_0() {
        assert!(super::applies_to("0.28.0"));
        assert!(super::applies_to("0.29.0"));
        assert!(super::applies_to("1.0.0"));
        assert!(!super::applies_to("0.27.0"));
        assert!(!super::applies_to("0.5.0"));
        assert!(!super::applies_to("0.28"));
    }

    #[test]
    fn the_successor_publishes_the_result_bound() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// `0.27.0` publishes none of it, so every check reports.
    #[test]
    fn the_predecessor_publishes_no_result_bound() {
        let mut failures = Vec::new();
        super::check("0.27.0", &released("0.27.0"), &mut failures);
        for named in [
            "operation.result-max-json-bytes is not declared",
            "do not require operation.result-max-json-bytes",
            "a machine does not state operation.result-max-json-bytes",
            "does not name",
            "no exec.start vector publishes",
            "0 vectors read a row ended",
            "MAX_RESOURCE_ID_CHARS",
            "MAX_EXEC_REFUSAL_CODE_CHARS",
            "MAX_PROCESS_ARGV_BYTES",
            "MAX_PROCESS_CWD_BYTES",
            "MAX_PROCESS_ENV_BYTES",
            "no requirement security.bounds.result",
        ] {
            assert_reported(&failures, named);
        }
    }

    #[test]
    fn a_fact_that_is_not_the_wire_constant_is_reported() {
        let tree = edited("schemas/capability.json", |capability| {
            capability["properties"]["facts"]["properties"]["operation.result-max-json-bytes"]["const"] =
                json!(wire::OPERATION_RESULT_MAX_JSON_BYTES + 1);
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "OPERATION_RESULT_MAX_JSON_BYTES is 524217");
        assert_reported(&failures, "rejects the probed snapshot");
    }

    #[test]
    fn an_optional_fact_is_reported() {
        let tree = edited("schemas/capability.json", |capability| {
            let required = capability["properties"]["facts"]["required"]
                .as_array_mut()
                .expect("required facts");
            required.retain(|fact| fact != "operation.result-max-json-bytes");
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "is not a required fact");
        assert_reported(&failures, "accepts a snapshot that does not state");
    }

    #[test]
    fn a_description_without_the_counting_rule_is_reported() {
        let tree = edited("schemas/capability.json", |capability| {
            capability["properties"]["facts"]["properties"]["operation.result-max-json-bytes"]["description"] =
                json!("The result bound.");
        });
        assert_reported(&failures_of(&tree), "counting rule of ADR 0053 decision 5");
    }

    #[test]
    fn a_machine_shape_that_does_not_require_the_fact_is_reported() {
        let tree = edited("schemas/vector.json", |vector| {
            let required = vector["properties"]["setup"]["items"]["oneOf"][0]["properties"]["state"]
                ["properties"]["facts"]["required"]
                .as_array_mut()
                .expect("required facts");
            required.retain(|fact| fact != "operation.result-max-json-bytes");
        });
        assert_reported(
            &failures_of(&tree),
            "do not require operation.result-max-json-bytes",
        );
    }

    #[test]
    fn a_machine_at_another_value_is_reported() {
        let tree = edited("vectors/http/write-limit.json", |vector| {
            vector["setup"][0]["state"]["facts"]["operation.result-max-json-bytes"] = json!(64);
        });
        assert_reported(
            &failures_of(&tree),
            "states operation.result-max-json-bytes 64, not",
        );
        let tree = edited(EXEC, |vector| {
            vector["setup"][0]["state"]["facts"]["operation.result-max-json-bytes"] =
                json!(wire::OPERATION_RESULT_MAX_JSON_BYTES);
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "not a value lower than");
        let tree = edited(PROBE_PATH, |vector| {
            vector["expected"]["response"]["body"]["result"]["facts"]["operation.result-max-json-bytes"] =
                json!(1);
        });
        assert_reported(&failures_of(&tree), "the probed snapshot does not state");
    }

    const PROBE_PATH: &str = super::PROBE;

    #[test]
    fn an_input_that_does_not_name_the_refusal_is_reported() {
        let tree = edited("schemas/inputs/workspace-file-delete.json", |input| {
            input
                .as_object_mut()
                .expect("an object")
                .remove("x-b10x-refusals");
        });
        assert_reported(&failures_of(&tree), "workspace-file-delete.json");
        let tree = edited("schemas/inputs/process-start.json", |input| {
            for refusal in input["x-b10x-refusals"]
                .as_array_mut()
                .expect("the refusals")
            {
                if refusal["code"] == wire::OPERATION_RESULT_LIMIT {
                    refusal["status"] = json!(422);
                }
            }
        });
        assert_reported(&failures_of(&tree), "process-start.json");
        let tree = edited("schemas/inputs/pipe-session-start.json", |input| {
            input["x-b10x-refusals"]
                .as_array_mut()
                .expect("the refusals")
                .retain(|code| code != wire::OPERATION_RESULT_LIMIT);
        });
        assert_reported(&failures_of(&tree), "pipe-session-start.json");
        let tree = edited("schemas/inputs/exec-signal.json", |input| {
            input["x-b10x-refusals"] = json!([super::refusal_entry()]);
        });
        assert_reported(
            &failures_of(&tree),
            "exec-signal.json: names operation.result-limit",
        );
    }

    #[test]
    fn a_refusal_vector_off_the_published_form_is_reported() {
        for (member, value, named) in [
            ("class", json!("refused"), "class is \"refused\""),
            ("address", json!("limit"), "address is \"limit\""),
            ("retriable", json!(true), "retriable is true"),
            ("message", json!("Too large."), "message is \"Too large.\""),
        ] {
            let tree = edited(EXEC, |vector| {
                vector["expected"]["response"]["body"]["error"][member] = value;
            });
            assert_reported(&failures_of(&tree), named);
        }
        let tree = edited(EXEC, |vector| {
            vector["expected"]["response"]["status"] = json!(507);
        });
        assert_reported(&failures_of(&tree), "answers 507");
        let tree = edited(EXEC, |vector| {
            vector["postconditions"] = json!([]);
        });
        assert_reported(&failures_of(&tree), "/driver/dispatch_count");
    }

    #[test]
    fn a_decided_kind_without_its_vector_is_reported() {
        let mut tree = released(VERSION);
        tree.remove(FILE_DELETE);
        assert_reported(
            &failures_of(&tree),
            "no workspace.file.delete vector publishes",
        );
    }

    #[test]
    fn a_refusal_on_a_mutation_of_an_existing_resource_is_reported() {
        let tree = edited(EXEC, |vector| {
            vector["action"]["operation"] = json!("exec.signal");
        });
        assert_reported(
            &failures_of(&tree),
            "exec.signal publishes operation.result-limit",
        );
    }

    #[test]
    fn a_refusal_the_lower_value_does_not_explain_is_reported() {
        let tree = edited(FILE_DELETE, |vector| {
            vector["setup"][0]["state"]["facts"]["operation.result-max-json-bytes"] =
                json!(wire::OPERATION_RESULT_MAX_JSON_BYTES - 1);
        });
        assert_reported(&failures_of(&tree), "so nothing is refused");
        // A process record holds its argv as sent: 64 items of 4,096 six-byte characters.
        let tree = edited(PROCESS, |vector| {
            vector["action"]["request"]["body"]["input"]["argv"] =
                json!(vec!["\u{1}".repeat(4_096); 64]);
        });
        assert_reported(
            &failures_of(&tree),
            "over substrate_wire::OPERATION_RESULT_MAX_JSON_BYTES",
        );
    }

    #[test]
    fn an_unrecorded_row_off_the_published_form_is_reported() {
        let tree = edited(UNRECORDED, |vector| {
            vector["expected"]["response"]["body"]["result"]["outcome"]["error"]["class"] =
                json!("exhausted");
            vector["setup"][0]["state"]["record"]["outcome"]["error"]["class"] = json!("exhausted");
        });
        assert_reported(&failures_of(&tree), "class is \"exhausted\"");
        let tree = edited(UNRECORDED, |vector| {
            vector["expected"]["response"]["body"]["result"]["resource"] = json!(null);
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "does not name its resource");
        assert_reported(&failures, "not the row the setup stored");
        let mut tree = released(VERSION);
        tree.remove(UNRECORDED);
        assert_reported(&failures_of(&tree), "0 vectors read a row ended");
    }

    #[test]
    fn a_member_bound_that_is_not_its_wire_constant_is_reported() {
        let tree = edited("schemas/common.json", |common| {
            common["$defs"]["process-id"]["maxLength"] = json!(64);
        });
        assert_reported(&failures_of(&tree), "MAX_RESOURCE_ID_CHARS is 32");
        for (pointer, named) in [
            (
                "/$defs/exec-refusal/oneOf/1/properties/code",
                "MAX_EXEC_REFUSAL_CODE_CHARS is 128",
            ),
            (
                "/$defs/process/properties/argv/items",
                "MAX_PROCESS_ARGV_BYTES is 131072",
            ),
            (
                "/$defs/process/properties/cwd",
                "MAX_PROCESS_CWD_BYTES is 4096",
            ),
            (
                "/$defs/process/properties/env_names/items",
                "MAX_PROCESS_ENV_BYTES is 131072",
            ),
            (
                "/$defs/confinement-applied/oneOf/0/properties/cgroup",
                "MAX_APPLIED_CGROUP_CHARS is 512",
            ),
            (
                "/$defs/exec-usage/oneOf/2/properties/message",
                "MAX_EXEC_USAGE_MESSAGE_CHARS is 2048",
            ),
        ] {
            let tree = edited("schemas/resource.json", |resource| {
                resource
                    .pointer_mut(pointer)
                    .expect("the member")
                    .as_object_mut()
                    .expect("a schema")
                    .remove("maxLength");
            });
            assert_reported(&failures_of(&tree), named);
        }
        let tree = edited("schemas/resource.json", |resource| {
            resource["$defs"]["lease"]["properties"]["principal"]["maxLength"] = json!(512);
        });
        assert_reported(&failures_of(&tree), "MAX_LEASE_PRINCIPAL_CHARS is 256");
    }

    #[test]
    fn a_refusal_vector_outside_the_requirement_is_reported() {
        let tree = edited("coverage.json", |coverage| {
            for requirement in coverage["requirements"]
                .as_array_mut()
                .expect("requirements")
            {
                if requirement["id"] == super::REQUIREMENT {
                    requirement["evidence"] = json!([]);
                }
            }
        });
        assert_reported(&failures_of(&tree), "does not list the vector");
    }

    #[test]
    fn the_route_counts_are_those_of_0_27_0() {
        let tree = edited("bundle.json", |bundle| {
            bundle["compatibility"]["preserves_routes"] = json!(44);
            bundle["compatibility"]["adds_routes"] = json!(1);
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "compatibility.preserves_routes is 44");
        assert_reported(&failures, "compatibility.adds_routes is 1");
        let tree = edited("bundle.json", |bundle| {
            bundle["compatibility"]["predecessor"] = json!("0.26.0");
        });
        assert_reported(
            &failures_of(&tree),
            "compatibility.predecessor is \"0.26.0\"",
        );
    }

    #[test]
    fn the_compatibility_block_states_each_addition() {
        let tree = edited("compatibility.json", |compatibility| {
            compatibility["client_visible_changes"] = json!(["0.28.0 adds no route."]);
        });
        let failures = failures_of(&tree);
        for named in [
            "the 45 routes of 0.27.0",
            "operation.result-max-json-bytes",
            "524217",
            "operation.result-limit (429)",
            "operation.result-unrecorded (500)",
        ] {
            assert_reported(&failures, named);
        }
    }
}
