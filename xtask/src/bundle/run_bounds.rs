//! What `0.27.0` exists for (ADR 0013 and ADR 0010, amendments of 2026-11-20, issue #236): three
//! bounds move and nothing else does. One run selects up to `MAX_EGRESS_APERTURES` (8) apertures,
//! one deployment declares up to `MAX_DEPLOYMENT_EGRESS_APERTURES` (12), which therefore bounds
//! the `exec.egress-apertures` fact that lists them, and one start declares up to
//! `MAX_READ_ONLY_ROOTS` (8) read-only roots.
//!
//! Every bundle before `0.27.0` publishes four for each, and keeps its bytes: the authored sources
//! of `0.21.0` to `0.26.0` state that four as a literal, and the checks of those bundles hold them
//! to [`Bounds::of`], whose historical values are named here rather than read from the wire crate
//! the bound has since left. From `0.27.0` on, [`check`] holds every published bound to its wire
//! constant. The bundle states them as literals, because `render_v2.rs` is hashed by every bundle
//! it rendered and cannot learn a new `{"$wire": …}` binding (AGENTS.md § *The gate*); the binding
//! lives here, which no bundle hashes.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const CAPABILITY: &str = "schemas/capability.json";
const COMMON: &str = "schemas/common.json";
const RESOURCE: &str = "schemas/resource.json";
const VECTOR: &str = "schemas/vector.json";
const APPLIED_NETWORK: &str = "/$defs/applied-network/oneOf";
const EGRESS_FACT: &str = "exec.egress-apertures";

/// The two start inputs whose sandbox carries `apertures`, and where each states it.
const STARTS: [(&str, &str); 2] = [
    (
        "schemas/inputs/exec-start.json",
        "/properties/sandbox/properties/apertures",
    ),
    (
        "schemas/inputs/pipe-session-start.json",
        "/properties/exec/properties/sandbox/properties/apertures",
    ),
];

/// The first bundle that publishes the bounds of issue #236; every later one keeps them.
const FIRST: (u64, u64, u64) = (0, 27, 0);

/// The run's aperture bound every bundle from `0.21.0` to `0.26.0` publishes.
pub(super) const RUN_APERTURES_BEFORE_0_27_0: u32 = 4;
/// The `exec.egress-apertures` list bound every bundle from `0.6.0` to `0.26.0` publishes: the run
/// bound, which a deployment declaring more than four contradicted.
pub(super) const FACT_APERTURES_BEFORE_0_27_0: u32 = 4;
/// The read-only root bound every bundle from `0.23.0` to `0.26.0` publishes.
pub(super) const READ_ONLY_ROOTS_BEFORE_0_27_0: u32 = 4;

/// The bounds one released bundle publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bounds {
    /// How many apertures one run's `apertures` list and applied list hold.
    pub(super) run_apertures: u32,
    /// How many entries the `exec.egress-apertures` fact lists.
    pub(super) fact_apertures: u32,
    /// How many read-only roots one start declares, and the fact's `max_roots`.
    pub(super) read_only_roots: u32,
}

impl Bounds {
    /// The bounds `version` publishes: the wire constants from `0.27.0`, four before it.
    pub(super) fn of(version: &str) -> Self {
        if applies_to(version) {
            Self {
                run_apertures: wire::MAX_EGRESS_APERTURES,
                fact_apertures: wire::MAX_DEPLOYMENT_EGRESS_APERTURES,
                read_only_roots: wire::MAX_READ_ONLY_ROOTS,
            }
        } else {
            Self {
                run_apertures: RUN_APERTURES_BEFORE_0_27_0,
                fact_apertures: FACT_APERTURES_BEFORE_0_27_0,
                read_only_roots: READ_ONLY_ROOTS_BEFORE_0_27_0,
            }
        }
    }

    /// The message `exec.apertures-too-many` carries under these bounds. The wire crate's row is
    /// this message at [`wire::MAX_EGRESS_APERTURES`], which a unit test holds.
    pub(super) fn apertures_too_many_message(self) -> String {
        format!(
            "An aperture list names at most {} apertures.",
            self.run_apertures
        )
    }

