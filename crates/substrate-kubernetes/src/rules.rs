//! What the driver's credential may hold (ADR 0047 decision 3, amended by design 34 § 14 Q1) and
//! the startup check that refuses more (design 34 § 6.1 step 2).
//!
//! The review is a `SelfSubjectRulesReview`, so it shows only what the API server can enumerate:
//! decision 3 refuses what is visible, and an `incomplete` review is logged and counted, never
//! read as proof of less.

use std::collections::BTreeSet;

use serde_json::Value;

/// ADR 0047 decision 3's namespaced Role, as `(apiGroup, resource, verbs)`.
pub const ROLE_RULES: [(&str, &str, &[&str]); 6] = [
    ("", "pods", &["create", "get", "list", "watch", "delete"]),
    ("", "pods/log", &["get"]),
    ("", "pods/exec", &["create"]),
    (
        "",
        "persistentvolumeclaims",
        &["create", "get", "list", "delete"],
    ),
    ("networking.k8s.io", "networkpolicies", &["get", "list"]),
    (
        "coordination.k8s.io",
        "leases",
        &["create", "get", "update"],
    ),
];

/// The resource rules every authenticated subject holds in a conformant cluster, from
/// `system:basic-user`. Pinned here and checked against a real cluster by the lane (O8).
pub const DEFAULT_RESOURCE_RULES: [(&str, &str, &[&str]); 3] = [
    (
        "authorization.k8s.io",
        "selfsubjectaccessreviews",
        &["create"],
    ),
    (
        "authorization.k8s.io",
        "selfsubjectrulesreviews",
        &["create"],
    ),
    ("authentication.k8s.io", "selfsubjectreviews", &["create"]),
];

/// The non-resource rules every authenticated subject holds, from `system:discovery` and
/// `system:public-info-viewer`, as `(verb, URLs)`. Also checked by O8.
pub const DEFAULT_NON_RESOURCE_RULES: [(&str, &[&str]); 1] = [(
    "get",
    &[
        "/api",
        "/api/*",
        "/apis",
        "/apis/*",
        "/healthz",
        "/livez",
        "/openapi",
        "/openapi/*",
        "/readyz",
        "/version",
        "/version/",
    ],
)];

/// What a review shows beyond the allowed set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// Every grant beyond [`ROLE_RULES`] and the defaults, as `verb group/resource` or
    /// `verb url`, or a description of a rule that cannot be read. Empty means none is visible.
    pub excess: Vec<String>,
    /// The review's own `incomplete`: the API server could not enumerate every rule.
    pub incomplete: bool,
}

/// Reads a review's `status` against the allowed set. A wildcard in any position, any verb on
/// `secrets`, any grant outside the table, and any rule whose members cannot be read are excess:
/// a rule this check cannot read is refused, never skipped.
#[must_use]
pub fn review(status: &Value) -> Review {
    let mut allowed: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
    for (group, resource, verbs) in ROLE_RULES.iter().chain(DEFAULT_RESOURCE_RULES.iter()) {
        for verb in *verbs {
            allowed.insert((group, resource, verb));
        }
    }
    let mut excess = Vec::new();
    match status["resourceRules"].as_array() {
        Some(rules) => {
            for rule in rules {
                resource_rule(rule, &allowed, &mut excess);
            }
        }
        None if status["resourceRules"].is_null() => {}
        None => excess.push("resourceRules is not a list".to_owned()),
    }
    match status["nonResourceRules"].as_array() {
        Some(rules) => {
            for rule in rules {
                non_resource_rule(rule, &mut excess);
            }
        }
        None if status["nonResourceRules"].is_null() => {}
        None => excess.push("nonResourceRules is not a list".to_owned()),
    }
    Review {
        excess,
        incomplete: status["incomplete"] == Value::Bool(true),
    }
}

/// The strings of `rule[member]`, or `None` when it is not a non-empty list of strings.
fn strings<'a>(rule: &'a Value, member: &str) -> Option<Vec<&'a str>> {
    let list = rule[member].as_array()?;
    let strings: Option<Vec<&str>> = list.iter().map(Value::as_str).collect();
    strings.filter(|strings| !strings.is_empty())
}

fn resource_rule(rule: &Value, allowed: &BTreeSet<(&str, &str, &str)>, excess: &mut Vec<String>) {
    let (Some(verbs), Some(groups), Some(resources)) = (
        strings(rule, "verbs"),
        strings(rule, "apiGroups"),
        strings(rule, "resources"),
    ) else {
        excess.push(format!("an unreadable resource rule {rule}"));
        return;
    };
    for group in &groups {
        for resource in &resources {
            for verb in &verbs {
                let wildcard = [*group, *resource, *verb]
                    .iter()
                    .any(|part| part.contains('*'));
                let secret = *resource == "secrets" || resource.starts_with("secrets/");
                if wildcard || secret || !allowed.contains(&(*group, *resource, *verb)) {
                    excess.push(format!("{verb} {group}/{resource}"));
                }
            }
        }
    }
}

fn non_resource_rule(rule: &Value, excess: &mut Vec<String>) {
    let (Some(verbs), Some(urls)) = (strings(rule, "verbs"), strings(rule, "nonResourceURLs"))
    else {
        excess.push(format!("an unreadable non-resource rule {rule}"));
        return;
    };
    for verb in &verbs {
        for url in &urls {
            let held = DEFAULT_NON_RESOURCE_RULES
                .iter()
                .any(|(allowed, urls)| allowed == verb && urls.contains(url));
            if !held {
                excess.push(format!("{verb} {url}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::review;

    #[test]
    fn the_default_non_resource_rules_pass_and_any_other_is_excess() {
        let status = json!({
            "resourceRules": [],
            "nonResourceRules": [
                {"verbs": ["get"], "nonResourceURLs": ["/api", "/api/*", "/healthz", "/version/"]},
            ],
            "incomplete": true,
        });
        let read = review(&status);
        assert!(read.excess.is_empty(), "{:?}", read.excess);
        assert!(read.incomplete);
        for rule in [
            json!({"verbs": ["get"], "nonResourceURLs": ["/metrics"]}),
            json!({"verbs": ["*"], "nonResourceURLs": ["/api"]}),
            json!({"verbs": ["get"], "nonResourceURLs": ["*"]}),
            json!({"verbs": ["get"]}),
        ] {
            let read = review(&json!({"nonResourceRules": [rule]}));
            assert!(!read.excess.is_empty(), "{rule} is excess");
        }
        assert!(!review(&json!({"resourceRules": {}})).excess.is_empty());
    }
}
