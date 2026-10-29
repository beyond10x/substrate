//! What `0.25.0` exists for (ADR 0047 decision 12, design 34 § 11): a second `driver` word, the
//! mechanism-neutral `confinement.no-egress` fact, and three refusals a driver answers by name for
//! a request member or a route it does not serve.
//!
//! Every value this check expects comes from `substrate_wire`: the driver words, the fact name and
//! each refusal's operation, code, address and message. The bundle states them as literals, because
//! `render_v2.rs` is hashed by every bundle it rendered and cannot learn a new `{"$wire": …}`
//! binding (AGENTS.md § *The gate*); the binding lives here, which no bundle hashes.

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const CAPABILITY: &str = "schemas/capability.json";

/// The Linux mechanism facts a namespace driver never publishes (ADR 0047 decision 11).
const LINUX_FACTS: [&str; 4] = [
    "exec.namespaces",
    "exec.cgroup-limits",
    "exec.cgroup-kill",
    "exec.no-egress",
];

/// The fact whose presence serves an inline capsule; `exec.inline-capsule-unserved` answers its
/// absence. The wire crate names it only as the serde rename on `CapabilityFacts`.
const INLINE_CAPSULE_FACT: &str = "exec.inline-capsule";

/// The published signal set; `exec.signal-unserved` answers a signal outside it.
const SIGNALS_FACT: &str = "exec.signals";

/// The first bundle that publishes design 34 § 11; every later one keeps it.
const FIRST: (u64, u64, u64) = (0, 25, 0);

pub(super) fn applies_to(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => (major, minor, patch) >= FIRST,
        _ => false,
    }
}

/// Every driver word the wire crate can emit. The match does not compile when a variant is added,
/// so `HostDriverKind::ALL` cannot fall behind without this function being revisited.
fn driver_words() -> Vec<Value> {
    wire::HostDriverKind::ALL
        .into_iter()
        .map(|kind| {
            match kind {
                wire::HostDriverKind::Host | wire::HostDriverKind::Kubernetes => {}
            }
            serde_json::to_value(kind).unwrap_or(Value::Null)
        })
        .collect()
}

/// The class check, for `0.25.0` and every later bundle.
pub(super) fn check(released: &Tree, failures: &mut Vec<String>) {
    check_driver(released, failures);
    check_fact(released, failures);
    for refusal in wire::DRIVER_UNSERVED_REFUSALS {
        check_refusal_vector(released, &refusal, failures);
    }
    check_machines(released, failures);
}

/// What only the bundle that introduced design 34 § 11 states: its client-visible changes.
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
    let mut named = vec!["0.24.0", "kubernetes", wire::CONFINEMENT_NO_EGRESS_FACT];
    named.extend(wire::DRIVER_UNSERVED_REFUSALS.map(|refusal| refusal.code));
    for named in named {
        if !changes.iter().any(|change| change.contains(named)) {
            failures.push(format!("{path}: client_visible_changes must state {named}"));
        }
    }
}

/// N7 A: `driver` is exactly the closed set of words the wire crate serializes.
fn check_driver(released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let expected = json!({ "enum": driver_words() });
    if capability.pointer("/properties/driver") != Some(&expected) {
        failures.push(format!(
            "{CAPABILITY}: driver must be exactly {expected}, every HostDriverKind word"
        ));
    }
}

/// N5 A: the fact is an optional `const: true`, published only from an observation.
fn check_fact(released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, CAPABILITY, failures) else {
        return;
    };
    let name = wire::CONFINEMENT_NO_EGRESS_FACT;
    let Some(fact) = capability
        .pointer("/properties/facts/properties")
        .and_then(|facts| facts.get(name))
    else {
        failures.push(format!("{CAPABILITY}: facts do not declare {name}"));
        return;
    };
    if fact.get("const") != Some(&json!(true)) {
        failures.push(format!(
            "{CAPABILITY}: {name} must be const true, never false"
        ));
    }
    if fact.get("x-b10x-published-only-from")
        != Some(&json!("a-verified-mechanism-never-configuration"))
    {
        failures.push(format!(
            "{CAPABILITY}: {name} must be published only from a verified mechanism"
        ));
    }
    let required = capability
        .pointer("/properties/facts/required")
        .and_then(Value::as_array)
        .is_some_and(|required| required.contains(&json!(name)));
    if required {
        failures.push(format!("{CAPABILITY}: {name} must be optional"));
    }
}

