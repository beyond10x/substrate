//! The local endpoint of ADR 0054, published by `substrate-wire/0.30.0` and held by every later
//! bundle.
//!
//! A local endpoint is an operator-declared aperture whose destination is a directory of Unix
//! sockets. The bundle publishes the start's optional `sandbox.local_endpoint_key`, the second
//! value of an applied aperture's `mechanism` with the `listen` and `key` it states, and one
//! vector for each of the three refusals. The renderer cannot be edited (every bundle hashes it),
//! so this module holds each published value to its `substrate_wire` constant: the key's bounds
//! and pattern to [`wire::MAX_LOCAL_ENDPOINT_KEY_BYTES`] and [`wire::valid_local_endpoint_key`],
//! each refusal to its row of [`wire::LOCAL_ENDPOINT_REFUSALS`], and the applied record and the
//! requested sandbox of the published vector to what the wire crate writes.

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const VERSION: &str = "0.30.0";
const PREDECESSOR: &str = "0.29.0";
/// The routes of `0.29.0`, which `0.30.0` preserves and adds none to.
const ROUTES_OF_0_29_0: u64 = 45;

const BUNDLE: &str = "bundle.json";
const COMMON: &str = "schemas/common.json";
const COMPATIBILITY: &str = "compatibility.json";
const COVERAGE: &str = "coverage.json";
const RESOURCE: &str = "schemas/resource.json";

/// The definition of the key in `common.json`.
const KEY_DEFINITION: &str = "local-endpoint-key";
/// The key's pattern, as ADR 0054 decision 3 states it.
const KEY_PATTERN: &str = "^[a-z0-9][a-z0-9-]*$";
/// The start's member, and the address both key refusals are answered at.
const KEY_MEMBER: &str = "local_endpoint_key";
const KEY_ADDRESS: &str = "sandbox.local_endpoint_key";

/// The coverage requirement every refusal vector is evidence of.
const REFUSALS_REQUIREMENT: &str = "exec.local-endpoints-refusals";
/// The coverage requirement of the vector that observes an applied local endpoint.
const APPLIED_REQUIREMENT: &str = "exec.local-endpoints-applied";

/// The vector of an applied local endpoint.
const APPLIED: &str = "vectors/http/local-endpoint-applied-is-observed.json";
/// One vector per row of [`wire::LOCAL_ENDPOINT_REFUSALS`], in its order, with the recorded state
/// of the accepted operation: the daemon refuses the first two before dispatch, and the host
/// driver answers the third (design 37 D23).
const REFUSAL_VECTORS: [(&str, &str); 3] = [
    ("vectors/http/local-endpoints-unserved.json", "refused"),
    (
        "vectors/http/local-endpoint-key-without-endpoint-is-refused.json",
        "refused",
    ),
    (
        "vectors/http/local-endpoint-key-in-use-is-refused.json",
        "terminal",
    ),
];

/// The two starts whose sandbox carries the key, and where each states it.
const STARTS: [(&str, &str); 2] = [
    ("schemas/inputs/exec-start.json", "/properties/sandbox"),
    (
        "schemas/inputs/pipe-session-start.json",
        "/properties/exec/properties/sandbox",
    ),
];

/// The two forms of an applied network that state one aperture each, in `resource.json`.
const APPLIED_FORMS: [&str; 2] = [
    "/$defs/applied-network/oneOf/1",
    "/$defs/applied-network/oneOf/2/items",
];

/// Keys the published pattern and the wire crate must judge alike.
const KEY_PROBES: [&str; 14] = [
    "worker-1", "a", "0", "9-", "a--b", "", "-a", "A", "a_b", "a.b", "a/b", "ä", "a b", "ex_01",
];

/// Whether `version` is the bundle that introduced the local endpoint: `0.30.0` only.
pub(super) fn binds(version: &str) -> bool {
    version == VERSION
}

/// Whether the local endpoint binds `version`: `0.30.0` and every later bundle.
pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => {
            (major, minor, patch) >= (0, 30, 0)
        }
        _ => false,
    }
}

/// Every check of the additions, for `0.30.0` and every later bundle.
pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    check_key_definition(released, failures);
    check_starts(released, failures);
    check_record_schema(released, failures);
    check_applied_vector(version, released, failures);
    check_refusal_vectors(released, failures);
    check_coverage(released, failures);
}

