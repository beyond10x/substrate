//! What `0.20.0` exists for (ADR 0036 S1 to S7 and S11): the host-process family.
//!
//! Every value this check expects is taken from `substrate_wire`, never repeated here: the route
//! table is the one the slices name, and each bound, refusal code, status, class and message is the
//! wire crate's own constant. So a bound that moves in the crate and not in the bundle, or a refusal
//! the crate can emit that no vector names, fails here. The bundle cannot carry these through a
//! `{"$wire": …}` marker: `render_v2.rs` is hashed by every bundle it rendered (AGENTS.md § *The
//! gate*), so the binding lives in this file, which no bundle hashes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

/// One route of the family: id, method, path, idempotency, input binding and schema stem.
struct Route {
    id: &'static str,
    method: &'static str,
    path: &'static str,
    idempotency: &'static str,
    input_binding: &'static str,
    stem: &'static str,
}

/// ADR 0036 S1 to S7 and S11. S8, S9 and S12 add their routes with their own successor.
const ROUTES: [Route; 7] = [
    Route {
        id: "process.start",
        method: "POST",
        path: "/v1/processes",
        idempotency: "keyed",
        input_binding: "body.input",
        stem: "process-start",
    },
    Route {
        id: "process.list",
        method: "GET",
        path: "/v1/processes",
        idempotency: "idempotent",
        input_binding: "query",
        stem: "process-list",
    },
    Route {
        id: "process.get",
        method: "GET",
        path: "/v1/processes/{process_id}",
        idempotency: "idempotent",
        input_binding: "query",
        stem: "process-get",
    },
    Route {
        id: "process.stop",
        method: "POST",
        path: "/v1/processes/{process_id}/stop",
        idempotency: "keyed",
        input_binding: "body.input",
        stem: "process-stop",
    },
    Route {
        id: "process.stop-by-owner",
        method: "POST",
        path: "/v1/processes/stop",
        idempotency: "none",
        input_binding: "body",
        stem: "process-stop-by-owner",
    },
    Route {
        id: "process.lease.renew",
        method: "POST",
        path: "/v1/processes/{process_id}/lease/renew",
        idempotency: "keyed",
        input_binding: "body.input",
        stem: "process-lease-renew",
    },
    Route {
        id: "process.logs.get",
        method: "GET",
        path: "/v1/processes/{process_id}/logs",
        idempotency: "idempotent",
        input_binding: "query",
        stem: "process-logs-get",
    },
];

/// The scope every route of the family requires. The hosted admission profile grants no such
/// scope, so no hosted posture reaches these routes.
const SCOPE: &str = "process";

pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_routes(released, failures);
    check_fact(released, failures);
    check_process_id(released, failures);
    check_record(released, failures);
    check_bounds(released, failures);
    check_refusal_annotations(released, failures);
    check_vectors(version, released, failures);
    check_client_visible_changes(released, failures);
}

fn check_routes(released: &Tree, failures: &mut Vec<String>) {
    let Some(registry) = json_at(released, "operations.json", failures) else {
        return;
    };
    let operations: Vec<&Value> = registry
        .get("operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    for route in &ROUTES {
        let declared: Vec<&&Value> = operations
            .iter()
            .filter(|operation| operation.get("id") == Some(&json!(route.id)))
            .collect();
        let [declared] = declared.as_slice() else {
            failures.push(format!(
                "operations.json: {} must be declared exactly once, found {}",
                route.id,
                declared.len()
            ));
            continue;
        };
        for (member, expected) in [
            ("method", json!(route.method)),
            ("path", json!(route.path)),
            ("api_major", json!(1)),
            ("idempotency", json!(route.idempotency)),
            ("input_binding", json!(route.input_binding)),
            ("required_scope", json!(SCOPE)),
            (
                "input_schema",
                json!(format!("schemas/inputs/{}.json", route.stem)),
            ),
            (
                "result_schema",
                json!(format!("schemas/results/{}.json", route.stem)),
            ),
        ] {
            if declared.get(member) != Some(&expected) {
                failures.push(format!(
                    "operations.json: {} {member} must be {expected}",
                    route.id
                ));
            }
        }
    }
    // S2: a start is served only where the fact is published, so a daemon without it answers the
    // named unserved refusal rather than running anything.
    let start = operations
        .iter()
        .find(|operation| operation.get("id") == Some(&json!("process.start")));
    let gated = start
        .and_then(|start| start.get("capability_predicates"))
        .and_then(Value::as_array)
        .is_some_and(|predicates| {
            predicates.contains(&json!({
                "fact": wire::PROCESS_HOST_FACT,
                "op": "eq",
                "value": true
            }))
        });
    if start.is_some() && !gated {
        failures.push(format!(
            "operations.json: process.start must require {} eq true",
            wire::PROCESS_HOST_FACT
        ));
    }
    if let Some(schema) = json_at(released, "schemas/operation-registry.json", failures) {
        let scopes = schema.pointer("/properties/operations/items/properties/required_scope/enum");
        if !scopes
            .and_then(Value::as_array)
            .is_some_and(|scopes| scopes.contains(&json!(SCOPE)))
        {
            failures.push(format!(
                "schemas/operation-registry.json: required_scope does not admit {SCOPE}"
            ));
        }
    }
}