/// One vector answers exactly the row: the operation, 501, `unserved`, the code, the wire message,
/// the address (or none), not retriable, and no driver dispatch.
fn check_refusal_vector(
    released: &Tree,
    refusal: &wire::UnservedRefusal,
    failures: &mut Vec<String>,
) {
    let mut found = 0_usize;
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let error = vector.pointer("/expected/response/body/error");
        if error.and_then(|error| error.get("code")) != Some(&json!(refusal.code)) {
            continue;
        }
        found += 1;
        let mut expected = json!({
            "class": wire::UnservedRefusal::CLASS,
            "code": refusal.code,
            "message": refusal.message,
            "retriable": false,
        });
        if let Some(address) = refusal.address {
            expected["address"] = json!(address);
        }
        if let (Some(operation), Some(object)) = (
            error.and_then(|error| error.get("operation")),
            expected.as_object_mut(),
        ) {
            object.insert("operation".to_owned(), operation.clone());
        }
        if error != Some(&expected) {
            failures.push(format!(
                "{path}: {} must be answered exactly {expected}",
                refusal.code
            ));
        }
        if vector.pointer("/action/operation") != Some(&json!(refusal.operation)) {
            failures.push(format!(
                "{path}: {} is answered for {}, not {}",
                refusal.code,
                vector.pointer("/action/operation").unwrap_or(&Value::Null),
                refusal.operation
            ));
        }
        if vector.pointer("/expected/response/status")
            != Some(&json!(wire::UnservedRefusal::STATUS))
        {
            failures.push(format!(
                "{path}: {} must be answered {}",
                refusal.code,
                wire::UnservedRefusal::STATUS
            ));
        }
        // The request is one the wire crate admits: a refusal vector whose input the daemon would
        // reject earlier as malformed proves nothing about this code. The capsule is checked whole,
        // manifest digest included.
        let input = vector.pointer("/action/request/body/input").cloned();
        let admitted = match refusal.operation {
            "exec.start" => input
                .and_then(|input| serde_json::from_value::<wire::ExecStartInput>(input).ok())
                .is_some_and(|start| {
                    start
                        .capsule
                        .as_ref()
                        .is_some_and(|capsule| wire::validate_execution_capsule(capsule).is_ok())
                }),
            "exec.signal" => input
                .and_then(|input| serde_json::from_value::<wire::ExecSignalInput>(input).ok())
                .is_some(),
            "workspace.adopt" => input
                .and_then(|input| serde_json::from_value::<wire::WorkspaceAdoptInput>(input).ok())
                .is_some(),
            _ => false,
        };
        if !admitted {
            failures.push(format!(
                "{path}: {} is answered for a request the wire crate does not admit",
                refusal.code
            ));
        }
        let undispatched = vector
            .get("postconditions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|condition| {
                condition.get("actual") == Some(&json!("/driver/dispatch_count"))
                    && condition.get("operator") == Some(&json!("equals"))
                    && condition.get("expected") == Some(&json!(0))
            });
        if !undispatched {
            failures.push(format!(
                "{path}: {} must state that nothing was dispatched",
                refusal.code
            ));
        }
        check_answering_machines(path, &vector, refusal, failures);
    }
    if found != 1 {
        failures.push(format!(
            "vectors: {} must be answered by exactly one vector, found {found}",
            refusal.code
        ));
    }
}

