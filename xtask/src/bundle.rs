//! `cargo xtask check-bundle` — verify a released contract bundle directory.
//!
//! Every released bundle needs a checker in the gate: "a bundle whose checker is not in the gate is
//! unverified from the next commit onward" (`AGENTS.md` § *The gate*). `0.1.0`–`0.4.0` are checked
//! by the four frozen `check-contract-bundle*.py` pairs, which stay Python because they are those
//! bundles' reproducibility proof. Everything cut from here on is checked by this verb, because
//! anything that *runs* in a b10x foundation repository is Rust (`atlas/AGENTS.md` § *Language*).
//!
//! Five claims, in the order a reader should trust them:
//!
//! 1. **Fixed point.** The directory is exactly what [`crate::render`] produces from
//!    `xtask/bundle-source/<version>`. This is the load-bearing one: it makes every byte in
//!    `contracts/` the output of a reviewable program rather than something somebody typed, so a
//!    hand-edit anywhere in the tree fails here and nowhere else has to look for it.
//! 2. **Manifest integrity.** `bundle.json` lists every other file once, with its exact length,
//!    digest and media type — the same self-description a consumer verifies after unpacking.
//! 3. **Compatibility.** The declared predecessor exists, the declared `preserves_routes` and
//!    `adds_routes` are the counts the two route inventories actually produce, and no route the
//!    predecessor served has been dropped. An additive successor that quietly removed a route would
//!    otherwise still pass its own schema.
//! 4. **Classification** (invariant 7). Every JSON under `schemas/` declares the pinned Draft
//!    2020-12 meta-schema and validates against it; every other JSON declares exactly one `$schema`
//!    pointing under `schemas/`, and validates against it. Unclassified JSON fails closed.
//! 5. **Its own additions.** The named contract change this version exists for is present. Without
//!    this, a successor could render, verify and preserve everything — and add nothing.
//!
//! `$ref` resolution is worth one note. Each schema is registered under a synthetic
//! `https://b10x.invalid/` URI that mirrors its path in the bundle, with its `$id` removed first, so
//! a relative reference like `../common.json#/$defs/workspace-id` resolves by path exactly as a
//! reader following the tree would resolve it. Leaving the `urn:b10x:…` `$id` in place would make
//! that same reference resolve against the URN and fail.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::render::{self, Inputs, Rendered};
use crate::repo;
use crate::report::Report;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
/// The URI namespace the schema registry is keyed under. A wire-visible b10x identifier, reserved
/// and unroutable by RFC 6761 `.invalid`; it names nothing on any network.
const RESOURCE_ROOT: &str = "https://b10x.invalid/substrate-wire";

/// `cargo xtask check-bundle <version>`.
#[derive(Debug, Parser)]
pub struct Args {
    /// Released bundle version to verify, for example `0.5.0`.
    pub version: String,
    /// Released bundle root (default `contracts/substrate-wire`).
    #[arg(long, value_name = "DIR")]
    pub contracts_root: Option<PathBuf>,
    /// Authored source root (default `xtask/bundle-source`).
    #[arg(long, value_name = "DIR")]
    pub source: Option<PathBuf>,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let root = repo::root()?;
    let contracts = args
        .contracts_root
        .clone()
        .unwrap_or_else(|| root.join("contracts/substrate-wire"));
    let source = args
        .source
        .clone()
        .unwrap_or_else(|| root.join("xtask/bundle-source"));
    Ok(check(&Inputs {
        version: args.version.clone(),
        source_root: source,
        contracts_root: contracts,
        repository_root: root,
        wire: render::wire_constants(),
    })?
    .emit())
}

/// Every claim above, against one bundle directory.
///
/// # Errors
///
/// Returns an error only when the bundle cannot be read at all. A bundle that reads but does not
/// hold produces a [`Report`] of failures, so the gate prints all of them rather than the first.
pub fn check(inputs: &Inputs) -> Result<Report> {
    let bundle = inputs.contracts_root.join(&inputs.version);
    if !bundle.is_dir() {
        return Ok(Report::failed(vec![format!(
            "{} is not a released bundle directory",
            bundle.display()
        )]));
    }
    let released = tree_of(&bundle)?;
    let mut failures = Vec::new();

    let rendered = render::render(inputs).with_context(|| {
        format!(
            "re-rendering {} from {}",
            inputs.version,
            inputs.source_root.display()
        )
    })?;
    check_fixed_point(&released, &rendered, &mut failures);
    check_manifest(&released, &mut failures);
    check_compatibility(inputs, &released, &mut failures);
    check_classification(&inputs.version, &released, &mut failures);
    check_additions(&inputs.version, &released, &mut failures);

    if failures.is_empty() {
        Ok(Report::passed(format!(
            "contract bundle {} verified: {} files, fixed point of xtask/bundle-source/{}",
            inputs.version,
            released.len(),
            inputs.version
        )))
    } else {
        Ok(Report::failed(failures))
    }
}

