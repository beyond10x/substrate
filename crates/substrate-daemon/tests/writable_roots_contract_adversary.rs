#![forbid(unsafe_code)]
//! Adversarial pass on the writable-roots wave (ADR 0038): the refusals a start can receive for a
//! writable root, read against the bundle that publishes the start's contract.
//!
//! `schemas/common.json` in `substrate-wire/0.22.0` publishes, on `$defs."writable-roots"`, the
//! register `x-b10x-refusals`. Its precedent, `$defs."read-only-files"` (ADR 0033), lists the
//! host-side refusals as well as the lexical ones (`exec.read-only-file-absent`, `-symlink`,
//! `-not-a-file`). The writable-roots register lists only the lexical and unserved codes, yet the
//! host driver and the daemon refuse a start with six more, each a 4xx a client sees on the wire:
//!
//! - `exec.writable-root-absent`, `-not-a-directory`, `-not-owned`
//!   (`crates/substrate-host/src/writable_roots.rs`, kind 1 pin and kind 2 pin),
//! - `exec.writable-root-not-git`, `-own-workspace`, `-workspace-duplicate`
//!   (`crates/substrate-host/src/lib.rs`, kind 2 admission).

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

fn published_writable_root_codes() -> BTreeSet<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/substrate-wire/0.22.0/schemas/common.json");
    let common: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("bundle 0.22.0 common.json"))
            .expect("common.json is JSON");
    let roots = &common["$defs"]["writable-roots"];
    assert!(roots.is_object(), "writable-roots is defined in 0.22.0");
    ["x-b10x-refusals", "x-b10x-failures"]
        .iter()
        .filter_map(|register| roots[*register].as_array())
        .flatten()
        .map(|entry| entry["code"].as_str().expect("code").to_owned())
        .collect()
}

/// Every code the implementation refuses a writable-root start with has a row in the published
/// register, as every read-only-file refusal does.
#[test]
fn every_writable_root_refusal_the_host_and_daemon_raise_is_published_in_0_22_0() {
    let published = published_writable_root_codes();
    let emitted = [
        "exec.writable-root-absent",
        "exec.writable-root-not-a-directory",
        "exec.writable-root-not-owned",
        substrate_host::WRITABLE_ROOT_NOT_GIT,
        substrate_host::WRITABLE_ROOT_OWN_WORKSPACE,
        substrate_host::WRITABLE_ROOT_WORKSPACE_DUPLICATE,
    ];
    let missing: Vec<&str> = emitted
        .iter()
        .copied()
        .filter(|code| !published.contains(*code))
        .collect();
    assert!(
        missing.is_empty(),
        "refusals the implementation raises that bundle 0.22.0 does not publish on \
         writable-roots: {missing:?}; published: {published:?}"
    );
}