/// The machine a refusal vector sets up is the driver the row describes: at least one, every one
/// `kubernetes` (the host driver publishes none of the three codes and its answers do not change),
/// and each without what would make the request served. A row added to
/// `DRIVER_UNSERVED_REFUSALS` without a condition here is reported rather than passed.
fn check_answering_machines(
    path: &str,
    vector: &Value,
    refusal: &wire::UnservedRefusal,
    failures: &mut Vec<String>,
) {
    let code = refusal.code;
    let kubernetes = serde_json::to_value(wire::HostDriverKind::Kubernetes).unwrap_or(Value::Null);
    let machines: Vec<&Value> = vector
        .get("setup")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|step| step.get("kind") == Some(&json!("machine")))
        .map(|step| step.get("state").unwrap_or(&Value::Null))
        .collect();
    if machines.is_empty() {
        failures.push(format!("{path}: {code} sets up no machine to answer it"));
    }
    let signal = vector.pointer("/action/request/body/input/signal");
    for machine in machines {
        let driver = machine.get("driver").unwrap_or(&Value::Null);
        if driver != &kubernetes {
            failures.push(format!(
                "{path}: {code} is answered by a {} machine; only {kubernetes} answers it",
                driver.as_str().unwrap_or("driverless")
            ));
        }
        let facts = machine.get("facts");
        let fact = |name: &str| facts.and_then(|facts| facts.get(name));
        if code == wire::EXEC_INLINE_CAPSULE_UNSERVED {
            if fact(INLINE_CAPSULE_FACT).is_some() {
                failures.push(format!(
                    "{path}: {code} is answered by a machine that publishes {INLINE_CAPSULE_FACT}"
                ));
            }
        } else if code == wire::EXEC_SIGNAL_UNSERVED {
            let published = fact(SIGNALS_FACT)
                .and_then(Value::as_array)
                .is_some_and(|signals| signal.is_some_and(|signal| signals.contains(signal)));
            if signal.is_none() || published {
                failures.push(format!(
                    "{path}: {code} is answered for a signal the machine publishes in {SIGNALS_FACT}"
                ));
            }
        } else if code != wire::WORKSPACE_ADOPT_UNSERVED {
            failures.push(format!(
                "{path}: {code} has no machine condition in this check; state what serves it"
            ));
        }
    }
}