/// The released tree is byte-for-byte what the renderer produces.
fn check_fixed_point(released: &Tree, rendered: &Rendered, failures: &mut Vec<String>) {
    let released_paths: BTreeSet<&String> = released.keys().collect();
    let rendered_paths: BTreeSet<&String> = rendered.keys().collect();
    for extra in released_paths.difference(&rendered_paths) {
        failures.push(format!("{extra}: present in the bundle, not rendered"));
    }
    for missing in rendered_paths.difference(&released_paths) {
        failures.push(format!("{missing}: rendered, absent from the bundle"));
    }
    for (path, bytes) in released {
        if let Some(expected) = rendered.get(path)
            && expected != bytes
        {
            failures.push(format!(
                "{path}: bundle bytes are not the renderer's output; re-render rather than editing"
            ));
        }
    }
}

/// `bundle.json` describes every other file exactly.
fn check_manifest(released: &Tree, failures: &mut Vec<String>) {
    let Some(bundle) = json_at(released, "bundle.json", failures) else {
        return;
    };
    let Some(files) = bundle.get("files").and_then(Value::as_array) else {
        failures.push("bundle.json: no files array".to_owned());
        return;
    };
    let mut listed = BTreeSet::new();
    for entry in files {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            failures.push("bundle.json: a manifest entry has no path".to_owned());
            continue;
        };
        if !listed.insert(path.to_owned()) {
            failures.push(format!("bundle.json: {path} is listed twice"));
        }
        let Some(bytes) = released.get(path) else {
            failures.push(format!("bundle.json: {path} is listed but absent"));
            continue;
        };
        if entry.get("byte_length").and_then(Value::as_u64) != Some(bytes.len() as u64) {
            failures.push(format!("bundle.json: {path} byte_length disagrees"));
        }
        if entry.get("sha256").and_then(Value::as_str)
            != Some(hex::encode(Sha256::digest(bytes))).as_deref()
        {
            failures.push(format!("bundle.json: {path} sha256 disagrees"));
        }
        let expected_media = if Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            "application/json"
        } else {
            "text/markdown"
        };
        if entry.get("media_type").and_then(Value::as_str) != Some(expected_media) {
            failures.push(format!("bundle.json: {path} media_type disagrees"));
        }
    }
    // The manifest describes the bundle *around* `bundle.json`; a file it does not name is a file
    // no consumer verifies.
    for path in released.keys() {
        if path != "bundle.json" && !listed.contains(path) {
            failures.push(format!("bundle.json: {path} is present but unlisted"));
        }
    }
}