/// What only `0.30.0` states: its predecessor, its route counts and its client-visible changes.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
    if let Some(bundle) = json_at(released, BUNDLE, failures) {
        for (member, expected) in [
            ("predecessor", json!(PREDECESSOR)),
            ("kind", json!("additive-v1")),
            ("adds_routes", json!(0)),
            ("preserves_routes", json!(ROUTES_OF_0_29_0)),
        ] {
            let stated = bundle.pointer(&format!("/compatibility/{member}"));
            if stated != Some(&expected) {
                failures.push(format!(
                    "{BUNDLE}: compatibility.{member} is {}, and {VERSION} states {expected}",
                    stated.unwrap_or(&Value::Null)
                ));
            }
        }
    }
    let routes = json_at(released, "operations.json", failures)
        .and_then(|operations| {
            operations
                .get("operations")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or_default() as u64;
    if routes != ROUTES_OF_0_29_0 {
        failures.push(format!(
            "operations.json: {routes} routes, and {VERSION} serves the {ROUTES_OF_0_29_0} of \
             {PREDECESSOR}"
        ));
    }
    let Some(compatibility) = json_at(released, COMPATIBILITY, failures) else {
        return;
    };
    let changes: Vec<&str> = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut named = vec![
        "ADR 0054".to_owned(),
        format!("the {ROUTES_OF_0_29_0} routes of {PREDECESSOR}"),
        KEY_ADDRESS.to_owned(),
        "MAX_LOCAL_ENDPOINT_KEY_BYTES".to_owned(),
        format!("{}<port>", wire::LOCAL_ENDPOINT_DESTINATION_PREFIX),
        "local-endpoint-forwarder".to_owned(),
        "No host path".to_owned(),
    ];
    named.extend(
        wire::LOCAL_ENDPOINT_REFUSALS
            .iter()
            .map(|refusal| format!("{} ({}", refusal.code, refusal.status)),
    );
    for named in named {
        if !changes.iter().any(|change| change.contains(&named)) {
            failures.push(format!(
                "{COMPATIBILITY}: client_visible_changes must state {named}"
            ));
        }
    }
}

/// The class a row is published with.
fn class_of(refusal: &wire::ExecStartRefusal) -> Value {
    serde_json::to_value(refusal.class).unwrap_or(Value::Null)
}

/// The `x-b10x-refusals` entry of a row.
fn published(refusal: &wire::ExecStartRefusal) -> Value {
    json!({
        "class": class_of(refusal),
        "code": refusal.code,
        "status": refusal.status,
    })
}

/// Row `index` of ADR 0054 decision 9, `0` to `2`.
fn row(index: usize) -> wire::ExecStartRefusal {
    wire::LOCAL_ENDPOINT_REFUSALS[index]
}

/// The key's definition states the wire bound and the pattern, and judges every probe as
/// [`wire::valid_local_endpoint_key`] does.
fn check_key_definition(released: &Tree, failures: &mut Vec<String>) {
    let Some(common) = json_at(released, COMMON, failures) else {
        return;
    };
    let Some(definition) = common.pointer(&format!("/$defs/{KEY_DEFINITION}")) else {
        failures.push(format!("{COMMON}: no {KEY_DEFINITION} definition"));
        return;
    };
    for (member, expected) in [
        ("type", json!("string")),
        ("minLength", json!(1)),
        ("maxLength", json!(wire::MAX_LOCAL_ENDPOINT_KEY_BYTES)),
        ("pattern", json!(KEY_PATTERN)),
    ] {
        if definition.get(member) != Some(&expected) {
            failures.push(format!(
                "{COMMON}: {KEY_DEFINITION}.{member} is {}, and the wire crate states {expected}",
                definition.get(member).unwrap_or(&Value::Null)
            ));
        }
    }
    let validator = match jsonschema::validator_for(definition) {
        Ok(validator) => validator,
        Err(error) => {
            failures.push(format!(
                "{COMMON}: {KEY_DEFINITION} does not compile: {error}"
            ));
            return;
        }
    };
    let longest = "a".repeat(wire::MAX_LOCAL_ENDPOINT_KEY_BYTES);
    let longer = "a".repeat(wire::MAX_LOCAL_ENDPOINT_KEY_BYTES + 1);
    for probe in KEY_PROBES
        .iter()
        .copied()
        .chain([longest.as_str(), longer.as_str()])
    {
        let published = validator.is_valid(&json!(probe));
        if published != wire::valid_local_endpoint_key(probe) {
            failures.push(format!(
                "{COMMON}: {KEY_DEFINITION} admits {probe:?} is {published}, and the wire crate \
                 says {}",
                !published
            ));
        }
    }
}

/// Both starts carry the key with the two key refusals, and both aperture members publish the
/// unserved refusal.
fn check_starts(released: &Tree, failures: &mut Vec<String>) {
    let key = json!({
        "$ref": format!("../common.json#/$defs/{KEY_DEFINITION}"),
        "x-b10x-refusals": [published(&row(1)), published(&row(2))],
    });
    let unserved = published(&row(0));
    for (path, sandbox) in STARTS {
        let Some(start) = json_at(released, path, failures) else {
            continue;
        };
        let Some(sandbox) = start.pointer(sandbox) else {
            failures.push(format!("{path}: no sandbox"));
            continue;
        };
        if sandbox.pointer(&format!("/properties/{KEY_MEMBER}")) != Some(&key) {
            failures.push(format!("{path}: sandbox.{KEY_MEMBER} is not {key}"));
        }
        if sandbox
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.contains(&json!(KEY_MEMBER)))
        {
            failures.push(format!("{path}: sandbox requires {KEY_MEMBER}"));
        }
        for member in ["aperture", "apertures"] {
            let refusals = sandbox
                .pointer(&format!("/properties/{member}/x-b10x-refusals"))
                .and_then(Value::as_array);
            if !refusals.is_some_and(|refusals| refusals.contains(&unserved)) {
                failures.push(format!(
                    "{path}: sandbox.{member} does not publish {unserved}"
                ));
            }
        }
    }
}

