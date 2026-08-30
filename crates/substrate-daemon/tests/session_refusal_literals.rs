#![forbid(unsafe_code)]
//! No session refusal code is written as a literal in either crate that raises one.
//!
//! This is the structural half of round 5's R4. Three attach refusals —
//! `session.not-attachable`, `session.already-attached`, `session.attachment-capacity` — were
//! literals in `crates/substrate-daemon/src/app/sessions.rs`, so they were outside
//! `SESSION_PTY_REFUSAL_CODES` and outside `SESSION_PROTOCOL_ERROR_CODES`, and therefore outside
//! the domain `xtask`'s register check ranges over. Neither direction of that check could see them
//! and none had a row in the document whose title is "Every refusal a session can raise".
//!
//! Widening the domain to `substrate_wire::SESSION_REFUSAL_CODES` fixes the three. This is what
//! stops a fourth: a code that is not bound to a constant cannot be in the array, so it cannot be
//! in the domain, so it cannot be in the register — and the only way to notice is to refuse the
//! literal itself.
//!
//! Operation identifiers are a different namespace and are left alone: `session.start`,
//! `session.get`, `session.attach`, `session.signal`, `session.retire`, `session.capabilities` and
//! `session.lease.renew` name routes in the operation registry, not refusals.
//!
//! Portable lane. Reads the two crates' sources and nothing else.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `session.…` words that are operation identifiers rather than refusal codes.
const OPERATION_IDS: [&str; 7] = [
    "session.attach",
    "session.capabilities",
    "session.get",
    "session.lease.renew",
    "session.retire",
    "session.signal",
    "session.start",
];

fn crate_sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|value| value == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Every `"session.…"` string literal in one file, with the line it is on.
fn session_literals(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let mut rest = line;
        while let Some(start) = rest.find("\"session.") {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else {
                break;
            };
            found.push((number + 1, after[..end].to_owned()));
            rest = &after[end..];
        }
    }
    found
}

#[test]
fn no_session_refusal_code_is_written_as_a_literal() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bound: BTreeSet<&str> = substrate_wire::SESSION_REFUSAL_CODES
        .iter()
        .copied()
        .collect();
    let operations: BTreeSet<&str> = OPERATION_IDS.into_iter().collect();
    let mut findings = Vec::new();
    for package in ["crates/substrate-daemon/src", "crates/substrate-host/src"] {
        for file in crate_sources(&workspace.join(package)) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (line, word) in session_literals(&text) {
                if operations.contains(word.as_str()) {
                    continue;
                }
                let relative = file.strip_prefix(&workspace).unwrap_or(&file);
                findings.push(format!(
                    "{}:{line} writes {word:?} as a literal{}",
                    relative.display(),
                    if bound.contains(word.as_str()) {
                        " (it is in SESSION_REFUSAL_CODES; bind the constant)"
                    } else {
                        " (it is in no wire constant at all, so nothing publishes it)"
                    }
                ));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "a session refusal code written out is a code outside the domain the bundle checker \
         ranges over, and so a code the refusal register need not list:\n{}",
        findings.join("\n")
    );
}