/// The declared successor relationship is the one the two route inventories actually describe.
fn check_compatibility(inputs: &Inputs, released: &Tree, failures: &mut Vec<String>) {
    let Some(bundle) = json_at(released, "bundle.json", failures) else {
        return;
    };
    let Some(predecessor) = bundle
        .pointer("/compatibility/predecessor")
        .and_then(Value::as_str)
    else {
        failures.push("bundle.json: no compatibility.predecessor".to_owned());
        return;
    };
    let current = match route_ids(released) {
        Ok(ids) => ids,
        Err(error) => {
            failures.push(format!("operations.json: {error}"));
            return;
        }
    };
    let predecessor_path = inputs
        .contracts_root
        .join(predecessor)
        .join("operations.json");
    let Ok(text) = std::fs::read_to_string(&predecessor_path) else {
        failures.push(format!(
            "bundle.json: predecessor {predecessor} has no operations.json at {}",
            predecessor_path.display()
        ));
        return;
    };
    let Ok(previous_registry) = serde_json::from_str::<Value>(&text) else {
        failures.push(format!("{predecessor}/operations.json does not parse"));
        return;
    };
    let previous: BTreeSet<String> = previous_registry
        .get("operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();

    let preserves = previous.intersection(&current).count() as u64;
    let adds = current.difference(&previous).count() as u64;
    if bundle
        .pointer("/compatibility/preserves_routes")
        .and_then(Value::as_u64)
        != Some(preserves)
    {
        failures.push(format!(
            "bundle.json: preserves_routes disagrees with the inventories ({preserves})"
        ));
    }
    if bundle
        .pointer("/compatibility/adds_routes")
        .and_then(Value::as_u64)
        != Some(adds)
    {
        failures.push(format!(
            "bundle.json: adds_routes disagrees with the inventories ({adds})"
        ));
    }
    // Additive means additive. A dropped route would still satisfy the counts above if a new one
    // were added in the same commit.
    for dropped in previous.difference(&current) {
        failures.push(format!(
            "operations.json: route {dropped} served by {predecessor} is absent; an additive \
             successor never drops one"
        ));
    }
}

/// Invariant 7: exactly one schema classification per JSON authority, and it validates.
fn check_classification(version: &str, released: &Tree, failures: &mut Vec<String>) {
    let mut documents: BTreeMap<&String, Value> = BTreeMap::new();
    for (path, bytes) in released {
        if Path::new(path)
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => {
                documents.insert(path, value);
            }
            Err(error) => failures.push(format!("{path}: does not parse: {error}")),
        }
    }

    let mut registry = jsonschema::Registry::new();
    let mut registered = BTreeSet::new();
    for (path, document) in &documents {
        if !path.starts_with("schemas/") {
            continue;
        }
        if document.get("$schema").and_then(Value::as_str) != Some(DRAFT_2020_12) {
            failures.push(format!(
                "{path}: schema authority must declare the pinned Draft 2020-12 meta-schema"
            ));
            continue;
        }
        if let Err(error) = jsonschema::draft202012::meta::validate(document) {
            failures.push(format!(
                "{path}: is not a valid Draft 2020-12 schema: {error}"
            ));
            continue;
        }
        // `$id` removed so the registered URI is the base a relative `$ref` resolves against.
        let mut resource = (*document).clone();
        if let Some(object) = resource.as_object_mut() {
            object.remove("$id");
        }
        let uri = resource_uri(version, path);
        match registry.add(uri.clone(), resource) {
            Ok(next) => {
                registry = next;
                registered.insert((*path).clone());
            }
            Err(error) => {
                // Fails closed: without a complete registry nothing below can be validated, and a
                // partial pass would read as a pass.
                failures.push(format!("{path}: cannot register as {uri}: {error}"));
                return;
            }
        }
    }
    let registry = match registry.prepare() {
        Ok(registry) => registry,
        Err(error) => {
            failures.push(format!("bundle schemas do not form a registry: {error}"));
            return;
        }
    };

    for (path, document) in &documents {
        if path.starts_with("schemas/") {
            continue;
        }
        let Some(declaration) = document.get("$schema").and_then(Value::as_str) else {
            failures.push(format!(
                "{path}: unclassified JSON authority (no exact schema mapping)"
            ));
            continue;
        };
        let Some(target) = resolve(path, declaration) else {
            failures.push(format!(
                "{path}: declared schema escapes the bundle: {declaration}"
            ));
            continue;
        };
        if !target.starts_with("schemas/") {
            failures.push(format!(
                "{path}: declared schema is not under schemas/: {declaration}"
            ));
            continue;
        }
        if !registered.contains(&target) {
            failures.push(format!(
                "{path}: declared schema is unavailable: {declaration}"
            ));
            continue;
        }
        let reference = json!({ "$ref": resource_uri(version, &target) });
        match jsonschema::draft202012::options()
            .with_registry(&registry)
            .build(&reference)
        {
            Ok(validator) => {
                if let Err(error) = validator.validate(document) {
                    failures.push(format!("{path}: classified schema validation: {error}"));
                }
            }
            Err(error) => {
                failures.push(format!("{path}: schema {target} does not compile: {error}"));
            }
        }
    }
}

