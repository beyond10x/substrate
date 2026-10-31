//! The namespace's `NetworkPolicy` set (design 34 § 6.1 step 4, § 6.3, § 14 Q1 B): the operator's
//! default-deny policy must deny everything, and the whole set is bound into the snapshot, so an
//! added or changed policy, an additive allow above all, moves `config_generation`.

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// One read of the namespace's policies: the default-deny policy's identity and the digest of the
/// whole set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySet {
    /// The default-deny policy's `metadata.uid`.
    pub default_deny_uid: String,
    /// Its `metadata.resourceVersion`.
    pub default_deny_resource_version: String,
    /// Lowercase hex SHA-256 of its canonical `spec`.
    pub default_deny_spec: String,
    /// Lowercase hex SHA-256 of the canonical list of every policy's name, uid,
    /// `resourceVersion` and `spec`, sorted by name.
    pub set_digest: String,
    /// The name of every other policy in the set that allows egress, sorted: its `egress` is
    /// present and not an empty list. Which Pods it selects is not read; any one of them withholds
    /// `confinement.no-egress` (design 34 § 6.2 row 5).
    pub egress_allowing: Vec<String>,
}

/// Why the set does not hold a default-deny policy named `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotDefaultDeny(pub String);

/// Reads `policies`, every item of the namespace's list, for the default-deny policy `name`.
///
/// It must exist and its `spec` must be exactly `podSelector: {}`, `policyTypes` naming `Ingress`
/// and `Egress` once each, and no rule: `ingress` and `egress` absent or empty. Any other member
/// of the `spec` is refused, so a field a later API version adds is not read as denying.
///
/// # Errors
///
/// [`NotDefaultDeny`] naming what is wrong; a policy that cannot be hashed canonically (a
/// floating-point number in it) is refused the same way.
pub fn read(policies: &[Value], name: &str) -> Result<PolicySet, NotDefaultDeny> {
    let policy = policies
        .iter()
        .find(|policy| policy["metadata"]["name"] == name)
        .ok_or_else(|| NotDefaultDeny(format!("the NetworkPolicy {name:?} is absent")))?;
    check_default_deny(name, &policy["spec"])?;
    let mut entries: Vec<(&str, Value)> = policies
        .iter()
        .map(|policy| {
            let metadata = &policy["metadata"];
            (
                metadata["name"].as_str().unwrap_or_default(),
                json!({
                    "name": metadata["name"],
                    "uid": metadata["uid"],
                    "resourceVersion": metadata["resourceVersion"],
                    "spec": policy["spec"],
                }),
            )
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let set: Vec<Value> = entries.into_iter().map(|(_, entry)| entry).collect();
    let mut egress_allowing: Vec<String> = policies
        .iter()
        .filter(|other| other["metadata"]["name"] != name && has_rules(&other["spec"]["egress"]))
        .map(|other| text(&other["metadata"]["name"]))
        .collect();
    egress_allowing.sort();
    Ok(PolicySet {
        default_deny_uid: text(&policy["metadata"]["uid"]),
        default_deny_resource_version: text(&policy["metadata"]["resourceVersion"]),
        default_deny_spec: digest(&policy["spec"], name)?,
        set_digest: digest(&Value::Array(set), "the namespace's policy set")?,
        egress_allowing,
    })
}

/// Whether a policy's `ingress` or `egress` member states any rule: present and not an empty
/// list. A member that is no list is read as allowing, never as denying.
fn has_rules(rules: &Value) -> bool {
    !rules.is_null() && rules.as_array().is_none_or(|rules| !rules.is_empty())
}

fn check_default_deny(name: &str, spec: &Value) -> Result<(), NotDefaultDeny> {
    let refuse = |why: &str| Err(NotDefaultDeny(format!("the NetworkPolicy {name:?} {why}")));
    let Some(members) = spec.as_object() else {
        return refuse("has no spec");
    };
    if let Some(other) = members.keys().find(|member| {
        !["podSelector", "policyTypes", "ingress", "egress"].contains(&member.as_str())
    }) {
        return refuse(&format!("has the spec member {other:?}"));
    }
    if spec["podSelector"]
        .as_object()
        .is_none_or(|selector| !selector.is_empty())
    {
        return refuse("does not select every Pod: podSelector must be {}");
    }
    let mut types: Vec<&str> = spec["policyTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    types.sort_unstable();
    if types != ["Egress", "Ingress"] || spec["policyTypes"].as_array().map(Vec::len) != Some(2) {
        return refuse("does not name Ingress and Egress once each in policyTypes");
    }
    for direction in ["ingress", "egress"] {
        if has_rules(&spec[direction]) {
            return refuse(&format!("allows {direction} traffic"));
        }
    }
    Ok(())
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn digest(value: &Value, what: &str) -> Result<String, NotDefaultDeny> {
    let canonical = substrate_wire::canonical_json(value)
        .map_err(|_| NotDefaultDeny(format!("{what} cannot be hashed canonically")))?;
    Ok(hex(&Sha256::digest(canonical.as_bytes())))
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}
