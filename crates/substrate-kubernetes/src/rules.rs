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
    ("", "pods/exec", &["get", "create"]),
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

/// The most excess grants a review names; the rest are counted, never rendered.
pub const NAMED_EXCESS: usize = 8;
/// The longest a named grant is, in characters: a member of a rule is as long as the API server
/// let it be.
const NAMED_EXCESS_CHARS: usize = 160;

/// What a review shows beyond the allowed set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// The first [`NAMED_EXCESS`] grants beyond [`ROLE_RULES`] and the defaults, as
    /// `verb group/resource` or `verb url`, or a description of a rule that cannot be read, each
    /// at most `NAMED_EXCESS_CHARS` characters.
    pub excess: Vec<String>,
    /// Every grant beyond the allowed set, counted over the distinct members of each rule, and
    /// one for each rule that cannot be read. Zero means none is visible.
    pub excess_total: u64,
    /// The review's own `incomplete`: the API server could not enumerate every rule.
    pub incomplete: bool,
}

/// The grants a review counts and the few it names. A rule's grants are the product of its
/// lists, so a rule of a few thousand short strings states billions: they are counted by
/// arithmetic, and only the named few are ever built.
#[derive(Default)]
struct Excess {
    named: Vec<String>,
    total: u64,
}

impl Excess {
    fn full(&self) -> bool {
        self.named.len() >= NAMED_EXCESS
    }

    fn name(&mut self, render: impl FnOnce() -> String) {
        if !self.full() {
            let rendered = render();
            let named = match rendered.char_indices().nth(NAMED_EXCESS_CHARS - 1) {
                Some((cut, _)) => format!("{}…", &rendered[..cut]),
                None => rendered,
            };
            self.named.push(named);
        }
    }

    fn unreadable(&mut self, render: impl FnOnce() -> String) {
        self.total = self.total.saturating_add(1);
        self.name(render);
    }
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
    let mut excess = Excess::default();
    match status["resourceRules"].as_array() {
        Some(rules) => {
            for rule in rules {
                resource_rule(rule, &allowed, &mut excess);
            }
        }
        None if status["resourceRules"].is_null() => {}
        None => excess.unreadable(|| "resourceRules is not a list".to_owned()),
    }
    match status["nonResourceRules"].as_array() {
        Some(rules) => {
            for rule in rules {
                non_resource_rule(rule, &mut excess);
            }
        }
        None if status["nonResourceRules"].is_null() => {}
        None => excess.unreadable(|| "nonResourceRules is not a list".to_owned()),
    }
    Review {
        excess: excess.named,
        excess_total: excess.total,
        incomplete: status["incomplete"] == Value::Bool(true),
    }
}

/// The distinct strings of `rule[member]`, or `None` when it is not a non-empty list of strings.
fn strings<'a>(rule: &'a Value, member: &str) -> Option<BTreeSet<&'a str>> {
    let list = rule[member].as_array()?;
    let strings: Option<BTreeSet<&str>> = list.iter().map(Value::as_str).collect();
    strings.filter(|strings| !strings.is_empty())
}

fn size(set: &BTreeSet<&str>) -> u64 {
    u64::try_from(set.len()).unwrap_or(u64::MAX)
}

fn resource_rule(rule: &Value, allowed: &BTreeSet<(&str, &str, &str)>, excess: &mut Excess) {
    let (Some(verbs), Some(groups), Some(resources)) = (
        strings(rule, "verbs"),
        strings(rule, "apiGroups"),
        strings(rule, "resources"),
    ) else {
        excess.unreadable(|| format!("an unreadable resource rule {rule}"));
        return;
    };
    // An allowed triple names no wildcard and no secret, so the rule's held grants are exactly
    // the allowed triples it covers, and everything else in its product is excess.
    let held = allowed
        .iter()
        .filter(|(group, resource, verb)| {
            groups.contains(group) && resources.contains(resource) && verbs.contains(verb)
        })
        .count();
    let product = size(&groups)
        .saturating_mul(size(&resources))
        .saturating_mul(size(&verbs));
    excess.total = excess
        .total
        .saturating_add(product.saturating_sub(u64::try_from(held).unwrap_or(u64::MAX)));
    // Each step names a grant or passes a distinct allowed triple, so this walk ends after at
    // most NAMED_EXCESS plus the allowed set's size steps, whatever the product.
    'walk: for group in &groups {
        for resource in &resources {
            for verb in &verbs {
                if excess.full() {
                    break 'walk;
                }
                let wildcard = [*group, *resource, *verb]
                    .iter()
                    .any(|part| part.contains('*'));
                let secret = *resource == "secrets" || resource.starts_with("secrets/");
                if wildcard || secret || !allowed.contains(&(*group, *resource, *verb)) {
                    excess.name(|| format!("{verb} {group}/{resource}"));
                }
            }
        }
    }
}