    /// The `listen` pattern of an applied aperture list: the n-th aperture listens on
    /// `127.0.0.n`, for n from 1 to the run bound. `None` for a bound a one-digit range cannot
    /// state.
    pub(super) fn listen_pattern(self) -> Option<String> {
        if !(1..=9).contains(&self.run_apertures) {
            return None;
        }
        let network = wire::APERTURE_LOOPBACK_ADDRESS
            .rsplit_once('.')
            .map_or("", |(network, _)| network)
            .replace('.', "\\.");
        Some(format!(
            "^{network}\\.[1-{}]:[0-9]{{1,5}}$",
            self.run_apertures
        ))
    }

    /// `roots` judged by the wire crate's rule, with this bundle's bound in place of the wire's:
    /// a list over the bound is [`wire::WireValidationError::InvalidReadOnlyRootBounds`], the
    /// first thing the wire crate checks.
    pub(super) fn validate_read_only_roots(
        self,
        roots: &[wire::ReadOnlyRoot],
    ) -> Result<(), wire::WireValidationError> {
        if roots.len() > self.read_only_roots as usize {
            return Err(wire::WireValidationError::InvalidReadOnlyRootBounds);
        }
        wire::validate_read_only_roots(roots)
    }
}

pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => (major, minor, patch) >= FIRST,
        _ => false,
    }
}

/// The class check, for `0.27.0` and every later bundle: every published bound is its wire
/// constant, a full deployment's snapshot validates and one over it does not, and the vectors
/// that probe each bound name one more than it.
pub(super) fn check(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let bounds = Bounds::of(version);
    check_capability(bounds, released, failures);
    check_vector_machines(bounds, released, failures);
    check_starts(bounds, released, failures);
    check_resource(bounds, released, failures);
    check_snapshot_validates(version, bounds, released, failures);
    check_bound_vectors(bounds, released, failures);
}

/// What only `0.27.0` states: its client-visible changes.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
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
    let run = format!("up to {}", wire::MAX_EGRESS_APERTURES);
    let deployment = format!("({})", wire::MAX_DEPLOYMENT_EGRESS_APERTURES);
    let roots = format!("up to {}", wire::MAX_READ_ONLY_ROOTS);
    let named = [
        "0.26.0",
        "#236",
        "MAX_EGRESS_APERTURES",
        "MAX_DEPLOYMENT_EGRESS_APERTURES",
        "MAX_READ_ONLY_ROOTS",
        EGRESS_FACT,
        wire::EXEC_READ_ONLY_ROOTS_FACT,
        wire::EXEC_APERTURES_TOO_MANY,
        wire::READ_ONLY_ROOT_INVALID,
        run.as_str(),
        deployment.as_str(),
        roots.as_str(),
    ];
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

/// Calls `each` with every object in `value`, depth first.
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

/// Every `properties.<member>` in `document`, with where it is, so a bound stated on any shape of
/// the member is found.
fn members<'a>(document: &'a Value, member: &str) -> Vec<(String, &'a Value)> {
    let mut found = Vec::new();
    visit(document, "", &mut |path, value| {
        if let Some(schema) = value
            .get("properties")
            .and_then(|properties| properties.get(member))
        {
            found.push((format!("{path}/properties/{member}"), schema));
        }
    });
    found
}

fn require_max_items(
    path: &str,
    pointer: &str,
    schema: &Value,
    bound: u32,
    constant: &str,
    failures: &mut Vec<String>,
) {
    if schema.get("maxItems") != Some(&json!(bound)) {
        failures.push(format!(
            "{path}: {pointer} maxItems is {}, and substrate_wire::{constant} is {bound}",
            schema.get("maxItems").unwrap_or(&Value::Null)
        ));
    }
}

