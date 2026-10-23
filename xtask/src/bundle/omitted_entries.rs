//! What `0.23.0` adds for ADR 0043: a listing names the entries it omits in a response header.
//!
//! A v1 directory page and a v2 tree leave out a FIFO, socket, device or non-UTF-8 name, and before
//! ADR 0043 a v1 page said nothing about it while a v2 tree reported it only by setting
//! `truncated`, which also meant "the limit was reached"
//! (`story:directory-listing-names-omitted-entries`). The count now travels in the
//! `x-b10x-omitted-entries` response header, which tells an omission from the limit while
//! `truncated` keeps answering both, and both result schemas publish it as an
//! `x-b10x-response-headers` annotation, because neither closed body can carry a new member.
//!
//! The header name this check expects is `substrate_wire::OMITTED_ENTRIES_HEADER`, the constant the
//! daemon writes. The bundle states it as a literal, because `render_v2.rs` is hashed by every
//! bundle it rendered and cannot learn a new `{"$wire": …}` binding (AGENTS.md § *The gate*); the
//! binding lives here, which no bundle hashes.

use serde_json::{Value, json};
use substrate_wire as wire;

use super::{Tree, json_at};

const FILE_READ: &str = "schemas/results/workspace-file-read.json";
const TREE_READ: &str = "schemas/results/workspace-tree-read-v2.json";

/// The class check, for `0.23.0` and every later bundle: the directory branch of the v1 read and
/// the v2 tree each publish the header as an optional positive count, and the tree's `truncated`
/// names both of its causes and the header that tells them apart.
pub(super) fn check(released: &Tree, failures: &mut Vec<String>) {
    if let Some(file_read) = json_at(released, FILE_READ, failures) {
        let directory = file_read
            .get("oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|branch| branch.pointer("/properties/kind/const") == Some(&json!("directory")));
        match directory {
            Some(branch) => check_header(FILE_READ, "the directory branch", branch, failures),
            None => failures.push(format!("{FILE_READ}: no directory branch")),
        }
    }
    if let Some(tree_read) = json_at(released, TREE_READ, failures) {
        check_header(TREE_READ, "the tree", &tree_read, failures);
        let truncated = tree_read
            .pointer("/properties/truncated/description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !(truncated.contains("limit_items")
            && truncated.contains("omitted")
            && truncated.contains(wire::OMITTED_ENTRIES_HEADER))
        {
            failures.push(format!(
                "{TREE_READ}: truncated must be described by both causes, naming limit_items \
                 and the {} header that tells an omission from the limit",
                wire::OMITTED_ENTRIES_HEADER
            ));
        }
    }
}

/// What only the bundle that introduced the header states: its client-visible change.
pub(super) fn check_introduction(released: &Tree, failures: &mut Vec<String>) {
    let path = "compatibility.json";
    let Some(compatibility) = json_at(released, path, failures) else {
        return;
    };
    let stated = compatibility
        .get("client_visible_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|change| {
            change.contains("ADR 0043")
                && change.contains(wire::OMITTED_ENTRIES_HEADER)
                && change.contains("truncated")
        });
    if !stated {
        failures.push(format!(
            "{path}: client_visible_changes must state, in one entry citing ADR 0043, the {} \
             response header and what truncated answers",
            wire::OMITTED_ENTRIES_HEADER
        ));
    }
}

/// One result: an `x-b10x-response-headers` annotation whose only header is the wire constant, an
/// integer with minimum 1, because the daemon sends it only when something was omitted.
fn check_header(path: &str, what: &str, schema: &Value, failures: &mut Vec<String>) {
    let name = wire::OMITTED_ENTRIES_HEADER;
    let Some(headers) = schema
        .get("x-b10x-response-headers")
        .and_then(Value::as_object)
    else {
        failures.push(format!(
            "{path}: {what} publishes no x-b10x-response-headers annotation"
        ));
        return;
    };
    let Some(header) = headers.get(name) else {
        failures.push(format!("{path}: {what} does not publish the {name} header"));
        return;
    };
    if header.get("type") != Some(&json!("integer")) || header.get("minimum") != Some(&json!(1)) {
        failures.push(format!(
            "{path}: {what}'s {name} must be an integer with minimum 1, because it is sent only \
             when an entry was omitted"
        ));
    }
    if header
        .get("description")
        .and_then(Value::as_str)
        .is_none_or(|text| !text.contains("ADR 0043"))
    {
        failures.push(format!(
            "{path}: {what}'s {name} must be described, citing ADR 0043"
        ));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::repo;

    const VERSION: &str = "0.23.0";

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
        super::check(tree, &mut failures);
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
            "not reported: {named}: {failures:?}"
        );
    }

    /// Acceptance, the bundle half: `0.23.0` publishes the header on both listings.
    #[test]
    fn the_bundle_publishes_the_omitted_entries_header_on_both_listings() {
        let failures = failures_of(&released(VERSION));
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn the_predecessor_publishes_no_header() {
        let failures = failures_of(&released("0.22.0"));
        assert_reported(
            &failures,
            "the directory branch publishes no x-b10x-response-headers annotation",
        );
        assert_reported(
            &failures,
            "the tree publishes no x-b10x-response-headers annotation",
        );
        assert_reported(&failures, "truncated must be described by both causes");
    }

    #[test]
    fn a_header_that_drifts_from_the_wire_crate_is_reported() {
        let tree = edited(super::TREE_READ, |document| {
            let headers = document["x-b10x-response-headers"]
                .as_object_mut()
                .expect("annotation");
            let header = headers.remove("x-b10x-omitted-entries").expect("header");
            headers.insert("x-b10x-omitted".to_owned(), header);
        });
        assert_reported(
            &failures_of(&tree),
            "the tree does not publish the x-b10x-omitted-entries header",
        );
    }

    #[test]
    fn a_header_that_admits_zero_is_reported() {
        let tree = edited(super::FILE_READ, |document| {
            let branch = document["oneOf"]
                .as_array_mut()
                .expect("branches")
                .iter_mut()
                .find(|branch| branch["properties"]["kind"]["const"] == json!("directory"))
                .expect("directory branch");
            branch["x-b10x-response-headers"]["x-b10x-omitted-entries"]["minimum"] = json!(0);
        });
        assert_reported(
            &failures_of(&tree),
            "the directory branch's x-b10x-omitted-entries must be an integer with minimum 1",
        );
    }

    #[test]
    fn a_missing_client_visible_change_is_reported() {
        let tree = edited("compatibility.json", |document| {
            let changes = document["client_visible_changes"]
                .as_array_mut()
                .expect("changes");
            changes.retain(|change| {
                !change
                    .as_str()
                    .is_some_and(|text| text.contains("ADR 0043"))
            });
        });
        assert_reported(
            &failures_of(&tree),
            "client_visible_changes must state, in one entry citing ADR 0043",
        );
    }
}
