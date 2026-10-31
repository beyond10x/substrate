//! The pinned contract (design 31 § 3): the advertised pair the runner accepts, and the operation
//! registry and hosted admission document of that bundle, embedded at build time from the frozen
//! directory and never restated in runner code.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

/// The contract pair this runner pins, independently of the wire crate's advertised constant.
pub const PINNED_CONTRACT: &str = "substrate-wire/0.26.0";
/// The bundle digest of [`PINNED_CONTRACT`].
pub const PINNED_SHA256: &str = "f6216ea867d743b8efbfddb86b7097ddcddd261a10dbca3f7a775ba1a6687403";

const OPERATIONS: &str = include_str!("../../../contracts/substrate-wire/0.26.0/operations.json");
const HOSTED_ADMISSION: &str =
    include_str!("../../../contracts/substrate-wire/0.26.0/hosted-admission.json");

#[derive(Debug, Deserialize)]
struct OperationsDocument {
    operations: Vec<Operation>,
}

/// One registry operation, as far as selection reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct Operation {
    pub id: String,
    pub method: String,
    pub path: String,
    pub capability_predicates: Vec<Predicate>,
}

/// One capability predicate. Those with an `input_pointer` hold by construction (§ 3, step 4).
#[derive(Debug, Clone, Deserialize)]
pub struct Predicate {
    pub fact: String,
    pub op: String,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub input_pointer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AdmissionDocument {
    scopes: Vec<AdmissionScope>,
}

#[derive(Debug, Deserialize)]
struct AdmissionScope {
    scope: String,
    route_families: Vec<String>,
}

/// How a predicate or a fact stands in one capability snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactStanding {
    Absent,
    Unsatisfied,
}

/// The embedded registry and admission scopes.
#[derive(Debug)]
pub struct Registry {
    operations: BTreeMap<String, Operation>,
    family_scope: BTreeMap<String, String>,
}

impl Registry {
    /// Parses the embedded documents.
    ///
    /// # Panics
    ///
    /// When an embedded document does not parse, which the crate's unit tests rule out.
    pub fn pinned() -> Self {
        let operations: OperationsDocument =
            serde_json::from_str(OPERATIONS).expect("the embedded operation registry parses");
        let admission: AdmissionDocument =
            serde_json::from_str(HOSTED_ADMISSION).expect("the embedded admission parses");
        let mut family_scope = BTreeMap::new();
        for scope in admission.scopes {
            for family in scope.route_families {
                family_scope.insert(family, scope.scope.clone());
            }
        }
        Self {
            operations: operations
                .operations
                .into_iter()
                .map(|operation| (operation.id.clone(), operation))
                .collect(),
            family_scope,
        }
    }

    pub fn operation(&self, id: &str) -> Option<&Operation> {
        self.operations.get(id)
    }

    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.operations.values()
    }

    /// The hosted scope whose `route_families` lists the operation's family, if any.
    pub fn scope(&self, id: &str) -> Option<&str> {
        let operation = self.operations.get(id)?;
        self.family_scope
            .get(&family(&operation.path)?)
            .map(String::as_str)
    }

    /// Every listed route family.
    #[cfg(test)]
    pub fn families(&self) -> BTreeSet<&str> {
        self.family_scope.keys().map(String::as_str).collect()
    }

    /// The fact-only predicates of an operation that fail in `facts`, by fact name.
    pub fn failing_facts(&self, id: &str, facts: &Value) -> BTreeMap<String, FactStanding> {
        let mut failing = BTreeMap::new();
        let Some(operation) = self.operations.get(id) else {
            return failing;
        };
        for predicate in &operation.capability_predicates {
            if predicate.input_pointer.is_some() {
                continue;
            }
            if let Some(standing) = predicate_standing(predicate, facts) {
                failing
                    .entry(predicate.fact.clone())
                    .and_modify(|current: &mut FactStanding| *current = (*current).min(standing))
                    .or_insert(standing);
            }
        }
        failing
    }
}