/// The fact list is bounded by the deployment bound, and `max_roots` is the root bound.
fn check_capability(bounds: Bounds, released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let facts = capability.pointer("/properties/facts/properties");
    match facts.and_then(|facts| facts.get(EGRESS_FACT)) {
        None => failures.push(format!("{CAPABILITY}: {EGRESS_FACT} is not declared")),
        Some(fact) => require_max_items(
            CAPABILITY,
            EGRESS_FACT,
            fact,
            bounds.fact_apertures,
            "MAX_DEPLOYMENT_EGRESS_APERTURES",
            failures,
        ),
    }
    let published = serde_json::to_value(wire::ReadOnlyRootFacts {
        max_roots: bounds.read_only_roots,
    })
    .unwrap_or(Value::Null);
    if facts
        .and_then(|facts| facts.get(wire::EXEC_READ_ONLY_ROOTS_FACT))
        .and_then(|fact| fact.pointer("/properties/max_roots/const"))
        != published.get("max_roots")
    {
        failures.push(format!(
            "{CAPABILITY}: {} max_roots must be the const substrate_wire::MAX_READ_ONLY_ROOTS, {}",
            wire::EXEC_READ_ONLY_ROOTS_FACT,
            bounds.read_only_roots
        ));
    }
}

/// A vector's machine may list as many declared apertures as a snapshot may.
fn check_vector_machines(bounds: Bounds, released: &Tree, failures: &mut Vec<String>) {
    let Some(vector) = json_at(released, VECTOR, failures) else {
        return;
    };
    let found = members(&vector, EGRESS_FACT);
    if found.is_empty() {
        failures.push(format!(
            "{VECTOR}: a vector's machine facts cannot carry {EGRESS_FACT}"
        ));
    }
    for (pointer, schema) in found {
        require_max_items(
            VECTOR,
            &pointer,
            schema,
            bounds.fact_apertures,
            "MAX_DEPLOYMENT_EGRESS_APERTURES",
            failures,
        );
    }
}

/// Both starts select up to the run bound, and declare up to the root bound.
fn check_starts(bounds: Bounds, released: &Tree, failures: &mut Vec<String>) {
    for (path, pointer) in STARTS {
        let Some(input) = json_at(released, path, failures) else {
            continue;
        };
        match input.pointer(pointer) {
            None => failures.push(format!("{path}: no apertures list at {pointer}")),
            Some(apertures) => require_max_items(
                path,
                pointer,
                apertures,
                bounds.run_apertures,
                "MAX_EGRESS_APERTURES",
                failures,
            ),
        }
    }
    let Some(common) = json_at(released, COMMON, failures) else {
        return;
    };
    match common.pointer("/$defs/read-only-roots") {
        None => failures.push(format!("{COMMON}: no read-only-roots definition")),
        Some(list) => require_max_items(
            COMMON,
            "/$defs/read-only-roots",
            list,
            bounds.read_only_roots,
            "MAX_READ_ONLY_ROOTS",
            failures,
        ),
    }
}

/// Every requested `apertures`, the applied list and every applied `read_only_roots` in the
/// resource schema state the wire bounds, and the applied list listens on exactly the addresses
/// a run that long is given.
fn check_resource(bounds: Bounds, released: &Tree, failures: &mut Vec<String>) {
    let Some(resource) = json_at(released, RESOURCE, failures) else {
        return;
    };
    for (member, bound, constant) in [
        ("apertures", bounds.run_apertures, "MAX_EGRESS_APERTURES"),
        (
            "read_only_roots",
            bounds.read_only_roots,
            "MAX_READ_ONLY_ROOTS",
        ),
    ] {
        let found = members(&resource, member);
        if found.is_empty() {
            failures.push(format!("{RESOURCE}: no shape states {member}"));
        }
        for (pointer, schema) in found {
            require_max_items(RESOURCE, &pointer, schema, bound, constant, failures);
        }
    }
    let list = resource
        .pointer(APPLIED_NETWORK)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|form| form.get("type") == Some(&json!("array")));
    let Some(list) = list else {
        failures.push(format!("{RESOURCE}: applied-network has no list form"));
        return;
    };
    require_max_items(
        RESOURCE,
        "applied-network list",
        list,
        bounds.run_apertures,
        "MAX_EGRESS_APERTURES",
        failures,
    );
    match bounds.listen_pattern() {
        None => failures.push(format!(
            "{RESOURCE}: no one-digit listen pattern states {} loopback addresses",
            bounds.run_apertures
        )),
        Some(pattern) => {
            if list.pointer("/items/properties/listen/pattern") != Some(&json!(pattern)) {
                failures.push(format!(
                    "{RESOURCE}: the applied list's listen pattern is not {pattern}"
                ));
            }
        }
    }
}