/// Every machine a vector sets up names a word the wire crate emits, and a `kubernetes` machine
/// states none of the Linux mechanism facts: those name mechanisms it does not use.
fn check_machines(released: &Tree, failures: &mut Vec<String>) {
    let words = driver_words();
    for (path, bytes) in released {
        if !path.starts_with("vectors/") {
            continue;
        }
        let Ok(vector) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        let machines = vector
            .get("setup")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|step| step.get("kind") == Some(&json!("machine")));
        for machine in machines {
            let driver = machine.pointer("/state/driver").unwrap_or(&Value::Null);
            if !words.contains(driver) {
                failures.push(format!(
                    "{path}: a machine names driver {driver}, which no HostDriverKind emits"
                ));
            }
            if driver != &json!("kubernetes") {
                continue;
            }
            for fact in LINUX_FACTS {
                if machine
                    .pointer("/state/facts")
                    .and_then(|facts| facts.get(fact))
                    .is_some()
                {
                    failures.push(format!(
                        "{path}: a kubernetes machine states the Linux fact {fact}"
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::repo;

    const VERSION: &str = "0.25.0";

    fn released() -> super::Tree {
        super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire")
                .join(VERSION),
        )
        .expect("the bundle reads")
    }

    fn failures_of(tree: &super::Tree) -> Vec<String> {
        let mut failures = Vec::new();
        super::super::check_additions(VERSION, tree, &mut failures);
        failures
    }

    fn edited(path: &str, edit: impl FnOnce(&mut serde_json::Value)) -> super::Tree {
        let mut tree = released();
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

    const SIGNAL: &str = "vectors/http/exec-signal-outside-the-published-set-is-unserved.json";
    const CAPSULE: &str = "vectors/http/inline-capsule-unserved-without-the-fact.json";
    const ADOPT: &str = "vectors/http/workspace-adopt-unserved-by-the-driver.json";

    #[test]
    fn the_successor_carries_the_namespace_driver_additions() {
        let failures = failures_of(&released());
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// A check that passes on the tree that predates the change proves nothing: `0.24.0` pins
    /// `driver` to `host` and publishes none of the three codes.
    #[test]
    fn the_predecessor_does_not_carry_them() {
        let mut failures = Vec::new();
        let predecessor = super::super::tree_of(
            &repo::root()
                .expect("root")
                .join("contracts/substrate-wire/0.24.0"),
        )
        .expect("the bundle reads");
        super::check(&predecessor, &mut failures);
        assert_reported(&failures, "driver must be exactly");
        assert_reported(&failures, "facts do not declare confinement.no-egress");
        for code in substrate_wire::DRIVER_UNSERVED_REFUSALS.map(|refusal| refusal.code) {
            assert_reported(
                &failures,
                &format!("{code} must be answered by exactly one"),
            );
        }
    }

    #[test]
    fn a_driver_pinned_back_to_host_is_refused() {
        let tree = edited(super::CAPABILITY, |capability| {
            capability["properties"]["driver"] = serde_json::json!({ "const": "host" });
        });
        assert_reported(&failures_of(&tree), "driver must be exactly");
    }

    #[test]
    fn a_fact_that_can_be_false_is_refused() {
        let tree = edited(super::CAPABILITY, |capability| {
            capability["properties"]["facts"]["properties"]["confinement.no-egress"] =
                serde_json::json!({ "type": "boolean" });
        });
        assert_reported(&failures_of(&tree), "must be const true");
    }

    #[test]
    fn a_required_fact_is_refused() {
        let tree = edited(super::CAPABILITY, |capability| {
            capability["properties"]["facts"]["required"]
                .as_array_mut()
                .expect("required")
                .push("confinement.no-egress".into());
        });
        assert_reported(&failures_of(&tree), "must be optional");
    }

    #[test]
    fn a_message_other_than_the_wire_crates_is_refused() {
        let tree = edited(SIGNAL, |vector| {
            vector["expected"]["response"]["body"]["error"]["message"] = "Not here.".into();
        });
        assert_reported(
            &failures_of(&tree),
            "exec.signal-unserved must be answered exactly",
        );
    }

    #[test]
    fn an_address_on_the_route_refusal_is_refused() {
        let tree = edited(ADOPT, |vector| {
            vector["expected"]["response"]["body"]["error"]["address"] = "root_name".into();
        });
        assert_reported(
            &failures_of(&tree),
            "workspace.adopt-unserved must be answered exactly",
        );
    }

    #[test]
    fn a_refusal_answered_501_by_another_status_is_refused() {
        let tree = edited(CAPSULE, |vector| {
            vector["expected"]["response"]["status"] = 422.into();
        });
        assert_reported(&failures_of(&tree), "must be answered 501");
    }

    #[test]
    fn a_capsule_whose_manifest_digest_is_wrong_is_refused() {
        let tree = edited(CAPSULE, |vector| {
            vector["action"]["request"]["body"]["input"]["capsule"]["manifest_sha256"] =
                "0".repeat(64).into();
        });
        assert_reported(
            &failures_of(&tree),
            "a request the wire crate does not admit",
        );
    }

    #[test]
    fn a_dispatched_refusal_is_refused() {
        let tree = edited(CAPSULE, |vector| {
            for condition in vector["postconditions"].as_array_mut().expect("conditions") {
                if condition["actual"] == "/driver/dispatch_count" {
                    condition["expected"] = 1.into();
                }
            }
        });
        assert_reported(
            &failures_of(&tree),
            "must state that nothing was dispatched",
        );
    }

    #[test]
    fn a_missing_vector_is_refused() {
        let mut tree = released();
        tree.remove(ADOPT);
        assert_reported(
            &failures_of(&tree),
            "workspace.adopt-unserved must be answered by exactly one vector, found 0",
        );
    }

    #[test]
    fn a_kubernetes_machine_with_a_linux_fact_is_refused() {
        let tree = edited(SIGNAL, |vector| {
            for step in vector["setup"].as_array_mut().expect("setup") {
                if step["kind"] == "machine" {
                    step["state"]["facts"]["exec.no-egress"] = true.into();
                }
            }
        });
        assert_reported(&failures_of(&tree), "states the Linux fact exec.no-egress");
    }

    fn machines_of(vector: &mut serde_json::Value) -> Vec<&mut serde_json::Value> {
        vector["setup"]
            .as_array_mut()
            .expect("setup")
            .iter_mut()
            .filter(|step| step["kind"] == "machine")
            .map(|step| &mut step["state"])
            .collect()
    }

    /// "The host driver publishes none of them": a host machine answering any of the three rows
    /// contradicts the row, whichever vector carries it.
    #[test]
    fn a_host_machine_answering_any_refusal_is_refused() {
        for (path, code) in [
            (CAPSULE, "exec.inline-capsule-unserved"),
            (SIGNAL, "exec.signal-unserved"),
            (ADOPT, "workspace.adopt-unserved"),
        ] {
            let tree = edited(path, |vector| {
                for state in machines_of(vector) {
                    state["driver"] = "host".into();
                }
            });
            assert_reported(
                &failures_of(&tree),
                &format!("{code} is answered by a host machine"),
            );
        }
    }

    /// A refusal vector with no machine states nothing about the driver that answers it, so the
    /// row's condition on that driver goes unchecked.
    #[test]
    fn a_refusal_vector_without_a_machine_is_refused() {
        let tree = edited(ADOPT, |vector| {
            vector["setup"]
                .as_array_mut()
                .expect("setup")
                .retain(|step| step["kind"] != "machine");
        });
        assert_reported(
            &failures_of(&tree),
            "workspace.adopt-unserved sets up no machine",
        );
    }

    #[test]
    fn a_signal_refusal_for_a_published_signal_is_refused() {
        let tree = edited(SIGNAL, |vector| {
            let signal = vector["action"]["request"]["body"]["input"]["signal"].clone();
            for state in machines_of(vector) {
                state["facts"]["exec.signals"]
                    .as_array_mut()
                    .expect("exec.signals")
                    .push(signal.clone());
            }
        });
        assert_reported(
            &failures_of(&tree),
            "exec.signal-unserved is answered for a signal the machine publishes",
        );
    }

    #[test]
    fn a_capsule_refusal_on_a_machine_with_the_inline_capsule_fact_is_refused() {
        let tree = edited(CAPSULE, |vector| {
            for state in machines_of(vector) {
                state["facts"]["exec.inline-capsule"] = serde_json::json!({});
            }
        });
        assert_reported(
            &failures_of(&tree),
            "exec.inline-capsule-unserved is answered by a machine that publishes exec.inline-capsule",
        );
    }

    #[test]
    fn a_machine_naming_an_unknown_driver_is_refused() {
        let tree = edited(ADOPT, |vector| {
            for step in vector["setup"].as_array_mut().expect("setup") {
                if step["kind"] == "machine" {
                    step["state"]["driver"] = "pod".into();
                }
            }
        });
        assert_reported(&failures_of(&tree), "which no HostDriverKind emits");
    }

    #[test]
    fn a_compatibility_block_that_omits_a_code_is_refused() {
        let tree = edited("compatibility.json", |compatibility| {
            for change in compatibility["client_visible_changes"]
                .as_array_mut()
                .expect("changes")
            {
                let text = change
                    .as_str()
                    .expect("text")
                    .replace("exec.signal-unserved", "x");
                *change = text.into();
            }
        });
        assert_reported(
            &failures_of(&tree),
            "client_visible_changes must state exec.signal-unserved",
        );
    }
}