/// An operation's route family: the first path segment after the API major, `ops` read as
/// `operations`.
pub fn family(path: &str) -> Option<String> {
    let mut segments = path.trim_start_matches('/').split('/');
    let major = segments.next()?;
    if !(major.starts_with('v') && major[1..].bytes().all(|byte| byte.is_ascii_digit())) {
        return None;
    }
    let first = segments.next().filter(|segment| !segment.is_empty())?;
    Some(if first == "ops" {
        "operations".to_owned()
    } else {
        first.to_owned()
    })
}

/// `None` when a fact-only predicate holds.
fn predicate_standing(predicate: &Predicate, facts: &Value) -> Option<FactStanding> {
    let observed = facts.get(&predicate.fact).filter(|value| !value.is_null());
    let Some(observed) = observed else {
        return Some(FactStanding::Absent);
    };
    let holds = match (predicate.op.as_str(), &predicate.value) {
        ("eq", Some(expected)) => observed == expected,
        ("gte", Some(expected)) => match (observed.as_f64(), expected.as_f64()) {
            (Some(observed), Some(expected)) => observed >= expected,
            _ => false,
        },
        ("one_of", Some(Value::Array(allowed))) => allowed.contains(observed),
        _ => false,
    };
    (!holds).then_some(FactStanding::Unsatisfied)
}

/// How a fact a case needs beyond its operations stands: present and neither `false` nor null
/// holds.
pub fn needed_fact_standing(fact: &str, facts: &Value) -> Option<FactStanding> {
    match facts.get(fact) {
        None | Some(Value::Null) => Some(FactStanding::Absent),
        Some(Value::Bool(false)) => Some(FactStanding::Unsatisfied),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FactStanding, PINNED_CONTRACT, Registry, family};

    #[test]
    fn every_pinned_operation_lands_in_one_listed_family_or_none_and_every_family_is_reached() {
        let registry = Registry::pinned();
        let families = registry.families();
        let mut reached = std::collections::BTreeSet::new();
        for operation in registry.operations() {
            let named = family(&operation.path).expect("every registry path has a family");
            match registry.scope(&operation.id) {
                Some(_) => {
                    assert!(families.contains(named.as_str()));
                    reached.insert(named);
                }
                None => assert!(
                    !families.contains(named.as_str()),
                    "{} is in a listed family without a scope",
                    operation.id
                ),
            }
        }
        assert_eq!(
            reached,
            families.iter().map(|family| (*family).to_owned()).collect()
        );
        assert_eq!(registry.scope("session.start"), Some("exec"));
        assert_eq!(registry.scope("operation.get"), Some("observe"));
        assert_eq!(registry.scope("workspace.file.read-v2"), Some("workspaces"));
        assert_eq!(registry.scope("process.start"), None);
        let process: Vec<_> = registry
            .operations()
            .filter(|operation| operation.id.starts_with("process."))
            .collect();
        assert_eq!(process.len(), 8, "design 31 § 3 step 5 names eight");
        assert!(PINNED_CONTRACT.ends_with("/0.26.0"));
    }

    #[test]
    fn a_family_is_the_first_segment_after_the_api_major() {
        assert_eq!(
            family("/v1/ops/{operation_id}").as_deref(),
            Some("operations")
        );
        assert_eq!(
            family("/v2/workspaces/{id}/tree").as_deref(),
            Some("workspaces")
        );
        assert_eq!(
            family("/v1/reconciliation-snapshots").as_deref(),
            Some("reconciliation-snapshots")
        );
        assert_eq!(family("/healthz"), None);
        assert_eq!(family("/v1"), None);
    }

    #[test]
    fn fact_only_predicates_name_absent_and_unsatisfied_facts() {
        let registry = Registry::pinned();
        let facts = json!({
            "exec.argv-only": true,
            "exec.cgroup-kill": false,
            "exec.output-limit-bytes": 1024,
        });
        let failing = registry.failing_facts("exec.start", &facts);
        assert_eq!(
            failing.get("exec.cgroup-kill"),
            Some(&FactStanding::Unsatisfied)
        );
        assert_eq!(failing.get("exec.namespaces"), Some(&FactStanding::Absent));
        assert!(!failing.contains_key("exec.argv-only"));
        assert!(
            !failing.contains_key("exec.output-limit-bytes"),
            "an input predicate holds by construction"
        );
        assert!(registry.failing_facts("machine.get", &facts).is_empty());
    }
}