/// `count` declared apertures as the wire crate publishes them in the fact.
fn declared_apertures(count: u32) -> Value {
    let facts: Vec<wire::EgressApertureFact> = (0..count)
        .map(|index| wire::EgressApertureFact {
            name: format!("aperture_{index}"),
            destination: format!("203.0.113.{}:443", index + 1),
            max_bytes: (index % 2 == 0).then_some(1_048_576),
        })
        .collect();
    serde_json::to_value(facts).unwrap_or(Value::Null)
}

/// A machine whose deployment declared the most apertures it may is a snapshot the published
/// capability schema accepts, with a root fact at its bound; one more aperture is not.
fn check_snapshot_validates(
    version: &str,
    bounds: Bounds,
    released: &Tree,
    failures: &mut Vec<String>,
) {
    let probe = "vectors/http/machine-probe.json";
    let Some(vector) = json_at(released, probe, failures) else {
        return;
    };
    let Some(snapshot) = vector.pointer("/expected/response/body/result") else {
        failures.push(format!("{probe}: no capability snapshot result"));
        return;
    };
    let Some(schema) = super::read_only_roots::validator(version, released, CAPABILITY, failures)
    else {
        return;
    };
    let with = |count: u32| {
        let mut snapshot = snapshot.clone();
        snapshot["facts"][EGRESS_FACT] = declared_apertures(count);
        snapshot["facts"][wire::EXEC_READ_ONLY_ROOTS_FACT] = json!(wire::ReadOnlyRootFacts {
            max_roots: bounds.read_only_roots,
        });
        snapshot
    };
    if let Err(error) = schema.validate(&with(bounds.fact_apertures)) {
        failures.push(format!(
            "{CAPABILITY}: rejects the snapshot of a deployment declaring {} apertures, \
             substrate_wire::MAX_DEPLOYMENT_EGRESS_APERTURES: {error} at {}",
            bounds.fact_apertures,
            error.instance_path()
        ));
    }
    if schema.is_valid(&with(bounds.fact_apertures + 1)) {
        failures.push(format!(
            "{CAPABILITY}: accepts a snapshot declaring {} apertures, over \
             substrate_wire::MAX_DEPLOYMENT_EGRESS_APERTURES",
            bounds.fact_apertures + 1
        ));
    }
}