/// S2: the fact is published as `true` or not at all, and never required.
fn check_fact(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/capability.json";
    let Some(capability) = json_at(released, path, failures) else {
        return;
    };
    let fact = capability
        .pointer("/properties/facts/properties")
        .and_then(|facts| facts.get(wire::PROCESS_HOST_FACT));
    if fact.and_then(|fact| fact.get("const")) != Some(&json!(true)) {
        failures.push(format!(
            "{path}: {} must be published as the constant true",
            wire::PROCESS_HOST_FACT
        ));
    }
    let required = capability
        .pointer("/properties/facts/required")
        .and_then(Value::as_array);
    if required.is_some_and(|required| required.contains(&json!(wire::PROCESS_HOST_FACT))) {
        failures.push(format!(
            "{path}: {} must stay optional; absent is the unserved daemon",
            wire::PROCESS_HOST_FACT
        ));
    }
}

fn check_process_id(released: &Tree, failures: &mut Vec<String>) {
    let Some(common) = json_at(released, "schemas/common.json", failures) else {
        return;
    };
    let expected = format!("^{}[A-Za-z0-9]+$", wire::PROCESS_ID_PREFIX);
    if common.pointer("/$defs/process-id/pattern") != Some(&json!(expected)) {
        failures.push(format!(
            "schemas/common.json: process-id must be {expected}"
        ));
    }
}

/// S2: `profile: host` and `confinement: none` on every record and every log tail, and no member
/// that could carry an environment value.
fn check_record(released: &Tree, failures: &mut Vec<String>) {
    let path = "schemas/resource.json";
    let Some(resource) = json_at(released, path, failures) else {
        return;
    };
    for definition in ["process", "process-log-tail"] {
        let Some(record) = resource.pointer(&format!("/$defs/{definition}")) else {
            failures.push(format!("{path}: no {definition} definition"));
            continue;
        };
        if record.get("additionalProperties") != Some(&json!(false)) {
            failures.push(format!("{path}: {definition} must be closed"));
        }
        let required = record
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (member, value) in [("profile", "host"), ("confinement", "none")] {
            if record.pointer(&format!("/properties/{member}/const")) != Some(&json!(value))
                || !required.contains(&json!(member))
            {
                failures.push(format!(
                    "{path}: {definition} must require {member} {value}"
                ));
            }
        }
    }
    if let Some(properties) = resource
        .pointer("/$defs/process/properties")
        .and_then(Value::as_object)
    {
        for member in properties.keys() {
            if member == "env" || member == "environment" {
                failures.push(format!(
                    "{path}: the process record carries {member}; a record names env_names and \
                     env_digest, never a value"
                ));
            }
        }
    }
    let branches = resource.get("oneOf").and_then(Value::as_array);
    if !branches.is_some_and(|branches| branches.contains(&json!({"$ref": "#/$defs/process"}))) {
        failures.push(format!(
            "{path}: the observed resource union does not include the process record"
        ));
    }
}

/// Every bound the family states, each read where the bundle states it and compared with the wire
/// crate's constant.
fn check_bounds(released: &Tree, failures: &mut Vec<String>) {
    let mut documents: BTreeMap<&str, Option<Value>> = BTreeMap::new();
    for (path, pointer, expected) in start_input_bounds().into_iter().chain(other_bounds()) {
        let document = documents
            .entry(path)
            .or_insert_with(|| json_at(released, path, failures));
        let Some(document) = document else {
            continue;
        };
        if document.pointer(pointer) != Some(&expected) {
            failures.push(format!(
                "{path}: {pointer} must be the wire bound {expected}"
            ));
        }
    }
}