/// The contract change this version exists for.
///
/// A successor that rendered, verified and preserved everything while adding nothing is the failure
/// this catches; the entries are the acceptance list of the story that cut the bundle.
fn check_additions(version: &str, released: &Tree, failures: &mut Vec<String>) {
    if version == "0.7.0" {
        check_delegation_additions(released, failures);
        return;
    }
    if version == "0.6.0" {
        check_aperture_additions(released, failures);
        return;
    }
    if version != "0.5.0" {
        return;
    }
    let require = |path: &str, pointer: &str, what: &str, failures: &mut Vec<String>| {
        let Some(document) = json_at(released, path, failures) else {
            return;
        };
        if document.pointer(pointer).is_none() {
            failures.push(format!("{path}: {what} is absent at {pointer}"));
        }
    };
    require(
        "schemas/inputs/exec-start.json",
        "/properties/secret_slots",
        "the secret_slots start field (ADR 0012)",
        failures,
    );
    require(
        "schemas/inputs/pipe-session-start.json",
        "/properties/exec/properties/secret_slots",
        "the secret_slots session-start field (ADR 0012)",
        failures,
    );
    require(
        "schemas/capability.json",
        "/properties/facts/properties/secrets.slots",
        "the secrets.slots capability fact (ADR 0012)",
        failures,
    );

    let required_requirements = [
        "secrets.slot-cleanup",
        "secrets.slot-delivery",
        "secrets.slot-non-leakage",
        "secrets.slot-sealed",
        "secrets.slot-unknown",
        "secrets.slot-unserved",
    ];
    let Some(coverage) = json_at(released, "coverage.json", failures) else {
        return;
    };
    let rows = coverage
        .get("requirements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for requirement in required_requirements {
        let covered = rows.iter().any(|row| {
            row.get("id").and_then(Value::as_str) == Some(requirement)
                && row
                    .get("evidence")
                    .and_then(Value::as_array)
                    .is_some_and(|evidence| !evidence.is_empty())
        });
        if !covered {
            failures.push(format!(
                "coverage.json: requirement {requirement} is absent or carries no evidence"
            ));
        }
    }

    // The refusal classes, read out of the vectors that assert them rather than out of prose.
    for (path, code) in [
        (
            "vectors/driver/secret-slot-unknown-name-refused.json",
            "exec.secret-slot-unknown",
        ),
        (
            "vectors/driver/secret-slot-unserved-without-capability.json",
            "exec.secret-slots-unserved",
        ),
    ] {
        let Some(vector) = json_at(released, path, failures) else {
            continue;
        };
        if vector
            .pointer("/expected/outcome/code")
            .and_then(Value::as_str)
            != Some(code)
        {
            failures.push(format!("{path}: does not assert the refusal class {code}"));
        }
    }
}

/// One bundle file's path relative to the bundle root, to its exact bytes.
type Tree = BTreeMap<String, Vec<u8>>;

fn tree_of(root: &Path) -> Result<Tree> {
    let mut tree = Tree::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .with_context(|| format!("cannot read {}", directory.display()))?;
        for entry in entries {
            let path = entry
                .with_context(|| format!("cannot read {}", directory.display()))?
                .path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| anyhow!("{} escapes the bundle root", path.display()))?
                .to_string_lossy()
                .into_owned();
            let bytes =
                std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
            tree.insert(relative, bytes);
        }
    }
    Ok(tree)
}

fn json_at(released: &Tree, path: &str, failures: &mut Vec<String>) -> Option<Value> {
    let Some(bytes) = released.get(path) else {
        failures.push(format!("{path}: absent from the bundle"));
        return None;
    };
    match serde_json::from_slice(bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            failures.push(format!("{path}: does not parse: {error}"));
            None
        }
    }
}

fn route_ids(released: &Tree) -> Result<BTreeSet<String>> {
    let bytes = released
        .get("operations.json")
        .ok_or_else(|| anyhow!("absent from the bundle"))?;
    let registry: Value = serde_json::from_slice(bytes)?;
    Ok(registry
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("no operations array"))?
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect())
}

fn resource_uri(version: &str, path: &str) -> String {
    format!("{RESOURCE_ROOT}/{version}/{path}")
}

/// Resolves a bundle-relative `$schema` declaration against the declaring document's directory.
///
/// Returns `None` when the reference climbs out of the bundle, which is the escape invariant 7
/// fails closed on.
fn resolve(from: &str, declaration: &str) -> Option<String> {
    let mut resolved: Vec<&str> = Vec::new();
    let parent: Vec<&str> = from.split('/').collect();
    for segment in &parent[..parent.len().saturating_sub(1)] {
        resolved.push(segment);
    }
    for segment in declaration.split('/') {
        match segment {
            "." | "" => {}
            ".." => {
                resolved.pop()?;
            }
            other => resolved.push(other),
        }
    }
    Some(resolved.join("/"))
}