/// Each form of an applied aperture names both mechanisms the wire crate writes, and states
/// `listen` and `key`; every requested sandbox of the exec record carries the key.
fn check_record_schema(released: &Tree, failures: &mut Vec<String>) {
    let Some(resource) = json_at(released, RESOURCE, failures) else {
        return;
    };
    let mechanisms: Vec<Value> = [
        wire::ApertureMechanism::LoopbackForwarder,
        wire::ApertureMechanism::LocalEndpointForwarder,
    ]
    .iter()
    .map(|mechanism| serde_json::to_value(mechanism).unwrap_or(Value::Null))
    .collect();
    for form in APPLIED_FORMS {
        let Some(shape) = resource.pointer(form) else {
            failures.push(format!("{RESOURCE}: no applied aperture at {form}"));
            continue;
        };
        let stated = shape.pointer("/properties/mechanism/enum");
        if stated != Some(&Value::Array(mechanisms.clone())) {
            failures.push(format!(
                "{RESOURCE}: the mechanism at {form} is {}, and the wire crate writes {}",
                stated.unwrap_or(&Value::Null),
                Value::Array(mechanisms.clone())
            ));
        }
        for member in ["listen", "key"] {
            if shape.pointer(&format!("/properties/{member}")).is_none() {
                failures.push(format!(
                    "{RESOURCE}: the aperture at {form} states no {member}"
                ));
            }
        }
        let key_ref = shape.pointer("/properties/key/$ref");
        if key_ref != Some(&json!(format!("common.json#/$defs/{KEY_DEFINITION}"))) {
            failures.push(format!(
                "{RESOURCE}: the key at {form} is not the {KEY_DEFINITION} definition"
            ));
        }
    }
    let mut requested = Vec::new();
    requested_shapes(&resource, "", &mut requested);
    if requested.is_empty() {
        failures.push(format!("{RESOURCE}: no requested sandbox"));
    }
    for (at, shape) in requested {
        let stated = shape.pointer(&format!("/properties/{KEY_MEMBER}/$ref"));
        if stated != Some(&json!(format!("common.json#/$defs/{KEY_DEFINITION}"))) {
            failures.push(format!(
                "{RESOURCE}: the requested sandbox at {at} does not carry {KEY_MEMBER}"
            ));
        }
    }
}