/// The bounds `process.start` states on its own input.
fn start_input_bounds() -> [(&'static str, &'static str, Value); 13] {
    let start = "schemas/inputs/process-start.json";
    [
        (
            start,
            "/properties/argv/x-b10x-bound/max_items",
            json!(wire::MAX_PROCESS_ARGV_ITEMS),
        ),
        (
            start,
            "/properties/argv/x-b10x-bound/max_total_bytes",
            json!(wire::MAX_PROCESS_ARGV_BYTES),
        ),
        (
            start,
            "/properties/env/x-b10x-bound/max_items",
            json!(wire::MAX_PROCESS_ENV_ITEMS),
        ),
        (
            start,
            "/properties/env/x-b10x-bound/max_total_bytes",
            json!(wire::MAX_PROCESS_ENV_BYTES),
        ),
        (
            start,
            "/properties/owner/properties/session/x-b10x-bound/max_bytes",
            json!(wire::MAX_PROCESS_OWNER_BYTES),
        ),
        (
            start,
            "/properties/owner/properties/name/x-b10x-bound/max_bytes",
            json!(wire::MAX_PROCESS_OWNER_BYTES),
        ),
        (
            start,
            "/properties/reason/x-b10x-bound/max_bytes",
            json!(wire::MAX_PROCESS_REASON_BYTES),
        ),
        (
            start,
            "/properties/lease/properties/keep_reason/x-b10x-bound/max_bytes",
            json!(wire::MAX_PROCESS_REASON_BYTES),
        ),
        (
            start,
            "/properties/lease/properties/ttl_ms/x-b10x-bound/minimum",
            json!(wire::MIN_LEASE_TTL_MS),
        ),
        (
            start,
            "/properties/lease/properties/ttl_ms/x-b10x-bound/maximum",
            json!(wire::MAX_LEASE_TTL_MS),
        ),
        (
            start,
            "/properties/timeout_ms/x-b10x-bound/maximum",
            json!(wire::MAX_PROCESS_TIMEOUT_MS),
        ),
        (
            start,
            "/properties/expires_at/x-b10x-bound/min_ahead_ms",
            json!(wire::MIN_LEASE_TTL_MS),
        ),
        (
            start,
            "/properties/expires_at/x-b10x-bound/max_ahead_ms",
            json!(wire::MAX_PROCESS_TIMEOUT_MS),
        ),
    ]
}

/// The bounds the other inputs, the results and the record state.
fn other_bounds() -> [(&'static str, &'static str, Value); 13] {
    let resource = "schemas/resource.json";
    [
        (
            "schemas/inputs/process-list.json",
            "/properties/limit/maximum",
            json!(wire::MAX_PROCESS_LIST_ITEMS),
        ),
        (
            "schemas/results/process-list.json",
            "/properties/items/maxItems",
            json!(wire::MAX_PROCESS_LIST_ITEMS),
        ),
        (
            "schemas/inputs/process-logs-get.json",
            "/properties/max_bytes/maximum",
            json!(wire::MAX_PROCESS_LOG_TAIL_BYTES),
        ),
        (
            "schemas/inputs/process-lease-renew.json",
            "/properties/ttl_ms/minimum",
            json!(wire::MIN_LEASE_TTL_MS),
        ),
        (
            "schemas/inputs/process-lease-renew.json",
            "/properties/ttl_ms/maximum",
            json!(wire::MAX_LEASE_TTL_MS),
        ),
        (
            resource,
            "/$defs/process/x-b10x-bounds/max_live_per_subject",
            json!(wire::MAX_PROCESSES_PER_SUBJECT),
        ),
        (
            resource,
            "/$defs/process/x-b10x-bounds/log_bytes",
            json!(wire::MAX_PROCESS_LOG_BYTES),
        ),
        (
            resource,
            "/$defs/process/x-b10x-bounds/retain_ended_ms",
            json!(wire::PROCESS_RETAIN_ENDED_MS),
        ),
        (
            resource,
            "/$defs/process/x-b10x-bounds/retain_ended_count",
            json!(wire::PROCESS_RETAIN_ENDED_COUNT),
        ),
        (
            resource,
            "/$defs/process/x-b10x-bounds/stop_grace_ms",
            json!(wire::PROCESS_STOP_GRACE_MS),
        ),
        (
            resource,
            "/$defs/process/properties/timeout_ms/maximum",
            json!(wire::MAX_PROCESS_TIMEOUT_MS),
        ),
        (
            resource,
            "/$defs/process-log-line/properties/text/x-b10x-bound/max_bytes",
            json!(wire::MAX_PROCESS_LOG_LINE_BYTES),
        ),
        (
            resource,
            "/$defs/process-log-tail/properties/retained_bytes/maximum",
            json!(wire::MAX_PROCESS_LOG_BYTES),
        ),
    ]
}