/// What `0.6.0` exists for: destination-bound egress apertures (ADR 0013).
///
/// The request field, the capability fact, the applied observation and the refusals — each read out
/// of the bundle rather than out of prose, so a successor that renders and preserves everything but
/// adds none of this fails here.
fn check_aperture_additions(released: &Tree, failures: &mut Vec<String>) {
    let require = |path: &str, pointer: &str, what: &str, failures: &mut Vec<String>| {
        let Some(document) = json_at(released, path, failures) else {
            return;
        };
        if document.pointer(pointer).is_none() {
            failures.push(format!("{path}: {what} is absent at {pointer}"));
        }
    };
    require(
        "schemas/inputs/exec-start.json",
        "/properties/sandbox/properties/aperture",
        "the aperture start field (ADR 0013)",
        failures,
    );
    require(
        "schemas/inputs/pipe-session-start.json",
        "/properties/exec/properties/sandbox/properties/aperture",
        "the aperture session-start field (ADR 0013)",
        failures,
    );
    require(
        "schemas/capability.json",
        "/properties/facts/properties/exec.egress-apertures",
        "the exec.egress-apertures capability fact (ADR 0013)",
        failures,
    );
    require(
        "schemas/resource.json",
        "/$defs/applied-network",
        "the applied-aperture observation (ADR 0013)",
        failures,
    );

    // Bound to the wire constant here rather than through a `{"$wire": …}` marker in the authored
    // source, because binding it there would mean editing `xtask/src/render.rs` — and every
    // released bundle records the digest of the renderer that produced it, so one edit to that file
    // stops `0.5.0` being a fixed point of its own source. This check makes the same claim from a
    // file no bundle hashes: change the constant and the bundle fails, exactly as intended.
    if let Some(document) = json_at(released, "schemas/capability.json", failures) {
        let declared = document
            .pointer("/properties/facts/properties/exec.egress-apertures/maxItems")
            .and_then(Value::as_u64);
        if declared != Some(u64::from(substrate_wire::MAX_EGRESS_APERTURES)) {
            failures.push(format!(
                "schemas/capability.json: exec.egress-apertures maxItems is {declared:?}, \
                 and substrate_wire::MAX_EGRESS_APERTURES is {}",
                substrate_wire::MAX_EGRESS_APERTURES
            ));
        }
    }

    // A request may never carry a destination. This is the schema saying so, not a comment.
    if let Some(document) = json_at(released, "schemas/inputs/exec-start.json", failures) {
        let rendered = document.to_string();
        for forbidden in ["\"host\"", "\"destination\"", "\"port\""] {
            if rendered.contains(forbidden) {
                failures.push(format!(
                    "schemas/inputs/exec-start.json: a start input names {forbidden}; \
                     configuration owns reach and a request selects a declared name"
                ));
            }
        }
    }

    check_aperture_coverage(released, failures);
}

/// The coverage half of `0.6.0`: every aperture requirement carries evidence, and every refusal is
/// read out of the vector that asserts it rather than out of prose.
fn check_aperture_coverage(released: &Tree, failures: &mut Vec<String>) {
    let required_requirements = [
        "security.egress-aperture-declared",
        "security.egress-aperture-default",
        "security.egress-aperture-exclusive",
        "security.egress-aperture-named",
        "security.egress-aperture-observed",
        "security.egress-aperture-probed",
        "security.egress-aperture-reach",
    ];
    let Some(coverage) = json_at(released, "coverage.json", failures) else {
        return;
    };
    let rows = coverage
        .get("requirements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for requirement in required_requirements {
        let covered = rows.iter().any(|row| {
            row.get("id").and_then(Value::as_str) == Some(requirement)
                && row
                    .get("evidence")
                    .and_then(Value::as_array)
                    .is_some_and(|evidence| !evidence.is_empty())
        });
        if !covered {
            failures.push(format!(
                "coverage.json: requirement {requirement} is absent or carries no evidence"
            ));
        }
    }

    // The refusal classes, read out of the vectors that assert them rather than out of prose.
    for (path, code) in [
        (
            "vectors/http/aperture-undeclared-is-unserved.json",
            "exec.aperture-undeclared",
        ),
        (
            "vectors/http/aperture-fact-absent-refuses.json",
            "exec.egress-apertures-unserved",
        ),
        (
            "vectors/http/aperture-destination-in-request-refused.json",
            "exec.aperture-destination-in-request",
        ),
    ] {
        let Some(vector) = json_at(released, path, failures) else {
            continue;
        };
        if vector.pointer("/expected/response/body/error/code") != Some(&json!(code)) {
            failures.push(format!("{path}: does not assert the refusal {code}"));
        }
    }
    // The floor did not move: the predecessor's own egress vector is still here, still answering
    // an aperture request from a driver that serves none exactly as it did.
    if let Some(vector) = json_at(released, "vectors/http/egress-unserved.json", failures)
        && vector.pointer("/expected/response/body/error/code")
            != Some(&json!("exec.network-unserved"))
    {
        failures.push(
            "vectors/http/egress-unserved.json: the pre-aperture refusal changed shape".to_owned(),
        );
    }
}