/// Every object of `value` that states both aperture members and `require`: a requested sandbox.
fn requested_shapes<'a>(value: &'a Value, at: &str, found: &mut Vec<(String, &'a Value)>) {
    match value {
        Value::Object(object) => {
            if let Some(properties) = object.get("properties").and_then(Value::as_object)
                && ["aperture", "apertures", "require"]
                    .iter()
                    .all(|member| properties.contains_key(*member))
            {
                found.push((at.to_owned(), value));
            }
            for (name, child) in object {
                requested_shapes(child, &format!("{at}/{name}"), found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                requested_shapes(child, &format!("{at}/{index}"), found);
            }
        }
        _ => {}
    }
}

/// The vector of an applied local endpoint: its start is one the wire admits; the applied network
/// and the requested sandbox it answers are what the wire crate writes; the applied aperture is a
/// local endpoint that states its destination by port alone, its `listen` and its key; and the
/// response validates against the exec result schema.
fn check_applied_vector(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let Some(vector) = json_at(released, APPLIED, failures) else {
        return;
    };
    let Some(input) = vector.pointer("/action/request/body/input") else {
        failures.push(format!("{APPLIED}: has no request input"));
        return;
    };
    let start = match serde_json::from_value::<wire::ExecStartInput>(input.clone()) {
        Ok(start) => start,
        Err(error) => {
            failures.push(format!(
                "{APPLIED}: the wire does not admit its start: {error}"
            ));
            return;
        }
    };
    let result = vector.pointer("/expected/response/body/result");
    let Some(network) = result.and_then(|result| result.pointer("/applied/network")) else {
        failures.push(format!("{APPLIED}: answers no applied network"));
        return;
    };
    match serde_json::from_value::<wire::AppliedNetwork>(network.clone()) {
        Ok(wire::AppliedNetwork::Aperture(aperture)) => {
            let destination = aperture
                .destination
                .strip_prefix(wire::LOCAL_ENDPOINT_DESTINATION_PREFIX)
                .and_then(|port| port.parse::<u16>().ok())
                .map(wire::local_endpoint_destination);
            let listen_port = aperture
                .listen
                .as_deref()
                .and_then(|listen| listen.rsplit_once(':'))
                .map(|(_, port)| port);
            let destination_port = aperture
                .destination
                .strip_prefix(wire::LOCAL_ENDPOINT_DESTINATION_PREFIX);
            if aperture.mechanism != wire::ApertureMechanism::LocalEndpointForwarder
                || destination.as_deref() != Some(aperture.destination.as_str())
                || aperture.listen.as_deref() != Some("127.0.0.1:7000")
                || listen_port != destination_port
                || aperture.key != start.sandbox.local_endpoint_key
                || aperture.max_bytes.is_some()
            {
                failures.push(format!(
                    "{APPLIED}: the applied aperture {network} is not a local endpoint that \
                     states local:<port>, the listen address of that port and the start's key"
                ));
            }
            match serde_json::to_value(&aperture) {
                Ok(written) if &written == network => {}
                Ok(written) => failures.push(format!(
                    "{APPLIED}: the wire crate writes the applied aperture as {written}"
                )),
                Err(error) => failures.push(format!("{APPLIED}: {error}")),
            }
        }
        Ok(other) => failures.push(format!(
            "{APPLIED}: the applied network is {other:?}, not the single form"
        )),
        Err(error) => failures.push(format!(
            "{APPLIED}: the wire does not read the applied network: {error}"
        )),
    }
    let requested = result.and_then(|result| result.get("requested"));
    match serde_json::to_value(&start.sandbox) {
        Ok(written) if Some(&written) == requested => {}
        Ok(written) => failures.push(format!(
            "{APPLIED}: the requested sandbox is not the start's, which the wire writes as \
             {written}"
        )),
        Err(error) => failures.push(format!("{APPLIED}: {error}")),
    }
    let Some(validator) =
        super::workdir_apertures::exec_result_validator(version, released, failures)
    else {
        return;
    };
    if let Some(result) = result
        && let Err(error) = validator.validate(result)
    {
        failures.push(format!(
            "{APPLIED}: schemas/results/exec-start.json rejects the exec record: {error} at {}",
            error.instance_path()
        ));
    }
}

/// Each refusal vector answers its row's code, status, class and fixed message at its row's
/// address, for a start the wire admits, and states the recorded state of its row.
fn check_refusal_vectors(released: &Tree, failures: &mut Vec<String>) {
    for (index, (path, recorded)) in REFUSAL_VECTORS.iter().enumerate() {
        let refusal = row(index);
        let Some(vector) = json_at(released, path, failures) else {
            continue;
        };
        let Some(input) = vector.pointer("/action/request/body/input") else {
            failures.push(format!("{path}: has no request input"));
            continue;
        };
        let start = match serde_json::from_value::<wire::ExecStartInput>(input.clone()) {
            Ok(start) => start,
            Err(error) => {
                failures.push(format!(
                    "{path}: the wire does not admit its start: {error}"
                ));
                continue;
            }
        };
        let address = if index == 0 {
            if start.sandbox.apertures.is_empty() {
                "sandbox.network.aperture"
            } else {
                "sandbox.apertures"
            }
        } else {
            KEY_ADDRESS
        };
        let op = vector.pointer("/action/request/body/op").cloned();
        let expected = json!({
            "address": address,
            "class": class_of(&refusal),
            "code": refusal.code,
            "message": refusal.message,
            "operation": op,
            "retriable": false,
        });
        let error = vector.pointer("/expected/response/body/error");
        if error != Some(&expected) {
            failures.push(format!(
                "{path}: answers {}, and its row answers {expected}",
                error.unwrap_or(&Value::Null)
            ));
        }
        let status = vector.pointer("/expected/response/status");
        if status != Some(&json!(refusal.status)) {
            failures.push(format!(
                "{path}: answers status {}, and its row answers {}",
                status.unwrap_or(&Value::Null),
                refusal.status
            ));
        }
        for (actual, expected) in [
            ("/probes/operation_state", json!(recorded)),
            ("/probes/program_started", json!(false)),
        ] {
            if !states(&vector, actual, &expected) {
                failures.push(format!("{path}: does not state {actual} {expected}"));
            }
        }
        let names_key = start.sandbox.local_endpoint_key.is_some();
        if index > 0 && !names_key {
            failures.push(format!("{path}: refuses a key for a start that names none"));
        }
    }
}

fn states(vector: &Value, actual: &str, expected: &Value) -> bool {
    vector
        .get("postconditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|condition| {
            condition.get("actual") == Some(&json!(actual))
                && condition.get("operator") == Some(&json!("equals"))
                && condition.get("expected") == Some(expected)
        })
}