/// The class, not the instance: the refusals the family's inputs declare are exactly the codes,
/// statuses and classes `substrate_wire::PROCESS_REFUSALS` holds, so no code the crate can emit is
/// missing from the contract and the contract names none the crate cannot.
fn check_refusal_annotations(released: &Tree, failures: &mut Vec<String>) {
    let expected: BTreeSet<(String, u64, String)> = wire::PROCESS_REFUSALS
        .iter()
        .map(|refusal| {
            (
                refusal.code.to_owned(),
                u64::from(refusal.status),
                class(refusal),
            )
        })
        .collect();
    let mut declared = BTreeSet::new();
    for route in &ROUTES {
        let path = format!("schemas/inputs/{}.json", route.stem);
        let Some(input) = json_at(released, &path, failures) else {
            continue;
        };
        for refusal in input
            .get("x-b10x-refusals")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(code), Some(status), Some(class)) = (
                refusal.get("code").and_then(Value::as_str),
                refusal.get("status").and_then(Value::as_u64),
                refusal.get("class").and_then(Value::as_str),
            ) else {
                failures.push(format!(
                    "{path}: a declared refusal lacks code, status or class"
                ));
                continue;
            };
            declared.insert((code.to_owned(), status, class.to_owned()));
        }
    }
    for (code, status, class) in expected.difference(&declared) {
        failures.push(format!(
            "schemas/inputs/process-*.json: no input declares {code} ({status}, {class})"
        ));
    }
    for (code, status, class) in declared.difference(&expected) {
        failures.push(format!(
            "schemas/inputs/process-*.json: {code} ({status}, {class}) is not a refusal the wire \
             crate emits"
        ));
    }
}

fn class(refusal: &wire::ProcessRefusal) -> String {
    serde_json::to_value(refusal.class)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Every route has a vector that answers it, every refusal the crate emits is answered by a vector
/// with the crate's exact status, class and message, and every success result a vector shows
/// validates against its route's result schema.
fn check_vectors(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let validators = result_validators(version, released, failures);
    let mut answered: BTreeSet<&str> = BTreeSet::new();
    let mut refused: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let Some(operation) = vector.pointer("/action/operation").and_then(Value::as_str) else {
            continue;
        };
        let Some(route) = ROUTES.iter().find(|route| route.id == operation) else {
            continue;
        };
        let Some(response) = vector.pointer("/expected/response") else {
            continue;
        };
        if let Some(error) = response.pointer("/body/error") {
            let code = error.get("code").and_then(Value::as_str).unwrap_or("");
            let Some(refusal) = wire::process_refusal(code) else {
                if code.starts_with("process.") {
                    failures.push(format!(
                        "{path}: {code} is not a refusal the wire crate emits"
                    ));
                }
                continue;
            };
            let observed = (
                response.get("status").and_then(Value::as_u64),
                error.get("class").and_then(Value::as_str),
                error.get("message").and_then(Value::as_str),
            );
            let class = class(&refusal);
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
            refused
                .entry(code.to_owned())
                .or_default()
                .push(path.clone());
        } else if let Some(result) = response.pointer("/body/result") {
            answered.insert(route.id);
            if let Some(Some(validator)) = validators.get(route.id)
                && let Err(error) = validator.validate(result)
            {
                failures.push(format!(
                    "{path}: the {} result does not validate against its result schema: {error}",
                    route.id
                ));
            }
        }
    }
    for route in &ROUTES {
        if !answered.contains(route.id) {
            failures.push(format!(
                "vectors: no {} vector answers a success result",
                route.id
            ));
        }
    }
    for refusal in &wire::PROCESS_REFUSALS {
        if !refused.contains_key(refusal.code) {
            failures.push(format!(
                "vectors: no host-process vector answers {}",
                refusal.code
            ));
        }
    }
}