/// What `0.7.0` exists for: delegated context and grant attribution (ADR 0011).
///
/// The request member, the two ledger members, the refusal classes and the conformance vector pair
/// — each read out of the bundle rather than out of prose, so a successor that renders and
/// preserves everything but adds none of this fails here.
fn check_delegation_additions(released: &Tree, failures: &mut Vec<String>) {
    check_delegated_request_member(released, failures);
    check_delegated_ledger_members(released, failures);
    check_delegated_vector_key_shape(released, failures);
    check_delegation_coverage(released, failures);
}

/// The request member is on *every* keyed arm, and the schema never requires it: whether a
/// deployment requires one is configuration, not contract (ADR 0011, "the field is optional
/// everywhere and the hosted requirement cannot be turned on").
fn check_delegated_request_member(released: &Tree, failures: &mut Vec<String>) {
    if let Some(request) = json_at(released, "schemas/request.json", failures) {
        if request.pointer("/$defs/delegated-context").is_none() {
            failures.push(
                "schemas/request.json: the delegated-context definition is absent at \
                 /$defs/delegated-context (ADR 0011)"
                    .to_owned(),
            );
        }
        let branches = request
            .get("anyOf")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if branches.is_empty() {
            failures.push("schemas/request.json: the keyed request union is empty".to_owned());
        }
        for (index, branch) in branches.iter().enumerate() {
            if branch.pointer("/properties/delegated_context").is_none() {
                failures.push(format!(
                    "schemas/request.json: branch {index} does not admit delegated_context"
                ));
            }
            if branch
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.iter().any(|name| name == "delegated_context"))
            {
                failures.push(format!(
                    "schemas/request.json: branch {index} makes delegated_context required; \
                     the member is optional everywhere in the contract"
                ));
            }
            if branch.get("additionalProperties") != Some(&json!(false)) {
                failures.push(format!(
                    "schemas/request.json: branch {index} is no longer a closed object"
                ));
            }
        }
        // Bound to the wire constant from a file no bundle hashes, for the reason
        // `check_aperture_additions` gives: a `{"$wire": …}` marker would mean editing
        // `xtask/src/render.rs`, whose sha256 is every released bundle's `generator.digest`.
        let declared = request
            .pointer("/$defs/delegated-context/maxLength")
            .and_then(Value::as_u64);
        if declared != Some(substrate_wire::MAX_DELEGATED_CONTEXT_BYTES as u64) {
            failures.push(format!(
                "schemas/request.json: delegated-context maxLength is {declared:?}, \
                 and substrate_wire::MAX_DELEGATED_CONTEXT_BYTES is {}",
                substrate_wire::MAX_DELEGATED_CONTEXT_BYTES
            ));
        }
    }
}

/// The ledger row. `principal` keeps its process-id meaning and is not reused: collapsing the two is
/// the confusion ADR 0011 exists to prevent, so both members must be present *beside* it.
fn check_delegated_ledger_members(released: &Tree, failures: &mut Vec<String>) {
    if let Some(operation) = json_at(released, "schemas/operation.json", failures) {
        let branches = operation
            .get("oneOf")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, branch) in branches.iter().enumerate() {
            for member in ["grant_ref", "platform_principal"] {
                if branch.pointer(&format!("/properties/{member}")).is_none() {
                    failures.push(format!(
                        "schemas/operation.json: state branch {index} carries no {member} \
                         (ADR 0011)"
                    ));
                }
                if branch
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| required.iter().any(|name| name == member))
                {
                    failures.push(format!(
                        "schemas/operation.json: state branch {index} requires {member}; \
                         both members are nullable"
                    ));
                }
            }
            if branch.pointer("/properties/principal").is_none() {
                failures.push(format!(
                    "schemas/operation.json: state branch {index} lost principal; the process id \
                     keeps its own column"
                ));
            }
        }
    }
}