fn non_resource_rule(rule: &Value, excess: &mut Excess) {
    let (Some(verbs), Some(urls)) = (strings(rule, "verbs"), strings(rule, "nonResourceURLs"))
    else {
        excess.unreadable(|| format!("an unreadable non-resource rule {rule}"));
        return;
    };
    let held_pair = |verb: &str, url: &str| {
        DEFAULT_NON_RESOURCE_RULES
            .iter()
            .any(|(allowed, urls)| *allowed == verb && urls.contains(&url))
    };
    let held = DEFAULT_NON_RESOURCE_RULES
        .iter()
        .flat_map(|(verb, urls)| urls.iter().map(move |url| (*verb, *url)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|(verb, url)| verbs.contains(verb) && urls.contains(url))
        .count();
    excess.total = excess.total.saturating_add(
        size(&verbs)
            .saturating_mul(size(&urls))
            .saturating_sub(u64::try_from(held).unwrap_or(u64::MAX)),
    );
    'walk: for verb in &verbs {
        for url in &urls {
            if excess.full() {
                break 'walk;
            }
            if !held_pair(verb, url) {
                excess.name(|| format!("{verb} {url}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{NAMED_EXCESS, review};

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

    /// The API server authorizes the driver's `pods/exec` WebSocket as `get` (O10, ADR 0047's
    /// addendum of 2026-10-29), so decision 3 holds `get` beside `create` there: a review that
    /// states both on `pods/exec` shows no excess.
    #[test]
    fn get_and_create_on_pods_exec_are_both_held() {
        let status = json!({
            "resourceRules": [
                {"verbs": ["get", "create"], "apiGroups": [""], "resources": ["pods/exec"]},
            ],
        });
        let read = review(&status);
        assert!(read.excess.is_empty(), "{:?}", read.excess);
        assert_eq!(read.excess_total, 0);
        let widened = review(&json!({
            "resourceRules": [
                {"verbs": ["get", "create", "list"], "apiGroups": [""], "resources": ["pods/exec"]},
            ],
        }));
        assert_eq!(widened.excess, ["list /pods/exec"]);
        assert_eq!(widened.excess_total, 1);
    }

    /// A rule of a few thousand short strings states a product of billions of grants. The review
    /// counts all of them, names at most [`NAMED_EXCESS`] and still refuses, without building the
    /// product: one string per grant would be tens of gigabytes.
    #[test]
    fn a_rule_whose_product_is_huge_is_counted_named_within_the_bound_and_refused() {
        let numbered = |prefix: &str, count: usize| -> Vec<String> {
            (0..count).map(|index| format!("{prefix}{index}")).collect()
        };
        let mut groups = numbered("g", 2_048);
        groups.push(String::new());
        groups.push(String::new());
        let mut resources = numbered("r", 2_048);
        resources.push("pods".to_owned());
        let mut verbs = numbered("v", 254);
        verbs.push("get".to_owned());
        verbs.push("delete".to_owned());
        let status = json!({
            "resourceRules": [{"verbs": verbs, "apiGroups": groups, "resources": resources}],
            "nonResourceRules": [{"verbs": ["get", "get"], "nonResourceURLs": numbered("/u", 4_096)}],
        });

        let read = review(&status);
        let resource_product = 2_049_u64 * 2_049 * 256;
        let held = 2;
        assert_eq!(read.excess_total, resource_product - held + 4_096);
        assert_eq!(read.excess.len(), NAMED_EXCESS);
        assert!(
            read.excess
                .iter()
                .all(|named| named.chars().count() <= super::NAMED_EXCESS_CHARS),
            "{:?}",
            read.excess
        );

        let long = "x".repeat(100_000);
        let read = review(
            &json!({"resourceRules": [{"verbs": ["get"], "apiGroups": [long], "resources": ["pods"]}]}),
        );
        assert_eq!(read.excess_total, 1);
        assert!(read.excess[0].chars().count() <= super::NAMED_EXCESS_CHARS);
    }
}