/// One validator per route, for its result schema, compiled against the bundle's own schemas.
fn result_validators(
    version: &str,
    released: &Tree,
    failures: &mut Vec<String>,
) -> BTreeMap<&'static str, Option<jsonschema::Validator>> {
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
                return BTreeMap::new();
            }
        }
    }
    let registry = match registry.prepare() {
        Ok(registry) => registry,
        Err(error) => {
            failures.push(format!("bundle schemas do not form a registry: {error}"));
            return BTreeMap::new();
        }
    };
    ROUTES
        .iter()
        .map(|route| {
            let target = format!("schemas/results/{}.json", route.stem);
            let reference = json!({ "$ref": super::resource_uri(version, &target) });
            let validator = jsonschema::draft202012::options()
                .with_registry(&registry)
                .build(&reference)
                .map_err(|error| failures.push(format!("{target}: does not compile: {error}")))
                .ok();
            (route.id, validator)
        })
        .collect()
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
    for named in [wire::PROCESS_HOST_FACT, "confinement none", "0.19.0"] {
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
        super::super::check_additions("0.20.0", tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released("0.20.0");
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
    fn the_successor_carries_the_host_process_family() {
        let failures = failures_of(&released("0.20.0"));
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing about the tree that
    /// carries it.
    #[test]
    fn the_predecessor_carries_none_of_it() {
        let failures = failures_of(&released("0.19.0"));
        for named in [
            "process.start must be declared exactly once",
            "process.stop-by-owner must be declared exactly once",
            "process.logs.get must be declared exactly once",
            "required_scope does not admit process",
            "process.host must be published as the constant true",
            "process-id must be ^proc_[A-Za-z0-9]+$",
            "no process definition",
            "schemas/inputs/process-start.json: absent",
            "no host-process vector answers process.host-unserved",
            "no process.list vector answers a success result",
            "client_visible_changes must state process.host",
        ] {
            assert_reported(&failures, named);
        }
    }

    /// Each bound is the wire constant: one moved in the bundle alone is reported by name.
    #[test]
    fn a_bound_that_is_not_the_wire_constant_is_reported() {
        let tree = edited("schemas/inputs/process-start.json", |document| {
            document["properties"]["argv"]["x-b10x-bound"]["max_items"] = 255.into();
        });
        assert_reported(
            &failures_of(&tree),
            "/properties/argv/x-b10x-bound/max_items must be the wire bound 256",
        );
        let tree = edited("schemas/resource.json", |document| {
            document["$defs"]["process"]["x-b10x-bounds"]["stop_grace_ms"] = 5_000.into();
        });
        assert_reported(
            &failures_of(&tree),
            "stop_grace_ms must be the wire bound 10000",
        );
    }

    /// The class: a refusal the crate emits that no input declares, or that an input declares
    /// with another status, is reported; so is one the crate does not emit.
    #[test]
    fn refusal_declarations_are_exactly_the_wire_table() {
        let tree = edited("schemas/inputs/process-start.json", |document| {
            let refusals = document["x-b10x-refusals"].as_array_mut().expect("array");
            refusals.retain(|refusal| refusal["code"] != "process.timeout-invalid");
            refusals.push(serde_json::json!({
                "class": "refused",
                "code": "process.container-restart-refused",
                "status": 422
            }));
        });
        let failures = failures_of(&tree);
        assert_reported(&failures, "no input declares process.timeout-invalid");
        assert_reported(
            &failures,
            "process.container-restart-refused (422, refused) is not a refusal the wire crate emits",
        );
    }

    /// Every refusal is answered by a vector with the wire's status, class and message.
    #[test]
    fn a_refusal_no_vector_answers_or_answers_wrongly_is_reported() {
        let mut tree = released("0.20.0");
        tree.remove("vectors/http/process-start-zero-timeout-is-refused.json");
        assert_reported(
            &failures_of(&tree),
            "no host-process vector answers process.timeout-invalid",
        );
        let path = "vectors/http/process-start-empty-reason-is-refused.json";
        let tree = edited(path, |document| {
            document["expected"]["response"]["status"] = 400.into();
        });
        assert_reported(
            &failures_of(&tree),
            "process.reason-required must answer 422 refused with the wire message",
        );
    }

    /// A success result in a vector is held to its route's result schema: a record that claimed
    /// confinement would not validate.
    #[test]
    fn a_vector_result_outside_its_schema_is_reported() {
        let tree = edited("vectors/http/process-get.json", |document| {
            document["expected"]["response"]["body"]["result"]["confinement"] = "namespaces".into();
        });
        assert_reported(
            &failures_of(&tree),
            "the process.get result does not validate against its result schema",
        );
    }
}