/// Both requirements are declared, and each vector is evidence of its own.
fn check_coverage(released: &Tree, failures: &mut Vec<String>) {
    let Some(coverage) = json_at(released, COVERAGE, failures) else {
        return;
    };
    let declared = |requirement: &str| {
        coverage
            .get("requirements")
            .and_then(Value::as_array)
            .is_some_and(|requirements| {
                requirements.iter().any(|entry| {
                    entry == requirement
                        || entry.get("id").and_then(Value::as_str) == Some(requirement)
                })
            })
    };
    for requirement in [APPLIED_REQUIREMENT, REFUSALS_REQUIREMENT] {
        if !declared(requirement) {
            failures.push(format!("{COVERAGE}: does not declare {requirement}"));
        }
    }
    let covers = |path: &str, requirement: &str| {
        released
            .get(path)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .and_then(|vector| vector.get("covers").cloned())
            .and_then(|covers| covers.as_array().cloned())
            .is_some_and(|covers| covers.contains(&json!(requirement)))
    };
    if !covers(APPLIED, APPLIED_REQUIREMENT) {
        failures.push(format!("{APPLIED}: does not cover {APPLIED_REQUIREMENT}"));
    }
    for (path, _) in REFUSAL_VECTORS {
        if !covers(path, REFUSALS_REQUIREMENT) {
            failures.push(format!("{path}: does not cover {REFUSALS_REQUIREMENT}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::repo;

    const VERSION: &str = super::VERSION;

    fn released(version: &str) -> super::Tree {
        super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire")
                .join(version),
        )
        .expect("the bundle reads")
    }

    fn failures_of(version: &str, tree: &super::Tree) -> Vec<String> {
        let mut failures = Vec::new();
        super::check(version, tree, &mut failures);
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
    fn the_module_applies_from_0_30_0_on_and_binds_its_introduction_to_it() {
        assert!(super::binds("0.30.0"));
        assert!(super::applies_to("0.30.0"));
        assert!(super::applies_to("0.31.0"));
        assert!(super::applies_to("1.0.0"));
        for version in ["0.29.0", "0.5.0", "0.30", "x"] {
            assert!(!super::applies_to(version), "{version}");
        }
        assert!(!super::binds("0.31.0"));
    }

    #[test]
    fn the_successor_publishes_the_local_endpoint() {
        let failures = failures_of(VERSION, &released(VERSION));
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The whole additions check of the version passes.
    #[test]
    fn the_additions_of_0_30_0_pass() {
        let tree = released(VERSION);
        let mut failures = Vec::new();
        super::super::check_additions(VERSION, &tree, &mut failures);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// `0.29.0` declares none of it, so every check reports.
    #[test]
    fn the_predecessor_declares_none_of_it() {
        let mut failures = Vec::new();
        super::check(
            super::PREDECESSOR,
            &released(super::PREDECESSOR),
            &mut failures,
        );
        for named in [
            "no local-endpoint-key definition",
            "schemas/inputs/exec-start.json: sandbox.local_endpoint_key is not",
            "schemas/inputs/pipe-session-start.json: sandbox.local_endpoint_key is not",
            "does not publish",
            "the mechanism at /$defs/applied-network/oneOf/1",
            "does not carry local_endpoint_key",
            "vectors/http/local-endpoint-applied-is-observed.json: absent from the bundle",
            "vectors/http/local-endpoint-key-in-use-is-refused.json: absent from the bundle",
            "does not declare exec.local-endpoints-refusals",
        ] {
            assert_reported(&failures, named);
        }
    }

    #[test]
    fn a_key_bound_other_than_the_wire_constant_is_reported() {
        let tree = edited(super::COMMON, |common| {
            common["$defs"]["local-endpoint-key"]["maxLength"] = json!(65);
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(&failures, "local-endpoint-key.maxLength is 65");
        assert_reported(&failures, "admits \"aaaa");
    }

    #[test]
    fn a_pattern_that_admits_an_underscore_is_reported() {
        let tree = edited(super::COMMON, |common| {
            common["$defs"]["local-endpoint-key"]["pattern"] = json!("^[a-z0-9_][a-z0-9_-]*$");
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(&failures, "local-endpoint-key.pattern");
        assert_reported(&failures, "admits \"a_b\" is true");
    }

    #[test]
    fn a_refusal_off_its_row_is_reported() {
        let path = super::REFUSAL_VECTORS[2].0;
        let tree = edited(path, |vector| {
            vector["expected"]["response"]["status"] = json!(422);
            vector["expected"]["response"]["body"]["error"]["class"] = json!("refused");
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(&failures, "answers status 422");
        assert_reported(&failures, "and its row answers");
    }

    #[test]
    fn a_key_in_use_recorded_as_refused_is_reported() {
        let path = super::REFUSAL_VECTORS[2].0;
        let tree = edited(path, |vector| {
            for condition in vector["postconditions"].as_array_mut().expect("a list") {
                if condition["actual"] == "/probes/operation_state" {
                    condition["expected"] = json!("refused");
                }
            }
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(
            &failures,
            "does not state /probes/operation_state \"terminal\"",
        );
    }

    #[test]
    fn an_applied_local_endpoint_without_its_key_is_reported() {
        let tree = edited(super::APPLIED, |vector| {
            let network =
                &mut vector["expected"]["response"]["body"]["result"]["applied"]["network"];
            network.as_object_mut().expect("an object").remove("key");
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(
            &failures,
            "is not a local endpoint that states local:<port>",
        );
        assert_reported(&failures, "rejects the exec record");
    }

    #[test]
    fn an_applied_local_endpoint_that_names_a_host_path_is_reported() {
        let tree = edited(super::APPLIED, |vector| {
            vector["expected"]["response"]["body"]["result"]["applied"]["network"]["destination"] =
                json!("/run/agent/worker-1");
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(
            &failures,
            "is not a local endpoint that states local:<port>",
        );
    }

    #[test]
    fn a_start_without_the_unserved_refusal_on_an_aperture_member_is_reported() {
        let tree = edited("schemas/inputs/exec-start.json", |start| {
            start["properties"]["sandbox"]["properties"]["aperture"]
                .as_object_mut()
                .expect("an object")
                .remove("x-b10x-refusals");
        });
        let failures = failures_of(VERSION, &tree);
        assert_reported(&failures, "sandbox.aperture does not publish");
    }
}