/// The conformance vector pair carries public key material and nothing else.
///
/// The setup shape is closed here rather than trusted: a private member added to it later would
/// otherwise validate, and the pair is the artifact connectors holds byte-identically.
fn check_delegated_vector_key_shape(released: &Tree, failures: &mut Vec<String>) {
    if let Some(vector) = json_at(released, "schemas/vector.json", failures) {
        let branch = vector
            .pointer("/properties/setup/items/oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|branch| {
                branch.pointer("/properties/kind/const") == Some(&json!("delegated-context-key"))
            })
            .cloned();
        match branch {
            None => failures.push(
                "schemas/vector.json: no delegated-context-key setup shape (ADR 0011)".to_owned(),
            ),
            Some(branch) => {
                if branch.pointer("/properties/state/additionalProperties") != Some(&json!(false)) {
                    failures.push(
                        "schemas/vector.json: the delegated-context-key state is open; \
                         a vector could carry material this bundle never reviews"
                            .to_owned(),
                    );
                }
                let members: BTreeSet<String> = branch
                    .pointer("/properties/state/properties")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(name, _)| name.clone())
                    .collect();
                for forbidden in ["private_key", "secret_key", "seed", "d", "signing_key"] {
                    if members.contains(forbidden) {
                        failures.push(format!(
                            "schemas/vector.json: the delegated-context-key state names \
                             {forbidden}; the pair carries verifying material only"
                        ));
                    }
                }
                if !members.contains("public_key") {
                    failures.push(
                        "schemas/vector.json: the delegated-context-key state names no public_key"
                            .to_owned(),
                    );
                }
            }
        }
    }
}

/// The coverage half of `0.7.0`: every trust requirement carries evidence, and every refusal is read
/// out of the vector that asserts it rather than out of prose.
fn check_delegation_coverage(released: &Tree, failures: &mut Vec<String>) {
    let required_requirements = [
        "trust.caller-written-identity-ignored",
        "trust.delegated-context-absent",
        "trust.delegated-context-audience",
        "trust.delegated-context-expiry",
        "trust.delegated-context-grant-conflict",
        "trust.delegated-context-malformed",
        "trust.delegated-context-optional",
        "trust.delegated-context-recorded",
        "trust.delegated-context-signature",
        "trust.delegated-context-subject-binding",
        "trust.delegated-context-unknown-key",
    ];
    let Some(coverage) = json_at(released, "coverage.json", failures) else {
        return;
    };
    let rows = coverage
        .get("requirements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for requirement in required_requirements {
        let covered = rows.iter().any(|row| {
            row.get("id").and_then(Value::as_str) == Some(requirement)
                && row
                    .get("evidence")
                    .and_then(Value::as_array)
                    .is_some_and(|evidence| !evidence.is_empty())
        });
        if !covered {
            failures.push(format!(
                "coverage.json: requirement {requirement} is absent or carries no evidence"
            ));
        }
    }

    check_delegation_refusal_vectors(released, failures);
}