/// The vector that probes each lexical bound names exactly one more than it, the wire crate
/// refuses that list by the bound, and every machine stating the root fact states the bound.
fn check_bound_vectors(bounds: Bounds, released: &Tree, failures: &mut Vec<String>) {
    let mut probed: BTreeSet<&str> = BTreeSet::new();
    let too_many = bounds.apertures_too_many_message();
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        for machine in vector
            .get("setup")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(fact) =
                machine.pointer(&format!("/state/facts/{}", wire::EXEC_READ_ONLY_ROOTS_FACT))
                && fact != &json!({ "max_roots": bounds.read_only_roots })
            {
                failures.push(format!(
                    "{path}: a machine states {} {fact}, not max_roots {}",
                    wire::EXEC_READ_ONLY_ROOTS_FACT,
                    bounds.read_only_roots
                ));
            }
        }
        if vector.pointer("/action/operation") != Some(&json!("exec.start")) {
            continue;
        }
        let input = vector.pointer("/action/request/body/input");
        let error = vector.pointer("/expected/response/body/error");
        let code = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str);
        if code == Some(wire::EXEC_APERTURES_TOO_MANY) {
            let listed = input
                .and_then(|input| input.pointer("/sandbox/apertures"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if listed != bounds.run_apertures as usize + 1 {
                failures.push(format!(
                    "{path}: {} is probed with {listed} apertures, not one over \
                     substrate_wire::MAX_EGRESS_APERTURES ({})",
                    wire::EXEC_APERTURES_TOO_MANY,
                    bounds.run_apertures
                ));
            }
            if error.and_then(|error| error.get("message")) != Some(&json!(too_many)) {
                failures.push(format!(
                    "{path}: {} must carry the wire message {too_many:?}",
                    wire::EXEC_APERTURES_TOO_MANY
                ));
            }
            probed.insert("apertures");
        }
        if code == Some(wire::READ_ONLY_ROOT_INVALID) {
            let roots = input
                .and_then(|input| input.get("read_only_roots"))
                .cloned()
                .and_then(|roots| serde_json::from_value::<Vec<wire::ReadOnlyRoot>>(roots).ok())
                .unwrap_or_default();
            if bounds.validate_read_only_roots(&roots)
                == Err(wire::WireValidationError::InvalidReadOnlyRootBounds)
            {
                if roots.len() == bounds.read_only_roots as usize + 1 {
                    probed.insert("read-only roots");
                } else {
                    failures.push(format!(
                        "{path}: the read-only root bound is probed with {} roots, not one over \
                         substrate_wire::MAX_READ_ONLY_ROOTS ({})",
                        roots.len(),
                        bounds.read_only_roots
                    ));
                }
            }
        }
    }
    for bound in ["apertures", "read-only roots"] {
        if !probed.contains(bound) {
            failures.push(format!(
                "vectors: no exec.start vector probes the {bound} bound one over it"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use substrate_wire as wire;

    use super::Bounds;
    use crate::repo;

    const VERSION: &str = "0.27.0";

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

    /// Issue #236: the bounds the wire crate holds from `0.27.0`, and the four every earlier
    /// bundle keeps.
    #[test]
    fn each_bundle_is_held_to_the_bounds_it_published() {
        assert_eq!(
            Bounds::of("0.27.0"),
            Bounds {
                run_apertures: 8,
                fact_apertures: 12,
                read_only_roots: 8,
            }
        );
        for version in ["0.6.0", "0.21.0", "0.23.0", "0.26.0"] {
            assert_eq!(
                Bounds::of(version),
                Bounds {
                    run_apertures: 4,
                    fact_apertures: 4,
                    read_only_roots: 4,
                },
                "{version}"
            );
        }
    }

    /// The message the wire crate sends is the one [`Bounds`] computes at the wire bound, so a
    /// bundle stating another bound is held to the message it states.
    #[test]
    fn the_wire_message_states_the_wire_bound() {
        let row = wire::APERTURE_LIST_REFUSALS
            .iter()
            .find(|refusal| refusal.code == wire::EXEC_APERTURES_TOO_MANY)
            .expect("the row");
        assert_eq!(
            row.message,
            Bounds::of(VERSION).apertures_too_many_message()
        );
        assert_eq!(
            Bounds::of("0.26.0").apertures_too_many_message(),
            "An aperture list names at most 4 apertures."
        );
    }

    #[test]
    fn the_successor_carries_the_wire_bounds() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The advertised bundle admits the snapshot of a deployment at the wire's declaration bound:
    /// `0.26.0` bounds the fact list by four and refuses one declaring five.
    #[test]
    fn the_advertised_bundle_admits_a_full_deployment() {
        let version = wire::ADVERTISED_CONTRACT_BUNDLE
            .rsplit_once('/')
            .map(|(_, version)| version)
            .expect("a versioned bundle");
        let mut failures = Vec::new();
        super::check_snapshot_validates(
            version,
            Bounds::of(VERSION),
            &released(version),
            &mut failures,
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn the_predecessor_carries_the_old_bounds_only() {
        let mut failures = Vec::new();
        super::check(VERSION, &released("0.26.0"), &mut failures);
        assert_reported(&failures, "MAX_DEPLOYMENT_EGRESS_APERTURES");
        assert_reported(&failures, "MAX_EGRESS_APERTURES");
        assert_reported(&failures, "MAX_READ_ONLY_ROOTS");
        let mut failures = Vec::new();
        super::check("0.26.0", &released("0.26.0"), &mut failures);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_bound_that_is_not_the_wire_constant_is_reported() {
        let tree = edited("schemas/capability.json", |capability| {
            capability["properties"]["facts"]["properties"]["exec.egress-apertures"]["maxItems"] =
                json!(wire::MAX_EGRESS_APERTURES);
        });
        assert_reported(&failures_of(&tree), "MAX_DEPLOYMENT_EGRESS_APERTURES is 12");
        let tree = edited("schemas/resource.json", |resource| {
            resource["$defs"]["confinement-request"]["properties"]["apertures"]["maxItems"] =
                json!(4);
        });
        assert_reported(&failures_of(&tree), "MAX_EGRESS_APERTURES is 8");
        let tree = edited("schemas/resource.json", |resource| {
            resource["$defs"]["confinement-applied"]["oneOf"][0]["properties"]["read_only_roots"]
                ["maxItems"] = json!(4);
        });
        assert_reported(&failures_of(&tree), "MAX_READ_ONLY_ROOTS is 8");
        let tree = edited("schemas/inputs/pipe-session-start.json", |start| {
            start["properties"]["exec"]["properties"]["sandbox"]["properties"]["apertures"]["maxItems"] =
                json!(4);
        });
        assert_reported(&failures_of(&tree), "pipe-session-start.json");
        let tree = edited("schemas/capability.json", |capability| {
            capability["properties"]["facts"]["properties"]["exec.read-only-roots"]["properties"]
                ["max_roots"]["const"] = json!(4);
        });
        assert_reported(&failures_of(&tree), "max_roots must be the const");
    }

    #[test]
    fn a_listen_pattern_short_of_the_run_bound_is_reported() {
        let tree = edited("schemas/resource.json", |resource| {
            resource["$defs"]["applied-network"]["oneOf"][2]["items"]["properties"]["listen"]["pattern"] =
                json!("^127\\.0\\.0\\.[1-4]:[0-9]{1,5}$");
        });
        assert_reported(&failures_of(&tree), "listen pattern");
    }

    #[test]
    fn a_bound_vector_that_is_not_one_over_the_bound_is_reported() {
        let tree = edited(
            "vectors/http/apertures-list-of-nine-is-refused.json",
            |vector| {
                vector["action"]["request"]["body"]["input"]["sandbox"]["apertures"]
                    .as_array_mut()
                    .expect("a list")
                    .push(json!("tenth"));
            },
        );
        assert_reported(&failures_of(&tree), "probed with 10 apertures");
        let mut tree = released(VERSION);
        tree.remove("vectors/http/read-only-roots-nine-is-refused.json");
        assert_reported(&failures_of(&tree), "probes the read-only roots bound");
        let tree = edited(
            "vectors/http/read-only-roots-applied-are-observed.json",
            |vector| {
                vector["setup"][0]["state"]["facts"]["exec.read-only-roots"] =
                    json!({ "max_roots": 4 });
            },
        );
        assert_reported(&failures_of(&tree), "not max_roots 8");
    }

    #[test]
    fn the_compatibility_block_states_each_bound() {
        let tree = edited("compatibility.json", |compatibility| {
            compatibility["client_visible_changes"] = json!(["0.27.0 adds no route."]);
        });
        let failures = failures_of(&tree);
        for named in [
            "MAX_DEPLOYMENT_EGRESS_APERTURES",
            "MAX_READ_ONLY_ROOTS",
            "up to 8",
            "(12)",
        ] {
            assert_reported(&failures, named);
        }
    }
}