/// Every refusal class design 09 section 5 names, read out of the vector that asserts it.
fn check_delegation_refusal_vectors(released: &Tree, failures: &mut Vec<String>) {
    for (path, code) in [
        (
            "vectors/http/delegated-context-absent-when-required.json",
            "delegated-context.absent",
        ),
        (
            "vectors/http/delegated-context-malformed.json",
            "delegated-context.malformed",
        ),
        (
            "vectors/http/delegated-context-unknown-key.json",
            "delegated-context.unknown-key",
        ),
        (
            "vectors/http/delegated-context-signature-invalid.json",
            "delegated-context.signature-invalid",
        ),
        (
            "vectors/http/delegated-context-audience-mismatch.json",
            "delegated-context.audience-mismatch",
        ),
        (
            "vectors/http/delegated-context-subject-mismatch.json",
            "delegated-context.subject-mismatch",
        ),
        (
            "vectors/http/delegated-context-expired.json",
            "delegated-context.expired",
        ),
        (
            "vectors/http/delegated-context-grant-conflict.json",
            "delegated-context.grant-conflict",
        ),
    ] {
        let Some(vector) = json_at(released, path, failures) else {
            continue;
        };
        if vector.pointer("/expected/response/body/error/code") != Some(&json!(code)) {
            failures.push(format!("{path}: does not assert the refusal {code}"));
        }
        // None of them degrades to "ran, unattributed" (invariant 3): every refusal vector states
        // that the ledger row it left carries no grant.
        let unattributed = vector
            .get("postconditions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|row| {
                row.get("actual") == Some(&json!("/ledger/grant_ref"))
                    && row.get("operator") == Some(&json!("equals"))
            });
        if !unattributed {
            failures.push(format!(
                "{path}: states nothing about the grant its refusal left on the ledger"
            ));
        }
    }

    // The accepting half of the pair: the row carries the grant, and the platform principal is not
    // the process id.
    let accepting = "vectors/http/delegated-context-records-grant.json";
    if let Some(vector) = json_at(released, accepting, failures) {
        let rows = vector
            .get("postconditions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for member in ["/ledger/grant_ref", "/ledger/platform_principal"] {
            let stated = rows.iter().any(|row| {
                row.get("actual") == Some(&json!(member))
                    && row.get("expected").is_some_and(|value| !value.is_null())
            });
            if !stated {
                failures.push(format!(
                    "{accepting}: does not state the {member} the verified context recorded"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Args, check, resolve, run};
    use crate::render::wire_constants;
    use crate::repo;
    use serde_json::Value;
    use std::path::PathBuf;

    const VERSION: &str = "0.6.0";
    /// The predecessor is checked too: the gate runs both, and so does this module.
    const PREDECESSOR: &str = "0.5.0";

    fn root() -> PathBuf {
        repo::root().expect("workspace root")
    }

    fn inputs() -> crate::render::Inputs {
        let root = root();
        crate::render::Inputs {
            version: VERSION.to_owned(),
            source_root: root.join("xtask/bundle-source"),
            contracts_root: root.join("contracts/substrate-wire"),
            repository_root: root,
            wire: wire_constants(),
        }
    }

    /// The gate's own claim: the released successor holds, whole.
    #[test]
    fn the_released_successor_bundle_holds() {
        let report = check(&inputs()).expect("the bundle reads");
        assert!(report.failures().is_empty(), "{}", report.failure_text());
        assert!(report.summary().contains(VERSION));
    }

    /// And so does the one before it. Every released bundle rendered by this renderer is a fixed
    /// point of its own source, or the whole tree stops being the output of a reviewable program.
    #[test]
    fn the_predecessor_bundle_still_holds() {
        let mut inputs = inputs();
        inputs.version = PREDECESSOR.to_owned();
        let report = check(&inputs).expect("the bundle reads");
        assert!(report.failures().is_empty(), "{}", report.failure_text());
    }

    /// The command surface runs the same check.
    #[test]
    fn the_command_verifies_the_released_successor() {
        let code = run(&Args {
            version: VERSION.to_owned(),
            contracts_root: None,
            source: None,
        })
        .expect("the command runs");
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", std::process::ExitCode::SUCCESS)
        );
    }

    /// A single edited byte in `contracts/` fails, which is what makes the tree trustworthy.
    #[test]
    fn an_edited_bundle_byte_is_refused() {
        let scratch = tempfile::Builder::new()
            .prefix("check-bundle")
            .tempdir()
            .expect("scratch");
        let contracts = scratch.path().join("substrate-wire");
        let bundle = contracts.join(VERSION);
        copy_tree(&root().join("contracts/substrate-wire"), &contracts);

        let unedited = check(&crate::render::Inputs {
            contracts_root: contracts.clone(),
            ..inputs()
        })
        .expect("the copy reads");
        assert!(
            unedited.failures().is_empty(),
            "{}",
            unedited.failure_text()
        );

        let target = bundle.join("compatibility.json");
        let mut document: Value =
            serde_json::from_slice(&std::fs::read(&target).expect("read")).expect("parse");
        document["status"] = Value::String("stable".to_owned());
        std::fs::write(
            &target,
            serde_json::to_string_pretty(&document).expect("serialize") + "\n",
        )
        .expect("write");

        let report = check(&crate::render::Inputs {
            contracts_root: contracts,
            ..inputs()
        })
        .expect("the edited copy reads");
        let text = report.failure_text();
        assert!(
            text.contains("compatibility.json"),
            "an edited byte must be named, got {text}"
        );
        assert!(text.contains("re-render rather than editing"), "{text}");
    }

    /// A `$schema` that climbs out of the bundle is not a classification.
    #[test]
    fn a_declaration_that_escapes_the_bundle_is_refused() {
        assert_eq!(
            resolve("vectors/driver/x.json", "../../schemas/vector.json").as_deref(),
            Some("schemas/vector.json")
        );
        assert_eq!(
            resolve("bundle.json", "schemas/bundle.json").as_deref(),
            Some("schemas/bundle.json")
        );
        assert_eq!(resolve("bundle.json", "../outside.json"), None);
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).expect("create");
        for entry in std::fs::read_dir(from).expect("read") {
            let entry = entry.expect("entry");
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).expect("copy");
            }
        }
    }
}
