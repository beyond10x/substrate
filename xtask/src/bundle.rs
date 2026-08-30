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
//!    `adds_routes` are the counts the two route inventories actually produce, no route the
//!    predecessor served has been dropped, and no operation both bundles serve answers at a
//!    different path. An additive successor that quietly removed a route would otherwise still pass
//!    its own schema, and one that quietly *moved* a route would pass with `adds_routes: 0` and
//!    nothing at all to report — the inventories are compared on id **and** path for that reason. A
//!    deliberate move is declared by keeping the predecessor's path answering, through a **new**
//!    entry whose `alias_of` names the operation that moved and which serves that path the way the
//!    predecessor served it. Nothing weaker is a declaration: an entry the predecessor already had
//!    would let two operations trade paths and each vouch for the other, and an entry under another
//!    method or scope resolves the URL only to refuse what a pinned consumer sends.
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
    // An inventory that cannot be read at all — a repeated id, one method and path reaching two
    // operations — stops the rest of this function, so the counts, the drops and the moves go
    // unreported for that bundle. That is fail-closed rather than a second defect: the verb exits
    // non-zero and names what has to be fixed before anything else about the successor relationship
    // can be believed. It does mean a first run reports one thing and a second run more.
    let current = match routes_of(released) {
        Ok(routes) => routes,
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
    let previous = match Routes::read(&previous_registry) {
        Ok(routes) => routes,
        Err(error) => {
            failures.push(format!("{predecessor}/operations.json: {error}"));
            return;
        }
    };

    let (previous_ids, current_ids) = (previous.ids(), current.ids());
    let preserves = previous_ids.intersection(&current_ids).count() as u64;
    let adds = current_ids.difference(&previous_ids).count() as u64;
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
    for dropped in previous_ids.difference(&current_ids) {
        failures.push(format!(
            "operations.json: route {dropped} served by {predecessor} is absent; an additive \
             successor never drops one"
        ));
    }
    check_route_paths(&previous, &current, released, inputs, predecessor, failures);
}

/// No operation both bundles serve answers at a different path, unless the move is declared.
///
/// This is the half of compatibility the id inventories cannot state. Their difference is
/// empty when a path moves, so the counts are the predecessor's own and nothing is dropped —
/// the gate could not tell a rename from a no-op.
fn check_route_paths(
    previous: &Routes,
    current: &Routes,
    released: &Tree,
    inputs: &Inputs,
    predecessor: &str,
    failures: &mut Vec<String>,
) {
    let previous_ids = previous.ids();
    let predecessor_root = inputs.contracts_root.join(predecessor);
    let published = Side::Released {
        root: &predecessor_root,
        version: predecessor,
    };
    let offered = Side::Rendered {
        tree: released,
        version: &inputs.version,
    };
    // A path is pinned as firmly as an id, and moving one is invisible to everything above: the id
    // difference is empty, so the counts are the predecessor's own and nothing is dropped. A
    // deliberate move stays expressible — `docs/design/16-sessions-are-not-pipe-sessions.md` needs
    // one — by keeping the predecessor's path answering through a new entry whose `alias_of` names
    // the operation that moved, and which answers there as the predecessor did. That declaration is
    // *read*, never a switch.
    for (id, was) in &previous.served {
        let Some(now) = current.served.get(id) else {
            continue;
        };
        // The shape, not the string: a parameter rename moves no concrete URL, so it is not a
        // move and there is nothing for a declaration to declare.
        if now.shape == was.shape {
            continue;
        }
        let shims = current.shims_for(was, &previous_ids);
        let offered: Vec<(&str, Vec<String>)> = shims
            .iter()
            .map(|shim| {
                (
                    shim.id.as_str(),
                    declaration_differences(was, shim, &published, &offered),
                )
            })
            .collect();
        if offered.iter().any(|(_, differing)| differing.is_empty()) {
            continue;
        }
        // Say what is missing rather than only that something is: with no shim the move was never
        // declared, and with one that differs the old URL resolves but does not answer what it did.
        let unmet = match offered.first() {
            None => format!("no new entry whose alias_of is {id} serves {}", was.path),
            Some((shim, differing)) => format!(
                "the entry {shim} standing at {} differs from what {predecessor} served there in {}",
                was.path,
                differing.join(", ")
            ),
        };
        failures.push(format!(
            "operations.json: route {id} served by {predecessor} at {} is served at {}; a path a \
         consumer pinned moves only while the old one keeps answering as before, and {unmet}",
            was.path, now.path
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
    if version == "0.8.0" {
        check_ceiling_additions(released, failures);
        return;
    }
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

/// The three members a declaration exists to change. **Everything else is compared.**
///
/// There is no whitelist of members that matter, because every attempt to write one was wrong. The
/// first named `method` and `required_scope`, and a shim gated on a capability predicate the moved
/// operation never carried slipped past it — `docs/design/13-pty-sessions.md:140-145` says what
/// that costs in as many words, that hanging a fact on a route "would take the whole route away
/// from a daemon that serves pipes perfectly well", and `docs/design/07-specification-and-\
/// conformance.md:126-128` makes an unknown required fact an unserved request. The second added
/// `address_schema` on the theory that it follows from the path; it does not, and five paths in
/// `0.8.0` carry two or three different values of it.
///
/// So the rule is the one `docs/design/16-sessions-are-not-pipe-sessions.md` states for an alias,
/// applied across the version boundary instead of within one bundle: a shim is the predecessor's
/// own entry, with a new `id`, the `path` it stands on, and the `alias_of` that says what it stands
/// for. A member on one side and not the other is a difference like any other, and there is no
/// mechanism here to excuse one. All eight released bundles carry the same fourteen members, the
/// registry item is `additionalProperties: false`, and no design proposes a fifteenth, so nothing
/// exercises that today. If a successor ever does add a required member, every declared move becomes
/// inexpressible at that boundary, because the predecessor's entry cannot carry it — and this rule
/// is what would have to change. It is not waived here.
const DECLARED_MEMBERS: [&str; 3] = ["id", "path", "alias_of"];

/// Registry members naming a schema document inside the bundle.
///
/// These are compared as **everything the named document means** — itself and the transitive
/// closure of the siblings it reaches by `$ref` — and never as the strings. Two reasons, both
/// measured rather than assumed.
///
/// **The name is not the document.** The same relative name holds a different document in every
/// version, so string equality compares two file names across two bundles and never what they say:
/// a shim could keep the name while the successor narrowed the document under it, and every request
/// a pinned consumer already sends would stop being valid at the path the declaration promised
/// would keep answering.
///
/// **The document is not self-contained.** 45 of the 74 schema documents `0.8.0`'s operations name
/// carry an external `$ref`, into `common.json` (56 references), `resource.json` (13),
/// `event.json` (2), `capability.json` (1) and `operation.json` (1). Every generated address schema
/// is *nothing but* `$ref`s into `common.json`, so comparing a document's own bytes can never see a
/// narrowing of the definition its parameters actually resolve through. The closure can.
///
/// The cost of comparing the closure is real and is the point: a version that changes a shared
/// schema cannot also move a path that reaches it. Measured against the released bundles, a
/// `0.7.0`→`0.8.0`-shaped cut leaves 11 of 26 routes movable and a `0.6.0`→`0.7.0`-shaped one 23 of
/// 26 — because `resource.json` and `capability.json` changed in the first and `operation.json` in
/// the second, and a route whose response schema changed does not answer at the old path the way it
/// did. `docs/design/16-sessions-are-not-pipe-sessions.md` changes no schema document, so all seven
/// of the routes it moves stay declarable.
const SCHEMA_MEMBERS: [&str; 3] = ["address_schema", "input_schema", "result_schema"];

/// A path template with its parameter *names* removed: `/v1/x/{a}` and `/v1/x/{b}` are one shape.
///
/// The router the daemon builds is matchit 0.8.4 under axum 0.8.9 (`Cargo.lock`), and it refuses
/// the second of two registrations whose templates differ only in a parameter's name —
/// "Insertion failed due to conflict with previously registered route". A parameter's name is local
/// to the registry: it names a property of the address schema the renderer generates
/// (`xtask/src/render.rs:355-366`) and never travels, because `hashing.json` substitutes the
/// parameter rather than transmitting its name.
///
/// So the shape, not the string, is what a request can tell apart — and it is what **both** readers
/// here use, from the one place, because they are two halves of one question. Two routes collide
/// when their shapes collide, so parking a route on `/v1/pipe-sessions/{workspace_id}` is not a way
/// to reach a path `/v1/pipe-sessions/{session_id}` already occupies. And an operation has moved
/// only when its shape has, so renaming `{session_id}` to `{id}` is not a move at all: no concrete
/// URL changed, and there is nothing for a consumer to have pinned differently.
fn path_shape(path: &str) -> String {
    let mut shape = String::with_capacity(path.len());
    let mut parameter: Option<String> = None;
    for character in path.chars() {
        match (character, &mut parameter) {
            ('{', None) => parameter = Some(String::new()),
            ('}', Some(name)) => {
                // A wildcard spans segments where a parameter spans one, so the two are not the
                // same shape. No released bundle has one; this keeps the rule right if one arrives.
                shape.push_str(if name.starts_with('*') { "{*}" } else { "{}" });
                parameter = None;
            }
            (_, Some(name)) => name.push(character),
            (_, None) => shape.push(character),
        }
    }
    if parameter.is_some() {
        // An unterminated `{` is not a template; say so rather than guessing at a shape.
        shape.push('{');
    }
    shape
}

/// One entry of a bundle's operation registry, kept whole so a declaration can be compared member
/// by member against the operation it stands in for.
#[derive(Debug, Clone)]
struct Route {
    id: String,
    path: String,
    /// [`path_shape`] of `path`, computed once here so every reader asks the same question.
    shape: String,
    entry: Value,
}

impl Route {
    /// The member as it stands, whatever its type.
    ///
    /// Deliberately **not** `and_then(Value::as_str)`: `capability_predicates` and `effects` are
    /// arrays, and a string-only reader would hand back `None` for both sides of a comparison and
    /// call two different predicate lists equal.
    fn member(&self, name: &str) -> Option<&Value> {
        self.entry.get(name)
    }

    fn members(&self) -> impl Iterator<Item = &str> {
        self.entry
            .as_object()
            .into_iter()
            .flat_map(|entry| entry.keys().map(String::as_str))
    }
}

/// One bundle's route inventory: which operation id is served, at which path, and which entries
/// stand in for another operation.
///
/// The id alone was all this used to read, so the only property the compatibility check could state
/// was *no id disappeared*. A path is pinned by a consumer exactly as an id is, and a successor that
/// keeps every id while moving a path is a rename an id-only inventory cannot see.
#[derive(Debug, Default)]
struct Routes {
    /// Operation id to the entry serving it. One entry per id: a registry that names an id twice is
    /// refused by [`Routes::read`], because keying by id would otherwise keep whichever the array
    /// happened to hold last and never look at the other path at all.
    served: BTreeMap<String, Route>,
    /// The entries declaring `alias_of: <id>`, by the id they stand in for.
    ///
    /// Consulted on the **successor** only. A predecessor's own aliases need no separate treatment:
    /// each is an operation id in its own right, so the successor has to keep serving it, at its
    /// path, exactly like every other id — which is what `served` above already states.
    aliased: BTreeMap<String, Vec<Route>>,
}

impl Routes {
    fn read(registry: &Value) -> Result<Self> {
        let entries = registry
            .get("operations")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("no operations array"))?;
        let mut routes = Self::default();
        let mut dispatch: BTreeMap<(&str, String), (&str, &str)> = BTreeMap::new();
        for entry in entries {
            let (Some(id), Some(path)) = (
                entry.get("id").and_then(Value::as_str),
                entry.get("path").and_then(Value::as_str),
            ) else {
                continue;
            };
            let route = Route {
                id: id.to_owned(),
                path: path.to_owned(),
                shape: path_shape(path),
                entry: entry.clone(),
            };
            if let Some(target) = entry.get("alias_of").and_then(Value::as_str) {
                routes
                    .aliased
                    .entry(target.to_owned())
                    .or_default()
                    .push(route.clone());
            }
            // A router dispatches one request to one operation. Two routes may share a path —
            // `0.8.0` has five such paths, `GET` and `DELETE` on one resource — but never a path
            // *and* a method: which operation the request reaches would be a property of the
            // router rather than of the registry. It is also how a declared move launders itself
            // into a collision, satisfying every condition on the path the operation left while
            // the operation lands on a path somebody else still serves.
            let method = entry.get("method").and_then(Value::as_str).unwrap_or("");
            if let Some(other) = dispatch.insert((method, route.shape.clone()), (id, path)) {
                let mut both = [other, (id, path)];
                both.sort_unstable();
                return Err(anyhow!(
                    "{method} {} is served by two operations, {} at {} and {} at {}; one method \
                     and path shape reach one operation",
                    route.shape,
                    both[0].0,
                    both[0].1,
                    both[1].0,
                    both[1].1
                ));
            }
            if let Some(first) = routes.served.insert(id.to_owned(), route) {
                // Named in sorted order, not in array order: the verdict on a registry has to be a
                // function of the routes it serves, and a message that echoed the order would make
                // the same two entries produce two different reports.
                let mut both = [first.path.as_str(), path];
                both.sort_unstable();
                return Err(anyhow!(
                    "operation {id} is served twice, at {} and at {}; an inventory keyed by id \
                     would keep one of them and never look at the other",
                    both[0],
                    both[1]
                ));
            }
        }
        Ok(routes)
    }

    fn ids(&self) -> BTreeSet<&str> {
        self.served.keys().map(String::as_str).collect()
    }

    /// The entries offered as the declaration that `was` moved: new ids, standing on the path the
    /// operation left, naming it in `alias_of`. Ordered by id, so the report does not depend on
    /// where in the registry they were authored.
    ///
    /// "New" is load-bearing. An id the predecessor already served may not stand in for somebody
    /// else's move: two existing operations naming each other as `alias_of` would trade paths and
    /// each "declare" the other, with every id preserved, nothing added and nothing to report.
    fn shims_for<'a>(&'a self, was: &Route, previous_ids: &BTreeSet<&str>) -> Vec<&'a Route> {
        let mut shims: Vec<&Route> = self
            .aliased
            .get(&was.id)
            .into_iter()
            .flatten()
            .filter(|alias| !previous_ids.contains(alias.id.as_str()) && alias.shape == was.shape)
            .collect();
        shims.sort_by(|left, right| left.id.cmp(&right.id));
        shims
    }
}

/// One side of a document comparison: where a bundle's schema documents are, and which version it
/// states as its own.
enum Side<'a> {
    /// The successor under check, held in memory as the tree that was rendered and read back.
    Rendered { tree: &'a Tree, version: &'a str },
    /// A released bundle directory on disk.
    Released { root: &'a Path, version: &'a str },
}

impl Side<'_> {
    fn version(&self) -> &str {
        match self {
            Self::Rendered { version, .. } | Self::Released { version, .. } => version,
        }
    }

    fn read(&self, path: &str) -> Option<Value> {
        match self {
            Self::Rendered { tree, .. } => serde_json::from_slice(tree.get(path)?).ok(),
            Self::Released { root, .. } => {
                serde_json::from_slice(&std::fs::read(root.join(path)).ok()?).ok()
            }
        }
    }
}

/// A schema document together with every sibling it reaches by `$ref`.
///
/// Comparing two of these compares what a document *means*, which is not what its own bytes say:
/// a generated address schema is nothing but references into `common.json`, so its own bytes cannot
/// change when the definition its parameter resolves through is narrowed.
struct Closure {
    /// Bundle-relative path to the normalised document, for the named document and every sibling
    /// reachable from it.
    documents: BTreeMap<String, Value>,
    /// References that name nothing in this bundle, or that climb out of it.
    ///
    /// Carried rather than dropped, and never silently equal: a dangling reference is not a
    /// document a consumer can be shown to still get, so either side having one is a difference.
    unresolved: BTreeSet<String>,
}

/// The `$ref` closure of one document, normalised for comparison against the other bundle's.
///
/// The walk is bounded by what it has already read rather than by depth, so a reference cycle —
/// legal in a schema tree, and `common.json` is reached from nearly everything — terminates instead
/// of recursing.
fn closure_of(side: &Side, start: &str) -> Closure {
    let mut documents: BTreeMap<String, Value> = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    let mut pending = vec![start.to_owned()];
    while let Some(path) = pending.pop() {
        if documents.contains_key(&path) || unresolved.contains(&path) {
            continue;
        }
        let Some(document) = side.read(&path) else {
            unresolved.insert(path);
            continue;
        };
        for reference in references(&document) {
            // Everything before the fragment: `../common.json#/$defs/session-id` names a sibling,
            // `#/$defs/session-id` names this document and is already in hand.
            let target = reference.split('#').next().unwrap_or_default();
            if target.is_empty() {
                continue;
            }
            match resolve(&path, target) {
                Some(next) => pending.push(next),
                None => {
                    unresolved.insert(reference.clone());
                }
            }
        }
        documents.insert(path, normalised(&document, side.version()));
    }
    Closure {
        documents,
        unresolved,
    }
}

/// Every `$ref` value anywhere in a document.
fn references(document: &Value) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![document];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(members) => {
                for (name, member) in members {
                    match (name.as_str(), member.as_str()) {
                        ("$ref", Some(reference)) => found.push(reference.to_owned()),
                        _ => pending.push(member),
                    }
                }
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    found
}

/// A document with everything that names the bundle it lives in taken out.
///
/// Two versions of one document differ wherever the document states its own version, and they state
/// it in more than one shape. Measured across all eight released bundles, every string carrying a
/// bundle's own version is one of five: the bare version, `substrate-wire/<version>`,
/// `substrate-wire@<version>`, `urn:b10x:substrate-wire:<version>:…` — which is what `$id` is, so it
/// needs no special case of its own — and a renderer file name. The version is therefore replaced
/// wherever it occurs as a whole number, which covers all five and any sixth a successor invents.
///
/// Without this, one of the seven routes `docs/design/16-sessions-are-not-pipe-sessions.md` moves
/// could not be declared at all: `schemas/results/pipe-session-capabilities.json` states
/// `substrate-wire/<version>` at `/properties/contract/const`, so a shim that is a byte-copy of the
/// operation it stands in for still "differed" from it. The escape — giving the shim a private
/// frozen copy of the document — is the shape that design rejects in as many words, because
/// splitting the schema reintroduces the drift the alias exists to prevent, and it would make one
/// daemon answer two contract versions at two paths.
fn normalised(document: &Value, version: &str) -> Value {
    fn walk(value: &Value, version: &str) -> Value {
        match value {
            Value::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(name, member)| (name.clone(), walk(member, version)))
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| walk(item, version)).collect())
            }
            Value::String(text) => Value::String(without_version(text, version)),
            other => other.clone(),
        }
    }
    walk(document, version)
}

/// `text` with every whole occurrence of `version` replaced by a placeholder.
///
/// "Whole" means not bordered by a digit on either side, so `0.8.0` inside `10.8.05` is left alone
/// while `urn:…:0.8.0:result:…`, `substrate-wire@0.8.0` and `render-contract-bundle-0.8.0.py` are
/// all normalised.
fn without_version(text: &str, version: &str) -> String {
    if version.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(version) {
        let (before, tail) = rest.split_at(at);
        let after = &tail[version.len()..];
        let bordered = before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit())
            || after.chars().next().is_some_and(|c| c.is_ascii_digit());
        out.push_str(before);
        if bordered {
            out.push_str(version);
        } else {
            out.push_str("{version}");
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Every member in which a declaration differs from what the predecessor served at that path.
///
/// Empty means the shim answers as the predecessor did, which is the only thing that makes a path
/// move a move rather than a withdrawal. A [`SCHEMA_MEMBERS`] reference is compared as the `$ref`
/// closure of the document it names in each bundle; every other member is compared as it stands.
fn declaration_differences(
    was: &Route,
    shim: &Route,
    published: &Side,
    offered: &Side,
) -> Vec<String> {
    let members: BTreeSet<&str> = was
        .members()
        .chain(shim.members())
        .filter(|member| !DECLARED_MEMBERS.contains(member))
        .collect();
    let mut differing = Vec::new();
    for member in members {
        let same = if SCHEMA_MEMBERS.contains(&member) {
            match (
                was.member(member).and_then(Value::as_str),
                shim.member(member).and_then(Value::as_str),
            ) {
                (Some(was_reference), Some(shim_reference)) => {
                    let published = closure_of(published, was_reference);
                    let offered = closure_of(offered, shim_reference);
                    published.unresolved.is_empty()
                        && offered.unresolved.is_empty()
                        && published.documents == offered.documents
                }
                // A member that is not a reference on both sides names no document at all.
                _ => false,
            }
        } else {
            was.member(member) == shim.member(member)
        };
        if !same {
            differing.push(member.to_owned());
        }
    }
    differing
}

fn routes_of(released: &Tree) -> Result<Routes> {
    let bytes = released
        .get("operations.json")
        .ok_or_else(|| anyhow!("absent from the bundle"))?;
    Routes::read(&serde_json::from_slice(bytes)?)
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

/// What `0.8.0` exists for: a declared aperture byte ceiling (ADR 0014).
///
/// The declaration published, the ceiling observed, the bound named, and the refusal that keeps a
/// ceiling out of a request — each read out of the bundle rather than out of prose, so a successor
/// that renders and preserves everything but adds none of this fails here.
fn check_ceiling_additions(released: &Tree, failures: &mut Vec<String>) {
    check_published_ceiling(released, failures);
    check_observed_ceiling(released, failures);
    check_named_bound(released, failures);
    check_ceiling_coverage(released, failures);
}

/// `/v1/machine` answers how much this daemon could ever pass, and the member stays optional: an
/// aperture declared without a ceiling is unbounded, which is what every aperture was at `0.7.0`.
fn check_published_ceiling(released: &Tree, failures: &mut Vec<String>) {
    let Some(capability) = json_at(released, "schemas/capability.json", failures) else {
        return;
    };
    let items = "/properties/facts/properties/exec.egress-apertures/items";
    if capability
        .pointer(&format!("{items}/properties/max_bytes"))
        .is_none()
    {
        failures.push(
            "schemas/capability.json: the declared aperture ceiling is absent from \
             exec.egress-apertures (ADR 0014)"
                .to_owned(),
        );
    }
    if capability
        .pointer(&format!("{items}/required"))
        .and_then(Value::as_array)
        .is_some_and(|required| required.iter().any(|name| name == "max_bytes"))
    {
        failures.push(
            "schemas/capability.json: exec.egress-apertures requires max_bytes; an aperture \
             declared without a ceiling is unbounded"
                .to_owned(),
        );
    }
}

/// The applied observation states the ceiling the run ran under beside the bytes that crossed.
fn check_observed_ceiling(released: &Tree, failures: &mut Vec<String>) {
    let Some(resource) = json_at(released, "schemas/resource.json", failures) else {
        return;
    };
    let aperture = resource
        .pointer("/$defs/applied-network/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|branch| branch.pointer("/properties/mode/const") == Some(&json!("aperture")))
        .cloned();
    match aperture {
        None => failures.push(
            "schemas/resource.json: no applied-aperture branch to carry a ceiling".to_owned(),
        ),
        Some(branch) => {
            if branch.pointer("/properties/max_bytes").is_none() {
                failures.push(
                    "schemas/resource.json: the applied aperture states no max_bytes (ADR 0014)"
                        .to_owned(),
                );
            }
            if branch
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.iter().any(|name| name == "max_bytes"))
            {
                failures.push(
                    "schemas/resource.json: the applied aperture requires max_bytes; a run with \
                     no declared ceiling reports none"
                        .to_owned(),
                );
            }
            // The bytes did not move to make room for the ceiling: a reader auditing what crossed
            // still reads the same pair it read at `0.6.0`.
            if branch
                .pointer("/properties/bytes/properties/to_destination")
                .is_none()
                || branch
                    .pointer("/properties/bytes/properties/from_destination")
                    .is_none()
            {
                failures.push(
                    "schemas/resource.json: the applied aperture lost a byte counter".to_owned(),
                );
            }
        }
    }
}

/// The refusal has somewhere to live, and it lives only where a run can have hit a bound.
///
/// `accepted` and `running` stay closed against it: a run that has not ended cannot name what
/// ended it, and `additionalProperties: false` is what makes that a contract rather than a habit.
fn check_named_bound(released: &Tree, failures: &mut Vec<String>) {
    let Some(resource) = json_at(released, "schemas/resource.json", failures) else {
        return;
    };
    let refusal = resource.pointer("/$defs/exec-refusal");
    match refusal {
        None => failures.push(
            "schemas/resource.json: no exec-refusal definition at /$defs/exec-refusal (ADR 0014)"
                .to_owned(),
        ),
        Some(refusal) => {
            for (pointer, expected) in [
                ("/properties/class/const", json!("exhausted")),
                ("/properties/code/const", json!("exec.aperture-byte-limit")),
            ] {
                if refusal.pointer(pointer) != Some(&expected) {
                    failures.push(format!(
                        "schemas/resource.json: exec-refusal{pointer} is not {expected} \
                         (design 10 section 5 row 5)"
                    ));
                }
            }
            if refusal.pointer("/properties/message").is_none() {
                failures.push("schemas/resource.json: exec-refusal carries no message".to_owned());
            }
            if refusal.pointer("/properties/address").is_some() {
                failures.push(
                    "schemas/resource.json: exec-refusal carries an address; the byte ceiling \
                     names none, because nothing in the request is at fault"
                        .to_owned(),
                );
            }
        }
    }
    let mut branches_naming_a_bound = 0;
    for branch in resource
        .pointer("/$defs/exec/oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let terminal = matches!(
            branch.pointer("/properties/state"),
            Some(shape)
                if shape.get("const") == Some(&json!("unknown"))
                    || shape
                        .get("enum")
                        .and_then(Value::as_array)
                        .is_some_and(|states| states.iter().any(|name| name == "cancelled"))
        );
        let carries = branch.pointer("/properties/refusal").is_some();
        if carries {
            branches_naming_a_bound += 1;
        }
        if carries != terminal {
            failures.push(format!(
                "schemas/resource.json: an exec branch with state {:?} {} a refusal; only a run \
                 that ended may name the bound that ended it",
                branch.pointer("/properties/state"),
                if carries { "carries" } else { "carries no" }
            ));
        }
    }
    if branches_naming_a_bound == 0 {
        failures
            .push("schemas/resource.json: no exec branch carries a refusal (ADR 0014)".to_owned());
    }
}

/// The coverage half of `0.8.0`: every ceiling requirement carries evidence, and both refusals are
/// read out of the vectors that assert them rather than out of prose.
fn check_ceiling_coverage(released: &Tree, failures: &mut Vec<String>) {
    let required_requirements = [
        "security.egress-aperture-ceiling-absent",
        "security.egress-aperture-ceiling-declared",
        "security.egress-aperture-ceiling-enforced",
        "security.egress-aperture-ceiling-not-requested",
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

    if let Some(vector) = json_at(
        released,
        "vectors/http/aperture-ceiling-in-request-refused.json",
        failures,
    ) && vector.pointer("/expected/response/body/error/code")
        != Some(&json!("exec.aperture-ceiling-in-request"))
    {
        failures.push(
            "vectors/http/aperture-ceiling-in-request-refused.json: does not assert the \
             refusal exec.aperture-ceiling-in-request"
                .to_owned(),
        );
    }
    if let Some(vector) = json_at(
        released,
        "vectors/http/aperture-ceiling-refuses-mid-run.json",
        failures,
    ) {
        if vector.pointer("/expected/response/body/result/refusal/code")
            != Some(&json!("exec.aperture-byte-limit"))
        {
            failures.push(
                "vectors/http/aperture-ceiling-refuses-mid-run.json: the run ends without naming \
                 exec.aperture-byte-limit"
                    .to_owned(),
            );
        }
        // The observation states the ceiling it ran under: a byte count with nothing to compare it
        // against is the report `0.7.0` already had.
        if vector
            .pointer("/expected/response/body/result/applied/network/max_bytes")
            .is_none()
        {
            failures.push(
                "vectors/http/aperture-ceiling-refuses-mid-run.json: the applied aperture states \
                 no ceiling"
                    .to_owned(),
            );
        }
    }
    // The floor did not move: an aperture declared without a ceiling is still the run `0.6.0`
    // proved, and the vector that proves it still asserts a completed one.
    if let Some(vector) = json_at(
        released,
        "vectors/driver/aperture-without-a-ceiling-is-unbounded.json",
        failures,
    ) && vector.pointer("/expected/outcome/state") != Some(&json!("exited"))
    {
        failures.push(
            "vectors/driver/aperture-without-a-ceiling-is-unbounded.json: an aperture with no \
             declared ceiling did not run to completion"
                .to_owned(),
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
    use crate::report::Report;
    use serde_json::{Value, json};
    use std::path::PathBuf;

    const VERSION: &str = "0.8.0";
    /// The predecessor is checked too: the gate runs both, and so does this module.
    const PREDECESSOR: &str = "0.7.0";

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

    /// `0.8.0`'s additions are a claim about `0.8.0` and about no bundle before it.
    ///
    /// Run the same check against the predecessor and every one of them must come back absent. A
    /// check that passes on the tree that predates the change proves nothing about the tree that
    /// carries it.
    #[test]
    fn the_predecessor_carries_none_of_the_successors_additions() {
        let predecessor =
            super::tree_of(&root().join("contracts/substrate-wire").join(PREDECESSOR))
                .expect("the predecessor reads");
        let mut failures = Vec::new();
        super::check_ceiling_additions(&predecessor, &mut failures);
        for absent in [
            "schemas/capability.json",
            "schemas/resource.json",
            "coverage.json",
            "vectors/http/aperture-ceiling-in-request-refused.json",
            "vectors/http/aperture-ceiling-refuses-mid-run.json",
        ] {
            assert!(
                failures.iter().any(|failure| failure.contains(absent)),
                "{PREDECESSOR} was not reported missing {absent}: {failures:?}"
            );
        }
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

    /// The route `docs/design/16-sessions-are-not-pipe-sessions.md` moves: it keeps its operation
    /// id and changes only the path it is served at.
    const MOVED_ID: &str = "session.attach";
    const MOVED_FROM: &str = "/v1/pipe-sessions/{session_id}/attach";
    const MOVED_TO: &str = "/v1/sessions/{session_id}/attach";
    /// The legacy entry a declared move keeps answering at `MOVED_FROM`.
    const ALIAS_ID: &str = "pipe-session.attach";

    /// A successor that keeps every operation id and moves one route's path is a rename no
    /// consumer can see: the id difference is empty, so `preserves_routes` and `adds_routes` are
    /// the predecessor's own numbers and nothing is dropped.
    #[test]
    fn a_successor_that_moves_a_route_path_is_refused() {
        let report = check_with_the_route_moved("moved-route", |_| {});
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "the moved id must be named: {text}"
        );
        assert!(
            text.contains(MOVED_FROM),
            "the path it left must be named: {text}"
        );
        assert!(
            text.contains(MOVED_TO),
            "the path it moved to must be named: {text}"
        );
        assert_eq!(
            report.failures().len(),
            1,
            "the move is the only thing wrong with this successor: {text}"
        );
    }

    /// A move is *declarable*, because design 16 needs one: the predecessor's path keeps answering
    /// through an entry whose `alias_of` names the operation that moved. The check reads that
    /// declaration — it is not switched off for the version.
    #[test]
    fn a_move_declared_by_an_alias_at_the_old_path_is_accepted() {
        let report =
            check_with_the_route_moved("declared-move", |source| declare_alias(source, MOVED_FROM));
        assert!(report.failures().is_empty(), "{}", report.failure_text());
    }

    /// And `alias_of` is not a switch. An alias serving some *other* path leaves the predecessor's
    /// path unanswered, which is the rename the check exists to refuse.
    #[test]
    fn an_alias_that_does_not_serve_the_old_path_declares_nothing() {
        let report = check_with_the_route_moved("stale-alias", |source| {
            declare_alias(source, "/v1/legacy-sessions/{session_id}/attach");
        });
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "the moved id must be named: {text}"
        );
        assert!(
            text.contains(MOVED_FROM),
            "the path it left must be named: {text}"
        );
        assert!(
            text.contains(MOVED_TO),
            "the path it moved to must be named: {text}"
        );
    }

    /// Renders a successor from `0.8.0`'s authored source with `MOVED_ID` moved to `MOVED_TO`,
    /// after `author` has had its say over the copied source tree, and checks what came out.
    ///
    /// Everything happens in a scratch copy: `contracts/` and `xtask/bundle-source/` are read and
    /// never written, because a released bundle directory is immutable (AGENTS.md invariant 6).
    fn check_with_the_route_moved(prefix: &str, author: impl FnOnce(&std::path::Path)) -> Report {
        let scratch = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("scratch");
        let source = scratch.path().join("bundle-source");
        let contracts = scratch.path().join("substrate-wire");
        copy_tree(
            &root().join("xtask/bundle-source").join(VERSION),
            &source.join(VERSION),
        );
        copy_tree(
            &root().join("contracts/substrate-wire").join(PREDECESSOR),
            &contracts.join(PREDECESSOR),
        );

        let authored = source.join(VERSION);
        edit_json(&authored.join("routes.json"), |routes| {
            for route in routes.as_array_mut().expect("routes.json is an array") {
                if route.get("id").and_then(Value::as_str) == Some(MOVED_ID) {
                    assert_eq!(route["path"], json!(MOVED_FROM), "{MOVED_ID} moved already");
                    route["path"] = json!(MOVED_TO);
                }
            }
        });
        author(&authored);

        let inputs = crate::render::Inputs {
            source_root: source,
            contracts_root: contracts.clone(),
            ..inputs()
        };
        let rendered = crate::render::render(&inputs).expect("the authored successor renders");
        for (path, bytes) in &rendered {
            let target = contracts.join(VERSION).join(path);
            std::fs::create_dir_all(target.parent().expect("a parent")).expect("create");
            std::fs::write(&target, bytes).expect("write");
        }
        check(&inputs).expect("the rendered successor reads")
    }

    /// Authors the declared form of the move, the way design 16 does it: a new operation at `at`,
    /// naming the operation it stands in for, and a successor schema that admits the field.
    fn declare_alias(authored: &std::path::Path, at: &str) {
        let at = at.to_owned();
        edit_json(&authored.join("routes.json"), |routes| {
            let routes = routes.as_array_mut().expect("routes.json is an array");
            let mut alias = routes
                .iter()
                .find(|route| route.get("id").and_then(Value::as_str) == Some(MOVED_ID))
                .cloned()
                .expect("the moved route");
            alias["id"] = json!(ALIAS_ID);
            alias["path"] = json!(at);
            alias["alias_of"] = json!(MOVED_ID);
            routes.push(alias);
        });
        // The successor's own registry schema admits the field; every earlier bundle stays closed
        // against it.
        edit_json(
            &authored.join("documents/schemas/operation-registry.json"),
            |registry| {
                registry["properties"]["operations"]["items"]["properties"]["alias_of"] = json!({
                    "pattern": "^[a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)+$",
                    "type": "string",
                });
            },
        );
        // One route added, so the successor's own bundle schema states one.
        edit_json(&authored.join("documents/schemas/bundle.json"), |bundle| {
            bundle["properties"]["compatibility"]["properties"]["adds_routes"] = json!({
                "const": 1,
            });
        });
    }

    fn edit_json(path: &std::path::Path, edit: impl FnOnce(&mut Value)) {
        let mut document: Value =
            serde_json::from_slice(&std::fs::read(path).expect("read")).expect("parse");
        edit(&mut document);
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&document).expect("serialize"),
        )
        .expect("write");
    }

    // ------------------------------------------------------------------------------------------
    // Adversarial cases. Added by the adversarial-verify step against 6702455; they assert the
    // story's own acceptance statement — "a successor bundle that serves an existing operation id
    // at a different path fails `cargo xtask check-bundle`" — against successors the three cases
    // above do not build.
    // ------------------------------------------------------------------------------------------

    /// Two operations that trade paths. Both already exist in the predecessor, both are `GET`,
    /// both take `{session_id}`, so the swap is expressible without touching anything else.
    const TRADE_A: &str = "session.attach";
    const TRADE_A_PATH: &str = "/v1/pipe-sessions/{session_id}/attach";
    const TRADE_B: &str = "session.get";
    const TRADE_B_PATH: &str = "/v1/pipe-sessions/{session_id}";

    /// Renders a successor from `VERSION`'s authored source after `author` has had its say, into a
    /// scratch contracts root holding the predecessor, and hands back the inputs that name it.
    ///
    /// The same shape as `check_with_the_route_moved`, minus its fixed `MOVED_ID` edit, so a case
    /// can author any successor it likes — and returning the inputs rather than a report, so a
    /// case can drive `run` (the verb the acceptance names) and not only `check`.
    fn author_a_successor(
        prefix: &str,
        author: impl FnOnce(&std::path::Path),
    ) -> (tempfile::TempDir, crate::render::Inputs) {
        let scratch = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("scratch");
        let source = scratch.path().join("bundle-source");
        let contracts = scratch.path().join("substrate-wire");
        copy_tree(
            &root().join("xtask/bundle-source").join(VERSION),
            &source.join(VERSION),
        );
        copy_tree(
            &root().join("contracts/substrate-wire").join(PREDECESSOR),
            &contracts.join(PREDECESSOR),
        );
        author(&source.join(VERSION));

        let inputs = crate::render::Inputs {
            source_root: source,
            contracts_root: contracts.clone(),
            ..inputs()
        };
        let rendered = crate::render::render(&inputs).expect("the authored successor renders");
        for (path, bytes) in &rendered {
            let target = contracts.join(VERSION).join(path);
            std::fs::create_dir_all(target.parent().expect("a parent")).expect("create");
            std::fs::write(&target, bytes).expect("write");
        }
        (scratch, inputs)
    }

    /// The successor's own registry schema admits `alias_of`, exactly as `declare_alias` does it.
    /// Without this the entry fails classification and the case would prove nothing about paths.
    fn admit_alias_of(authored: &std::path::Path) {
        edit_json(
            &authored.join("documents/schemas/operation-registry.json"),
            |registry| {
                registry["properties"]["operations"]["items"]["properties"]["alias_of"] = json!({
                    "pattern": "^[a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)+$",
                    "type": "string",
                });
            },
        );
    }

    /// Authors the path trade: each of the two operations moves onto the other's path and names
    /// the other as its `alias_of`.
    fn trade_the_two_paths(authored: &std::path::Path) {
        admit_alias_of(authored);
        edit_json(&authored.join("routes.json"), |routes| {
            for route in routes.as_array_mut().expect("routes.json is an array") {
                match route.get("id").and_then(Value::as_str) {
                    Some(TRADE_A) => {
                        assert_eq!(
                            route["path"],
                            json!(TRADE_A_PATH),
                            "{TRADE_A} moved already"
                        );
                        route["path"] = json!(TRADE_B_PATH);
                        route["alias_of"] = json!(TRADE_B);
                    }
                    Some(TRADE_B) => {
                        assert_eq!(
                            route["path"],
                            json!(TRADE_B_PATH),
                            "{TRADE_B} moved already"
                        );
                        route["path"] = json!(TRADE_A_PATH);
                        route["alias_of"] = json!(TRADE_A);
                    }
                    _ => {}
                }
            }
        });
    }

    /// Two existing operations trade paths. Every id is preserved, `adds_routes` is still 0, no
    /// entry is added — and each move is "declared" by the *other* operation sitting on the path
    /// it vacated, because `Routes::answers` asks only whether some entry naming `alias_of: <id>`
    /// is served at that path and never whether that entry stands in for `<id>` in any way.
    ///
    /// Both old URLs still resolve, and both resolve to the wrong operation:
    /// `GET /v1/pipe-sessions/{session_id}` now reaches `session.attach`, a duplex byte channel,
    /// where it used to reach `session.get`. That is two existing operation ids served at a
    /// different path, which the acceptance statement says must fail.
    #[test]
    fn two_operations_that_trade_paths_are_refused() {
        let (_scratch, inputs) = author_a_successor("traded-paths", trade_the_two_paths);
        let report = check(&inputs).expect("the traded successor reads");
        let text = report.failure_text();
        assert!(
            text.contains(TRADE_A) && text.contains(TRADE_B),
            "both operations moved and both must be named: {text}"
        );
    }

    /// The same successor through the verb the acceptance names, not through `check`.
    #[test]
    fn the_command_refuses_two_operations_that_trade_paths() {
        let (_scratch, inputs) = author_a_successor("traded-paths-cli", trade_the_two_paths);
        let code = run(&Args {
            version: VERSION.to_owned(),
            contracts_root: Some(inputs.contracts_root.clone()),
            source: Some(inputs.source_root.clone()),
        })
        .expect("the command runs");
        assert_ne!(
            format!("{code:?}"),
            format!("{:?}", std::process::ExitCode::SUCCESS),
            "check-bundle accepted a bundle in which two operations traded paths"
        );
    }

    /// `Routes::answers` is documented as "whether a request to `path` still reaches `id`", and it
    /// reads the path string alone. An alias parked on the predecessor's path under a *different*
    /// method leaves every pinned request to that URL answered by a 405 — the old path answers
    /// nothing a consumer sends — and the declaration is accepted anyway.
    #[test]
    fn an_alias_answering_the_old_path_under_another_method_declares_nothing() {
        let report = check_with_the_route_moved("alias-wrong-method", |authored| {
            declare_alias(authored, MOVED_FROM);
            edit_json(&authored.join("routes.json"), |routes| {
                for route in routes.as_array_mut().expect("routes.json is an array") {
                    if route.get("id").and_then(Value::as_str) == Some(ALIAS_ID) {
                        assert_eq!(route["method"], json!("GET"), "{MOVED_ID} was a GET");
                        route["method"] = json!("DELETE");
                    }
                }
            });
        });
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "no GET reaches {MOVED_FROM} any more, so the move is undeclared: {text}"
        );
    }

    /// And the same for the scope. An alias on the old path demanding a scope the moved operation
    /// never demanded refuses every pinned consumer's existing token with a 403; the old URL is as
    /// dead to them as if it had been deleted, and the gate reports the successor verified.
    #[test]
    fn an_alias_answering_the_old_path_under_another_scope_declares_nothing() {
        let report = check_with_the_route_moved("alias-wrong-scope", |authored| {
            declare_alias(authored, MOVED_FROM);
            edit_json(&authored.join("routes.json"), |routes| {
                for route in routes.as_array_mut().expect("routes.json is an array") {
                    if route.get("id").and_then(Value::as_str) == Some(ALIAS_ID) {
                        assert_eq!(route["required_scope"], json!("session"), "scope moved");
                        route["required_scope"] = json!("workspaces");
                    }
                }
            });
        });
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "no session-scoped consumer reaches {MOVED_FROM} any more: {text}"
        );
    }

    /// Boundary: a route that is genuinely dropped is still refused after the inventory stopped
    /// being a bare id set. Differencing `previous_ids`/`current_ids` must keep saying so.
    #[test]
    fn a_dropped_route_is_still_refused() {
        let (_scratch, inputs) = author_a_successor("dropped-route", |authored| {
            edit_json(&authored.join("routes.json"), |routes| {
                let routes = routes.as_array_mut().expect("routes.json is an array");
                routes.retain(|route| {
                    route.get("id").and_then(Value::as_str) != Some("session.lease.renew")
                });
            });
            // The successor's own bundle schema states the count its inventory now produces.
            edit_json(&authored.join("documents/schemas/bundle.json"), |bundle| {
                bundle["properties"]["compatibility"]["properties"]["preserves_routes"] =
                    json!({ "const": 25 });
            });
        });
        let report = check(&inputs).expect("the truncated successor reads");
        let text = report.failure_text();
        assert!(
            text.contains("session.lease.renew") && text.contains("never drops one"),
            "a dropped route must still be named: {text}"
        );
    }

    /// Boundary: a route that is dropped *and* another that moves onto its path must both be
    /// reported. Neither loop may swallow the other's finding.
    #[test]
    fn a_dropped_route_does_not_mask_a_move_onto_its_path() {
        let (_scratch, inputs) = author_a_successor("dropped-and-moved", |authored| {
            edit_json(&authored.join("routes.json"), |routes| {
                let routes = routes.as_array_mut().expect("routes.json is an array");
                routes.retain(|route| {
                    route.get("id").and_then(Value::as_str) != Some("session.lease.renew")
                });
                for route in routes.iter_mut() {
                    if route.get("id").and_then(Value::as_str) == Some(MOVED_ID) {
                        route["path"] = json!("/v1/pipe-sessions/{session_id}/lease/renew");
                    }
                }
            });
            edit_json(&authored.join("documents/schemas/bundle.json"), |bundle| {
                bundle["properties"]["compatibility"]["properties"]["preserves_routes"] =
                    json!({ "const": 25 });
            });
        });
        let report = check(&inputs).expect("the successor reads");
        let text = report.failure_text();
        assert!(
            text.contains("session.lease.renew"),
            "the dropped route must be named: {text}"
        );
        assert!(
            text.contains(MOVED_ID) && text.contains(MOVED_FROM),
            "the moved route must be named too: {text}"
        );
    }

    /// Boundary: a predecessor whose `operations.json` cannot be read must fail closed. An error
    /// path that reports and still lets the verb exit 0 is the defect the story was written
    /// against.
    #[test]
    fn a_predecessor_whose_registry_does_not_parse_is_refused() {
        let (_scratch, inputs) = author_a_successor("broken-predecessor", |_| {});
        std::fs::write(
            inputs
                .contracts_root
                .join(PREDECESSOR)
                .join("operations.json"),
            b"{ not json",
        )
        .expect("write");
        // `render` reads the predecessor's inventory too, so the verb fails there first and
        // never reaches `check_compatibility`'s own arm. Either way it does not exit 0, which is
        // the property that matters; `main` turns the error into `ExitCode::FAILURE`.
        let outcome = run(&Args {
            version: VERSION.to_owned(),
            contracts_root: Some(inputs.contracts_root.clone()),
            source: Some(inputs.source_root.clone()),
        });
        match outcome {
            Err(error) => assert!(
                format!("{error:#}").contains("operations.json"),
                "the unreadable file must be named: {error:#}"
            ),
            Ok(code) => assert_ne!(
                format!("{code:?}"),
                format!("{:?}", std::process::ExitCode::SUCCESS),
                "an unreadable predecessor inventory must not exit 0"
            ),
        }
    }

    /// `Routes::read` keys `served` by id and inserts unconditionally, so two entries sharing one
    /// id collapse to whichever the registry array happens to hold *last*. Nothing in the gate
    /// rejects a duplicate id, and the counts are set-valued, so the verdict on one and the same
    /// pair of served routes turns on array order alone: original-then-twin is refused as a move,
    /// twin-then-original is verified and the second path is never looked at.
    #[test]
    fn one_id_at_two_paths_gets_the_same_verdict_in_either_order() {
        let twin_last = the_verdict_on_a_duplicated_id("dup-twin-last", true);
        let twin_first = the_verdict_on_a_duplicated_id("dup-twin-first", false);
        assert_eq!(
            twin_last, twin_first,
            "the same two served routes, two array orders, two verdicts"
        );
    }

    /// Renders a successor holding `MOVED_ID` twice — once at its released path, once at
    /// `MOVED_TO` — with the twin either after or before the original, and returns what the gate
    /// said about it.
    fn the_verdict_on_a_duplicated_id(prefix: &str, twin_last: bool) -> Vec<String> {
        let (_scratch, inputs) = author_a_successor(prefix, |authored| {
            edit_json(&authored.join("routes.json"), |routes| {
                let routes = routes.as_array_mut().expect("routes.json is an array");
                let at = routes
                    .iter()
                    .position(|route| route.get("id").and_then(Value::as_str) == Some(MOVED_ID))
                    .expect("the route to duplicate");
                let mut twin = routes[at].clone();
                assert_eq!(twin["path"], json!(MOVED_FROM), "{MOVED_ID} moved already");
                twin["path"] = json!(MOVED_TO);
                routes.insert(if twin_last { at + 1 } else { at }, twin);
            });
        });
        check(&inputs)
            .expect("the duplicated successor reads")
            .failures()
            .to_vec()
    }

    // ------------------------------------------------------------------------------------------
    // Adversarial pass 2. Added against 969817f. The three cases above that guard the declaration
    // (`a_move_declared_by_an_alias_at_the_old_path_is_accepted` and the two `an_alias_answering_…`
    // cases) all hold the *vacated* path still. These attack what the declaration says about the
    // path the operation moved *to*, and what the six-field whitelist leaves out of "answers there
    // as the predecessor did".
    // ------------------------------------------------------------------------------------------

    /// The operation `MOVED_ID` is moved onto a path the predecessor already served, and *keeps
    /// serving*, with another operation.
    const OCCUPIED_ID: &str = "session.get";
    const OCCUPIED_PATH: &str = "/v1/pipe-sessions/{session_id}";

    /// Authors a declared move of `MOVED_ID` onto `to`, with the shim `declare_alias` builds
    /// sitting at `MOVED_FROM` and answering there exactly as the predecessor did.
    fn declare_a_move_to(authored: &std::path::Path, to: &str) {
        let to = to.to_owned();
        edit_json(&authored.join("routes.json"), |routes| {
            for route in routes.as_array_mut().expect("routes.json is an array") {
                if route.get("id").and_then(Value::as_str) == Some(MOVED_ID) {
                    assert_eq!(route["path"], json!(MOVED_FROM), "{MOVED_ID} moved already");
                    route["path"] = json!(to);
                }
            }
        });
        declare_alias(authored, MOVED_FROM);
    }

    /// A **declared** move onto a path another preserved operation still serves.
    ///
    /// Every condition the declaration asks for is met: `pipe-session.attach` is new, it sits at
    /// exactly the path `session.attach` left, and it matches on all six of `ANSWERED_BY`. So
    /// `still_answers` says yes and the successor verifies — while `GET
    /// /v1/pipe-sessions/{session_id}` is now claimed by two operations at once, `session.get` and
    /// `session.attach`. Which one a request reaches is not a property of the registry any more,
    /// and `session.attach` is the duplex byte channel whose arrival at that URL is the harm the
    /// path-trade fix was written for. Nothing in the gate compares two entries of the *same*
    /// bundle for a `(method, path)` collision, so the escape hatch launders one move into one.
    ///
    /// By the story's own acceptance statement this successor serves an existing operation id at a
    /// different path and must fail.
    #[test]
    fn a_declared_move_onto_an_occupied_path_is_refused() {
        let (_scratch, inputs) = author_a_successor("declared-move-onto-occupied", |authored| {
            declare_a_move_to(authored, OCCUPIED_PATH);
        });
        let registry: Value = serde_json::from_slice(
            &std::fs::read(inputs.contracts_root.join(VERSION).join("operations.json"))
                .expect("read the rendered registry"),
        )
        .expect("the rendered registry parses");
        let mut claimants: Vec<&str> = registry["operations"]
            .as_array()
            .expect("an operations array")
            .iter()
            .filter(|entry| {
                entry.get("path").and_then(Value::as_str) == Some(OCCUPIED_PATH)
                    && entry.get("method").and_then(Value::as_str) == Some("GET")
            })
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .collect();
        claimants.sort_unstable();
        let mut expected = vec![MOVED_ID, OCCUPIED_ID];
        expected.sort_unstable();
        assert_eq!(
            claimants, expected,
            "the fixture must actually put two operations on one GET path"
        );

        let report = check(&inputs).expect("the successor reads");
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "{MOVED_ID} is served at {OCCUPIED_PATH}, which {OCCUPIED_ID} also serves under the \
             same method; a bundle in which one GET path reaches two operations is not a \
             declaration of anything: {text}"
        );
    }

    /// The same successor through the verb the acceptance statement names.
    #[test]
    fn the_command_refuses_a_declared_move_onto_an_occupied_path() {
        let (_scratch, inputs) =
            author_a_successor("declared-move-onto-occupied-cli", |authored| {
                declare_a_move_to(authored, OCCUPIED_PATH);
            });
        let code = run(&Args {
            version: VERSION.to_owned(),
            contracts_root: Some(inputs.contracts_root.clone()),
            source: Some(inputs.source_root.clone()),
        })
        .expect("the command runs");
        assert_ne!(
            format!("{code:?}"),
            format!("{:?}", std::process::ExitCode::SUCCESS),
            "check-bundle accepted a successor that moved {MOVED_ID} onto {OCCUPIED_PATH}, which \
             {OCCUPIED_ID} still serves under the same method"
        );
    }

    /// Two shims, both new, both parked on the path `MOVED_ID` left, both naming it.
    ///
    /// `still_answers` is an `any` over `aliased[&was.id]`, so one qualifying shim is enough and a
    /// second is never looked at. The result is a registry that answers `GET
    /// /v1/pipe-sessions/{session_id}/attach` with two different operation ids — produced by the
    /// declaration mechanism alone, with no other edit.
    #[test]
    fn two_shims_on_one_vacated_path_are_refused() {
        let (_scratch, inputs) = author_a_successor("two-shims", |authored| {
            declare_a_move_to(authored, MOVED_TO);
            edit_json(&authored.join("routes.json"), |routes| {
                let routes = routes.as_array_mut().expect("routes.json is an array");
                let mut second = routes
                    .iter()
                    .find(|route| route.get("id").and_then(Value::as_str) == Some(ALIAS_ID))
                    .cloned()
                    .expect("the first shim");
                second["id"] = json!("legacy-session.attach");
                routes.push(second);
            });
            edit_json(&authored.join("documents/schemas/bundle.json"), |bundle| {
                bundle["properties"]["compatibility"]["properties"]["adds_routes"] =
                    json!({ "const": 2 });
            });
        });
        let report = check(&inputs).expect("the successor reads");
        let text = report.failure_text();
        assert!(
            !report.failures().is_empty(),
            "two entries answer GET {MOVED_FROM}; a declaration that admits a second claimant on \
             the vacated path declares nothing determinate: {text}"
        );
    }

    /// `ANSWERED_BY` omits `capability_predicates` because they "decide a refusal the driver owns,
    /// not the shape of the exchange" (`bundle.rs:563-565`). That is the same sentence that was
    /// already rejected for `required_scope`, and this repository's own design says so in as many
    /// words: "hanging `sessions.pty` on `POST /v1/pipe-sessions` would take the whole route away
    /// from a daemon that serves pipes perfectly well"
    /// (`docs/design/13-pty-sessions.md:140-145`), and design 07 §4 makes an unknown required fact
    /// an `unserved` request.
    ///
    /// So a shim carrying a predicate the moved operation never carried resolves the vacated URL
    /// and refuses every request a pinned consumer sends to it, on every host that does not
    /// publish that fact — which is exactly the withdrawal-dressed-as-a-declaration the method and
    /// scope cases refuse.
    #[test]
    fn an_alias_gated_on_a_fact_the_moved_operation_never_needed_declares_nothing() {
        let report = check_with_the_route_moved("alias-extra-predicate", |authored| {
            declare_alias(authored, MOVED_FROM);
            edit_json(&authored.join("routes.json"), |routes| {
                for route in routes.as_array_mut().expect("routes.json is an array") {
                    if route.get("id").and_then(Value::as_str) == Some(ALIAS_ID) {
                        route["capability_predicates"] = json!([
                            { "fact": "sessions.pty", "op": "eq", "value": true }
                        ]);
                    }
                }
            });
        });
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "no host without the sessions.pty fact answers {MOVED_FROM} any more, so the move is \
             undeclared: {text}"
        );
    }

    /// `ANSWERED_BY` omits `address_schema` because it "follows from the path, which is compared
    /// exactly" (`bundle.rs:565-566`). It does not follow from the path: five paths in `0.8.0`
    /// carry two or three different `address_schema` values already, so the field is authored, not
    /// derived, and a shim may name one that contradicts the template it sits on.
    ///
    /// Here the shim sits at `/v1/pipe-sessions/{session_id}/attach` and names
    /// `schemas/addresses/workspace-get.json`, which requires `workspace_id` and forbids anything
    /// else. No request to that path can produce an address that validates, so the declaration
    /// hands back a URL nothing can be sent to.
    #[test]
    fn an_alias_naming_an_address_schema_the_path_cannot_fill_declares_nothing() {
        let report = check_with_the_route_moved("alias-wrong-address", |authored| {
            declare_alias(authored, MOVED_FROM);
            edit_json(&authored.join("routes.json"), |routes| {
                for route in routes.as_array_mut().expect("routes.json is an array") {
                    if route.get("id").and_then(Value::as_str) == Some(ALIAS_ID) {
                        assert_eq!(
                            route["address_schema"],
                            json!("schemas/addresses/pipe-session-attach.json"),
                            "the shim no longer copies the moved operation's address schema"
                        );
                        route["address_schema"] = json!("schemas/addresses/workspace-get.json");
                    }
                }
            });
        });
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "the shim's address schema requires workspace_id and the path it sits on offers \
             session_id, so nothing reaches {MOVED_FROM}: {text}"
        );
    }

    /// `ANSWERED_BY` compares `input_schema` and `result_schema` as **strings**, and those strings
    /// are bundle-relative paths — `schemas/inputs/pipe-session-attach.json` names one document
    /// inside `0.7.0` and a different one inside `0.8.0` (their `$id`s alone already differ). So
    /// the equality that is documented as "the request envelope the consumer builds and the
    /// response it parses" (`bundle.rs:552-556`) compares two file *names* across two bundles and
    /// never the documents they name.
    ///
    /// Here the shim sits on the vacated path with a byte-identical `input_schema` string, and the
    /// successor's copy of that schema requires a member `0.7.0`'s copy did not have. Every
    /// attach request a pinned consumer already sends — the empty object `0.7.0` specified — is
    /// now invalid at the path it was promised would keep answering, and the declaration is
    /// accepted.
    #[test]
    fn an_alias_whose_input_schema_names_a_narrowed_document_declares_nothing() {
        let report = check_with_the_route_moved("alias-narrowed-input", |authored| {
            declare_alias(authored, MOVED_FROM);
            edit_json(
                &authored.join("documents/schemas/inputs/pipe-session-attach.json"),
                |schema| {
                    assert_eq!(
                        schema["required"],
                        json!([]),
                        "0.8.0's attach input is no longer the empty object 0.7.0 published"
                    );
                    schema["properties"]["client_key"] = json!({ "type": "string" });
                    schema["required"] = json!(["client_key"]);
                },
            );
        });
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "the shim's input_schema string is unchanged and the document it names is not, so \
             the empty attach input 0.7.0 published no longer reaches {MOVED_FROM}: {text}"
        );
    }

    // ------------------------------------------------------------------------------------------
    // The mutation gap. Every member a shim carries is compared, and every one of them is pinned
    // here: without a case per member, moving a name into `DECLARED_MEMBERS` stops it being
    // compared and no test notices. Verified by doing exactly that, member by member, and watching
    // this case go red for each.
    // ------------------------------------------------------------------------------------------

    /// Each member of a shim, and a value for it the registry schema admits and the moved operation
    /// does not have. `direction` is absent: the renderer writes `outbound` on every entry
    /// (`xtask/src/render.rs:436-441`) and no authored source can vary it.
    const PERTURBATIONS: [(&str, &str); 9] = [
        ("method", r#""DELETE""#),
        ("required_scope", r#""workspaces""#),
        ("idempotency", r#""keyed""#),
        ("input_binding", r#""body.input""#),
        ("risk", r#""read""#),
        ("exposure", r#""callable""#),
        ("effects", r#"["process", "network:egress"]"#),
        (
            "capability_predicates",
            r#"[{ "fact": "sessions.pty", "op": "eq", "value": true }]"#,
        ),
        ("result_schema", r#""schemas/results/machine-get.json""#),
    ];

    /// A shim that differs from the predecessor's entry in **any** member declares nothing, and the
    /// report names the member.
    ///
    /// One case per member rather than one shim carrying all nine: a single perturbed shim would go
    /// on failing with eight of the nine comparisons deleted, which is precisely the hole this
    /// closes. `address_schema` and `input_schema` have their own cases above, written by the
    /// adversarial pass and left alone.
    #[test]
    fn a_shim_differing_in_any_member_declares_nothing() {
        for (member, perturbation) in PERTURBATIONS {
            let value: Value = serde_json::from_str(perturbation).expect("a perturbation parses");
            let report = check_with_the_route_moved(&format!("shim-{member}"), |authored| {
                declare_alias(authored, MOVED_FROM);
                edit_json(&authored.join("routes.json"), |routes| {
                    for route in routes.as_array_mut().expect("routes.json is an array") {
                        if route.get("id").and_then(Value::as_str) == Some(ALIAS_ID) {
                            assert_ne!(
                                route[member], value,
                                "{member} is already the perturbed value; the case proves nothing"
                            );
                            route[member] = value.clone();
                        }
                    }
                });
            });
            let text = report.failure_text();
            assert!(
                text.contains(MOVED_ID),
                "a shim differing in {member} must not declare the move: {text}"
            );
            assert!(
                text.contains(member),
                "the report must name {member} as the difference: {text}"
            );
        }
    }

    /// Two existing operations that swap **everything** but their ids, and declare each other.
    ///
    /// Found by mutating `shims_for` to drop its "a shim is new" filter and watching the suite stay
    /// green: the path-trade case above no longer needs that filter, because the two entries differ
    /// in nine members and the member comparison refuses them on its own. Nothing was left pinning
    /// the filter. This pins it. Here each entry carries the *other* operation's members, so every
    /// member comparison passes and only "a shim is new" stands between the registry and a
    /// successor in which `session.attach` and `session.get` have exchanged identities with every
    /// id preserved, `adds_routes: 0` and no entry added.
    #[test]
    fn two_existing_operations_may_not_declare_each_others_moves() {
        let (_scratch, inputs) = author_a_successor("swapped-identities", |authored| {
            admit_alias_of(authored);
            edit_json(&authored.join("routes.json"), |routes| {
                let routes = routes.as_array_mut().expect("routes.json is an array");
                let find = |routes: &Vec<Value>, id: &str| {
                    routes
                        .iter()
                        .find(|route| route.get("id").and_then(Value::as_str) == Some(id))
                        .cloned()
                        .expect("the route to swap")
                };
                let (a, b) = (find(routes, TRADE_A), find(routes, TRADE_B));
                for route in routes.iter_mut() {
                    match route.get("id").and_then(Value::as_str) {
                        // Everything but the id: the entry keeps its name and takes on the other
                        // operation's whole contract, including the path it is served at.
                        Some(TRADE_A) => {
                            *route = b.clone();
                            route["id"] = json!(TRADE_A);
                            route["alias_of"] = json!(TRADE_B);
                        }
                        Some(TRADE_B) => {
                            *route = a.clone();
                            route["id"] = json!(TRADE_B);
                            route["alias_of"] = json!(TRADE_A);
                        }
                        _ => {}
                    }
                }
            });
        });
        let report = check(&inputs).expect("the swapped successor reads");
        let text = report.failure_text();
        assert!(
            text.contains(TRADE_A) && text.contains(TRADE_B),
            "both operations moved and neither is declared by an entry the predecessor already \
             served: {text}"
        );
    }

    // ------------------------------------------------------------------------------------------
    // Adversarial pass 3. Added against eb7edec. Round 2 inverted the member rule from a whitelist
    // to a blacklist — `DECLARED_MEMBERS` names the three members a declaration may change and
    // everything else is compared — and gave `SCHEMA_MEMBERS` a modulus of exactly one member,
    // `$id`. These attack the two things that inversion cost: what the blacklist now refuses that
    // the story says must stay expressible, and what the `(method, path)` collision check added in
    // the same round still lets through.
    // ------------------------------------------------------------------------------------------

    /// The *other* route family `docs/design/16-sessions-are-not-pipe-sessions.md` moves. The
    /// design lists all seven ids it relocates (`:34`); `session.attach` is the one the cases above
    /// use, and this is the one whose result schema names the bundle version.
    const CAPABILITIES_ID: &str = "session.capabilities";
    const CAPABILITIES_FROM: &str = "/v1/pipe-sessions";
    const CAPABILITIES_TO: &str = "/v1/sessions";
    const CAPABILITIES_ALIAS: &str = "pipe-session.capabilities";
    /// The document `session.capabilities` answers with, in both bundles, under the same name.
    const CAPABILITIES_RESULT: &str = "schemas/results/pipe-session-capabilities.json";

    /// Authors design 16's declaration for `session.capabilities`, in the shape design 16 states:
    /// the canonical entry moves to `/v1/sessions`, and a **new** legacy entry
    /// `pipe-session.capabilities` stands at `/v1/pipe-sessions` naming it, "byte-identical to its
    /// target in every field except `id`, `path` and `alias_of` — same method, scope, risk,
    /// idempotency, effects, exposure, and the same address, input and result schemas"
    /// (`docs/design/16-sessions-are-not-pipe-sessions.md:78-80`).
    fn declare_the_capabilities_move(authored: &std::path::Path) {
        admit_alias_of(authored);
        edit_json(&authored.join("routes.json"), |routes| {
            let routes = routes.as_array_mut().expect("routes.json is an array");
            let mut alias = routes
                .iter()
                .find(|route| route.get("id").and_then(Value::as_str) == Some(CAPABILITIES_ID))
                .cloned()
                .expect("the capabilities route");
            assert_eq!(
                alias["result_schema"],
                json!(CAPABILITIES_RESULT),
                "{CAPABILITIES_ID} no longer answers with {CAPABILITIES_RESULT}"
            );
            for route in routes.iter_mut() {
                if route.get("id").and_then(Value::as_str) == Some(CAPABILITIES_ID) {
                    assert_eq!(
                        route["path"],
                        json!(CAPABILITIES_FROM),
                        "{CAPABILITIES_ID} moved already"
                    );
                    route["path"] = json!(CAPABILITIES_TO);
                }
            }
            alias["id"] = json!(CAPABILITIES_ALIAS);
            alias["path"] = json!(CAPABILITIES_FROM);
            alias["alias_of"] = json!(CAPABILITIES_ID);
            routes.push(alias);
        });
        // One route added, so the successor's own bundle schema states one.
        edit_json(&authored.join("documents/schemas/bundle.json"), |bundle| {
            bundle["properties"]["compatibility"]["properties"]["adds_routes"] = json!({
                "const": 1,
            });
        });
    }

    /// Acceptance evidence 3: "a deliberate, declared path change is still expressible, because
    /// `docs/design/16-sessions-are-not-pipe-sessions.md` needs one".
    ///
    /// It is not, for the route family that design actually moves. `SCHEMA_MEMBERS` compares the
    /// resolved documents modulo `$id` alone, and `schemas/results/pipe-session-capabilities.json`
    /// states the bundle's own version as a `const` — `substrate-wire/0.7.0` against
    /// `substrate-wire/0.8.0` — so the two documents differ **by construction at every version
    /// boundary**, exactly as `$id` does, and at 0.5.0→0.6.0 and 0.6.0→0.7.0 as well. The shim
    /// therefore "differs in `result_schema`" from the operation it is a byte-copy of, and the move
    /// is reported undeclared.
    ///
    /// The successor cannot author its way out. Pointing the shim at a frozen private copy of the
    /// predecessor's document is the one escape `SCHEMA_MEMBERS` documents, and it is the shape
    /// design 16 rejects in as many words: "the two paths share one result schema, which is the
    /// whole point of `alias_of`; marking only the old one means splitting that schema, which
    /// reintroduces the drift the alias exists to prevent" (`:113-116`). It would also make one
    /// daemon answer `substrate-wire/0.8.0` at one path and `substrate-wire/0.9.0` at the other.
    #[test]
    fn a_declared_move_of_the_capabilities_route_is_expressible() {
        // First, the mechanism, out of the released bundles rather than out of this comment: the
        // two versions of the shared document differ only where they name their own version.
        let contracts = root().join("contracts/substrate-wire");
        let mut published: Value = serde_json::from_slice(
            &std::fs::read(contracts.join(PREDECESSOR).join(CAPABILITIES_RESULT)).expect("read"),
        )
        .expect("parse");
        let mut offered: Value = serde_json::from_slice(
            &std::fs::read(contracts.join(VERSION).join(CAPABILITIES_RESULT)).expect("read"),
        )
        .expect("parse");
        assert_eq!(
            published.pointer("/properties/contract/const"),
            Some(&json!(format!("substrate-wire/{PREDECESSOR}"))),
        );
        assert_eq!(
            offered.pointer("/properties/contract/const"),
            Some(&json!(format!("substrate-wire/{VERSION}"))),
        );
        for document in [&mut published, &mut offered] {
            let object = document.as_object_mut().expect("an object");
            object.remove("$id");
            object["properties"]["contract"]["const"] = json!("");
        }
        assert_eq!(
            published, offered,
            "the two versions of {CAPABILITIES_RESULT} must differ only at $id and at the version \
             they name, or this case is testing something else"
        );

        let (_scratch, inputs) =
            author_a_successor("declared-capabilities-move", declare_the_capabilities_move);
        let report = check(&inputs).expect("the successor reads");
        assert!(
            report.failures().is_empty(),
            "design 16's own declaration for {CAPABILITIES_ID} — a new entry at \
             {CAPABILITIES_FROM} naming it, byte-identical in every other member — must be \
             expressible, and there is no other document the shim could name: {}",
            report.failure_text()
        );
    }

    /// The path `session.get` serves, with its one parameter renamed. A router matches the same
    /// URLs to both templates: a path parameter's name is local to the registry — it names a
    /// property of the generated address schema (`xtask/src/render.rs:355-366`) and nothing on the
    /// wire, because `hashing.json` substitutes the parameter rather than transmitting its name.
    const OCCUPIED_PATH_RENAMED: &str = "/v1/pipe-sessions/{workspace_id}";

    /// `a_declared_move_onto_an_occupied_path_is_refused` with one parameter renamed.
    ///
    /// Round 2 closed that case by keying a `(method, path)` map on the path **string**. The class
    /// it was an instance of is "a declared move onto a path a request cannot be told apart from
    /// one somebody else still serves", and string equality is not that class: `GET
    /// /v1/pipe-sessions/{workspace_id}` and `GET /v1/pipe-sessions/{session_id}` are two keys and
    /// one route. `GET /v1/pipe-sessions/01J…` reaches `session.attach`, a duplex byte channel,
    /// or `session.get`, according to which registration the router happened to install first —
    /// which is the harm `a_declared_move_onto_an_occupied_path_is_refused` was written for,
    /// reached through the declaration mechanism with no other edit.
    ///
    /// By the story's own acceptance statement this successor serves an existing operation id at a
    /// different path and must fail.
    #[test]
    fn a_declared_move_onto_a_parameter_renamed_occupied_path_is_refused() {
        let (_scratch, inputs) = author_a_successor("declared-move-onto-renamed", |authored| {
            declare_a_move_to(authored, OCCUPIED_PATH_RENAMED);
        });
        let registry: Value = serde_json::from_slice(
            &std::fs::read(inputs.contracts_root.join(VERSION).join("operations.json"))
                .expect("read the rendered registry"),
        )
        .expect("the rendered registry parses");
        let mut claimants: Vec<&str> = registry["operations"]
            .as_array()
            .expect("an operations array")
            .iter()
            .filter(|entry| {
                entry.get("method").and_then(Value::as_str) == Some("GET")
                    && matches!(
                        entry.get("path").and_then(Value::as_str),
                        Some(OCCUPIED_PATH | OCCUPIED_PATH_RENAMED)
                    )
            })
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .collect();
        claimants.sort_unstable();
        let mut expected = vec![MOVED_ID, OCCUPIED_ID];
        expected.sort_unstable();
        assert_eq!(
            claimants, expected,
            "the fixture must actually put two operations on one GET template shape"
        );

        let report = check(&inputs).expect("the successor reads");
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "{MOVED_ID} is served at {OCCUPIED_PATH_RENAMED}, which no request can be told apart \
             from {OCCUPIED_PATH} that {OCCUPIED_ID} still serves; a parameter's name is not a \
             discriminator: {text}"
        );
    }

    /// The `$defs` every generated address schema resolves through. `session-id` is reached by a
    /// `$ref` from `schemas/addresses/pipe-session-attach.json` and from every other address schema
    /// whose template carries `{session_id}` (`xtask/src/render.rs:355-366`).
    const NARROWED_DEF: &str = "session-id";
    /// A session id the predecessor's `session-id` admits. `0.8.0`'s own fixtures only ever use
    /// `ses_vector`, so narrowing to lower-case breaks nothing else in the bundle.
    const PINNED_SESSION_ID: &str = "ses_ABC123";

    /// The document comparison is not one level deep, and the sibling it cannot see is not one
    /// file.
    ///
    /// `SCHEMA_MEMBERS` compares the documents a shim names "as the documents they resolve to",
    /// but `without_id` compares the document's own bytes and never follows a `$ref`. 45 of the 74
    /// schema documents `0.8.0`'s operations name carry an external `$ref`, into five different
    /// siblings — `common.json` (56 references), `resource.json` (13), `event.json` (2),
    /// `capability.json` (1), `operation.json` (1) — and two of those five, `resource.json` and
    /// `capability.json`, are among the seven documents that actually changed at the last real
    /// version boundary. So the hole is not a corner: it is most of the surface, and the successor
    /// controls it.
    ///
    /// Here the successor narrows `common.json#/$defs/session-id` from `^ses_[A-Za-z0-9]+$` to
    /// lower-case only. The shim at `MOVED_FROM` is a byte-copy of the moved operation and its
    /// `address_schema` document is byte-identical to the predecessor's modulo `$id` — both are
    /// generated, so they cannot differ — and the move is declared. Every pinned consumer holding
    /// a session id with a digit or a capital in it now gets a refusal at the URL the declaration
    /// promised would keep answering exactly as before.
    #[test]
    fn an_alias_whose_address_schema_refs_a_narrowed_sibling_declares_nothing() {
        let narrow = |authored: &std::path::Path| {
            declare_alias(authored, MOVED_FROM);
            edit_json(&authored.join("documents/schemas/common.json"), |common| {
                let def = &mut common["$defs"][NARROWED_DEF];
                assert_eq!(
                    def["pattern"],
                    json!("^ses_[A-Za-z0-9]+$"),
                    "0.8.0's {NARROWED_DEF} is no longer the pattern this case narrows"
                );
                def["pattern"] = json!("^ses_[a-z]+$");
            });
        };

        let scratch = tempfile::Builder::new()
            .prefix("alias-narrowed-sibling")
            .tempdir()
            .expect("scratch");
        let source = scratch.path().join("bundle-source");
        let contracts = scratch.path().join("substrate-wire");
        copy_tree(
            &root().join("xtask/bundle-source").join(VERSION),
            &source.join(VERSION),
        );
        copy_tree(
            &root().join("contracts/substrate-wire").join(PREDECESSOR),
            &contracts.join(PREDECESSOR),
        );
        let authored = source.join(VERSION);
        edit_json(&authored.join("routes.json"), |routes| {
            for route in routes.as_array_mut().expect("routes.json is an array") {
                if route.get("id").and_then(Value::as_str) == Some(MOVED_ID) {
                    assert_eq!(route["path"], json!(MOVED_FROM), "{MOVED_ID} moved already");
                    route["path"] = json!(MOVED_TO);
                }
            }
        });
        narrow(&authored);
        let inputs = crate::render::Inputs {
            source_root: source,
            contracts_root: contracts.clone(),
            ..inputs()
        };
        let rendered = crate::render::render(&inputs).expect("the authored successor renders");
        for (path, bytes) in &rendered {
            let target = contracts.join(VERSION).join(path);
            std::fs::create_dir_all(target.parent().expect("a parent")).expect("create");
            std::fs::write(&target, bytes).expect("write");
        }

        // The narrowing is real: an id the predecessor's `session-id` admits, the successor's
        // refuses. Checked against the two `$defs` themselves, so nothing here depends on how the
        // gate resolves a reference.
        let admits = |root: &std::path::Path, id: &str| {
            let common: Value = serde_json::from_slice(
                &std::fs::read(root.join("schemas/common.json")).expect("read"),
            )
            .expect("parse");
            let def = common["$defs"][NARROWED_DEF].clone();
            jsonschema::draft202012::options()
                .build(&def)
                .expect("the $def compiles")
                .is_valid(&json!(id))
        };
        assert!(
            admits(&contracts.join(PREDECESSOR), PINNED_SESSION_ID),
            "{PREDECESSOR} must admit {PINNED_SESSION_ID}, or the case narrows nothing"
        );
        assert!(
            !admits(&contracts.join(VERSION), PINNED_SESSION_ID),
            "the successor must refuse {PINNED_SESSION_ID}, or the case narrows nothing"
        );

        // And the two address schema documents the shim and the predecessor's entry name are
        // byte-identical modulo `$id`, so the member comparison sees nothing at all.
        let address = |root: &std::path::Path| {
            let mut document: Value = serde_json::from_slice(
                &std::fs::read(root.join("schemas/addresses/pipe-session-attach.json"))
                    .expect("read"),
            )
            .expect("parse");
            document.as_object_mut().expect("an object").remove("$id");
            document
        };
        assert_eq!(
            address(&contracts.join(PREDECESSOR)),
            address(&contracts.join(VERSION)),
            "the case only says something if the compared documents are equal"
        );

        let report = check(&inputs).expect("the successor reads");
        let text = report.failure_text();
        assert!(
            text.contains(MOVED_ID),
            "{MOVED_FROM} no longer accepts the session ids {PREDECESSOR} accepted there, so the \
             old path does not keep answering as before and the move is undeclared: {text}"
        );
    }

    // ------------------------------------------------------------------------------------------
    // The rest of the three classes pass 3 found instances of. Each red case above is one member
    // of a class; these are the members it did not name, and the two rules it did not exercise.
    // ------------------------------------------------------------------------------------------

    /// Class: two path templates a request cannot tell apart are one route.
    ///
    /// The red case is one instance — a declared move onto a parameter-renamed occupied path. The
    /// rule itself is `path_shape`, and both readers ask it: the collision map and the move test.
    /// Pinned here directly, including the two shapes no released bundle has.
    #[test]
    fn a_path_shape_is_what_a_request_can_tell_apart() {
        use super::path_shape;
        // A parameter's name is not a discriminator: matchit refuses the second registration.
        assert_eq!(
            path_shape("/v1/pipe-sessions/{session_id}"),
            path_shape("/v1/pipe-sessions/{workspace_id}")
        );
        assert_eq!(path_shape("/v1/x/{a}/y/{b}"), path_shape("/v1/x/{q}/y/{r}"));
        // Everything else still is one.
        assert_ne!(path_shape("/v1/sessions/{a}"), path_shape("/v1/execs/{a}"));
        assert_ne!(path_shape("/v1/x/{a}"), path_shape("/v1/x/{a}/attach"));
        assert_ne!(path_shape("/v1/x/{a}"), path_shape("/v1/x/literal"));
        // A wildcard spans segments where a parameter spans one. No released bundle has one; the
        // rule is right for the day one arrives rather than the day after.
        assert_ne!(path_shape("/v1/x/{*rest}"), path_shape("/v1/x/{rest}"));
        // An unterminated brace is not a template and is not quietly turned into one — nor
        // quietly dropped, which is the other way to get it wrong and the one a comparison with
        // `/v1/x/{a}` alone does not catch.
        assert_ne!(path_shape("/v1/x/{a"), path_shape("/v1/x/{a}"));
        assert_ne!(path_shape("/v1/x/{a"), path_shape("/v1/x/"));
    }

    /// And the rule holds where it has to hold: no released bundle serves one method and shape
    /// twice. `Routes::read` refuses that, so a bundle that did could not be verified at all.
    #[test]
    fn every_released_bundle_serves_one_operation_per_method_and_shape() {
        let contracts = root().join("contracts/substrate-wire");
        let mut checked = 0;
        for entry in std::fs::read_dir(&contracts).expect("read contracts") {
            let bundle = entry.expect("entry").path();
            if !bundle.is_dir() {
                continue;
            }
            let registry: Value = serde_json::from_slice(
                &std::fs::read(bundle.join("operations.json")).expect("read the registry"),
            )
            .expect("the registry parses");
            super::Routes::read(&registry)
                .unwrap_or_else(|error| panic!("{}: {error}", bundle.display()));
            checked += 1;
        }
        assert_eq!(checked, 8, "every released bundle must be checked");
    }

    /// Class, the other half: if a parameter's name is not a discriminator, then renaming one is
    /// not a move — no concrete URL changed and there is nothing for a consumer to have pinned
    /// differently. The same rule that refuses the collision has to dissolve this, or it is two
    /// rules wearing one name.
    ///
    /// `{session_id}` becomes `{exec_id}`, which the generated address schema resolves through a
    /// definition `common.json` already has, so the successor renders. No shim, no `alias_of`, and
    /// the bundle verifies.
    #[test]
    fn renaming_a_path_parameter_is_not_a_move() {
        let (_scratch, inputs) = author_a_successor("parameter-renamed", |authored| {
            edit_json(&authored.join("routes.json"), |routes| {
                for route in routes.as_array_mut().expect("routes.json is an array") {
                    if route.get("id").and_then(Value::as_str) == Some(MOVED_ID) {
                        assert_eq!(route["path"], json!(MOVED_FROM), "{MOVED_ID} moved already");
                        route["path"] = json!("/v1/pipe-sessions/{exec_id}/attach");
                    }
                }
            });
        });
        let report = check(&inputs).expect("the successor reads");
        assert!(
            report.failures().is_empty(),
            "a parameter's name is local to the registry, so renaming one moves nothing: {}",
            report.failure_text()
        );
    }

    /// Class: a schema document means itself *and* everything it reaches.
    ///
    /// The red case narrows a sibling one `$ref` away. `0.8.0`'s own closures go three deep —
    /// `operation.get`, `event.list` and `reconciliation.snapshot.get` reach `common.json` and
    /// `error.json` through `resource.json` and `operation.json` — and no route that is
    /// closure-equal across `0.7.0`→`0.8.0` is more than one hop deep, so the deeper half of the
    /// class cannot be built out of the released bundles. It is built here instead.
    #[test]
    fn the_closure_follows_a_reference_further_than_one_hop() {
        let far = |pattern: &str| {
            tree_of_documents(&[
                (
                    "schemas/addresses/a.json",
                    json!({ "properties": { "id": { "$ref": "../b.json#/$defs/id" } } }),
                ),
                (
                    "schemas/b.json",
                    json!({ "$defs": { "id": { "$ref": "common.json#/$defs/id" } } }),
                ),
                (
                    "schemas/common.json",
                    json!({ "$defs": { "id": { "pattern": pattern } } }),
                ),
            ])
        };
        let (wide, narrow) = (far("^x[0-9]+$"), far("^x1$"));
        let closure = |tree: &super::Tree| {
            super::closure_of(
                &super::Side::Rendered {
                    tree,
                    version: VERSION,
                },
                "schemas/addresses/a.json",
            )
        };
        let (wide, narrow) = (closure(&wide), closure(&narrow));
        assert!(wide.unresolved.is_empty() && narrow.unresolved.is_empty());
        assert_eq!(
            wide.documents.keys().collect::<Vec<_>>(),
            vec![
                "schemas/addresses/a.json",
                "schemas/b.json",
                "schemas/common.json"
            ],
            "the closure must reach the document two hops away"
        );
        assert_ne!(
            wide.documents, narrow.documents,
            "a narrowing two hops from the named document is still a narrowing"
        );
    }

    /// A `$ref` cycle is legal in a schema tree and must terminate rather than recurse. If this
    /// case ever hangs instead of failing, the bound in `closure_of` is what went.
    #[test]
    fn the_closure_terminates_on_a_reference_cycle() {
        let tree = tree_of_documents(&[
            ("schemas/a.json", json!({ "$ref": "b.json#/$defs/x" })),
            (
                "schemas/b.json",
                json!({ "$defs": { "x": { "$ref": "a.json" } } }),
            ),
        ]);
        let closure = super::closure_of(
            &super::Side::Rendered {
                tree: &tree,
                version: VERSION,
            },
            "schemas/a.json",
        );
        assert!(closure.unresolved.is_empty());
        assert_eq!(closure.documents.len(), 2, "both documents, once each");
    }

    /// A reference that names nothing, and one that climbs out of the bundle, are carried rather
    /// than dropped. Two bundles that both dangle are not thereby equal: an unresolved reference is
    /// not a document a consumer can be shown to still get, so it fails closed.
    #[test]
    fn an_unresolvable_reference_is_carried_not_dropped() {
        let tree = tree_of_documents(&[(
            "schemas/a.json",
            json!({
                "allOf": [
                    { "$ref": "gone.json#/$defs/x" },
                    { "$ref": "../../outside.json" },
                ],
            }),
        )]);
        let closure = super::closure_of(
            &super::Side::Rendered {
                tree: &tree,
                version: VERSION,
            },
            "schemas/a.json",
        );
        assert_eq!(
            closure.unresolved,
            [
                "../../outside.json".to_owned(),
                "schemas/gone.json".to_owned()
            ]
            .into_iter()
            .collect(),
            "both the missing sibling and the escaping reference must be named"
        );
    }

    /// Class: a member that states the bundle's own version differs at every boundary by
    /// construction, and does it in more than one shape.
    ///
    /// The red case is the one shape a route can reach — `substrate-wire/<version>`, in
    /// `schemas/results/pipe-session-capabilities.json`. The other three live in `bundle.json`,
    /// `compatibility.json`, `origins.json` and `packaging.json`, which no route's schema reaches,
    /// so they are pinned here rather than through a rendered successor.
    #[test]
    fn every_shape_a_bundle_states_its_own_version_in_is_normalised() {
        use super::without_version;
        for stated in [
            "0.8.0",
            "substrate-wire/0.8.0",
            "substrate-wire@0.8.0",
            "urn:b10x:substrate-wire:0.8.0:result:pipe-session-capabilities",
            "scripts/render-contract-bundle-0.8.0.py",
        ] {
            assert_eq!(
                without_version(stated, "0.8.0"),
                without_version(&stated.replace("0.8.0", "0.7.0"), "0.7.0"),
                "{stated} states the bundle's own version and must normalise away"
            );
        }
        // And a version inside a longer number is not a version.
        assert_eq!(without_version("10.8.05", "0.8.0"), "10.8.05");
        assert_eq!(without_version("v0.8.0", "0.8.0"), "v{version}");
        // A string that says nothing about the version is untouched.
        assert_eq!(
            without_version("^ses_[A-Za-z0-9]+$", "0.8.0"),
            "^ses_[A-Za-z0-9]+$"
        );
    }

    /// A reference that resolves in neither bundle is not a document a consumer can be shown to
    /// still get, so the comparison fails closed rather than calling two equally blind sides equal.
    ///
    /// Nothing a successor can render produces this — a dangling `$ref` fails classification long
    /// before it reaches here — so the two `Side`s are built directly. Without it, the `unresolved`
    /// set is computed and never read, which is how the last dead field got in.
    #[test]
    fn two_sides_that_dangle_the_same_way_are_not_thereby_equal() {
        let entry = json!({ "address_schema": "schemas/addresses/a.json", "method": "GET" });
        let was = route_named(MOVED_ID, MOVED_FROM, entry.clone());
        let mut declaring = entry.clone();
        declaring["alias_of"] = json!(MOVED_ID);
        let shim = route_named(ALIAS_ID, MOVED_FROM, declaring);

        // One document, one reference, and nothing it names — identically, on both sides.
        let dangling = tree_of_documents(&[(
            "schemas/addresses/a.json",
            json!({ "properties": { "id": { "$ref": "../common.json#/$defs/id" } } }),
        )]);
        let differing = super::declaration_differences(
            &was,
            &shim,
            &super::Side::Rendered {
                tree: &dangling,
                version: PREDECESSOR,
            },
            &super::Side::Rendered {
                tree: &dangling,
                version: VERSION,
            },
        );
        assert_eq!(
            differing,
            vec!["address_schema".to_owned()],
            "a member whose document reaches nothing is a difference, not a match"
        );
    }

    fn route_named(id: &str, path: &str, entry: Value) -> super::Route {
        super::Route {
            id: id.to_owned(),
            path: path.to_owned(),
            shape: super::path_shape(path),
            entry,
        }
    }

    fn tree_of_documents(documents: &[(&str, Value)]) -> super::Tree {
        documents
            .iter()
            .map(|(path, document)| {
                (
                    (*path).to_owned(),
                    serde_json::to_vec(document).expect("serialize"),
                )
            })
            .collect()
    }

    /// Two routes whose schema closures did **not** change across the boundary. Both are `GET`,
    /// both are among the eleven of `0.8.0`'s twenty-six whose `address_schema`, `input_schema` and
    /// `result_schema` closures are equal to `0.7.0`'s, so nothing except "a shim is new" stands
    /// between a swap of the two and a verified bundle.
    const SWAP_A: &str = "session.attach";
    const SWAP_B: &str = "session.capabilities";

    /// `two_existing_operations_may_not_declare_each_others_moves` stopped pinning the rule it was
    /// written for, and this replaces it rather than relaxing it.
    ///
    /// That case swaps `session.attach` with `session.get`, and `session.get`'s result schema
    /// reaches `resource.json`, which `0.8.0` changed for the aperture ceiling. Once
    /// [`SCHEMA_MEMBERS`] began comparing the `$ref` closure, one half of that swap started failing
    /// on `result_schema` for a reason that has nothing to do with the swap — and the failure text
    /// names both operations, so the case still passed with "a shim is new" deleted while
    /// `session.attach`'s own move was being declared by an operation the predecessor already
    /// served. Verified by deleting that filter and driving the fixture: one of the two moves is
    /// reported, the other is not.
    ///
    /// Here both routes' closures are unchanged, so the member comparison has nothing to say and
    /// the filter is the only thing left. Each entry keeps its id and takes the other's whole
    /// contract, including its path, and names the other in `alias_of`.
    #[test]
    fn an_identity_swap_between_two_unchanged_routes_declares_nothing() {
        let (_scratch, inputs) = author_a_successor("swapped-unchanged", |authored| {
            admit_alias_of(authored);
            edit_json(&authored.join("routes.json"), |routes| {
                let routes = routes.as_array_mut().expect("routes.json is an array");
                let of = |routes: &Vec<Value>, id: &str| {
                    routes
                        .iter()
                        .find(|route| route.get("id").and_then(Value::as_str) == Some(id))
                        .cloned()
                        .expect("the route to swap")
                };
                let (a, b) = (of(routes, SWAP_A), of(routes, SWAP_B));
                assert_eq!(
                    a["method"], b["method"],
                    "the swap must not change the method"
                );
                for route in routes.iter_mut() {
                    match route.get("id").and_then(Value::as_str) {
                        Some(SWAP_A) => {
                            *route = b.clone();
                            route["id"] = json!(SWAP_A);
                            route["alias_of"] = json!(SWAP_B);
                        }
                        Some(SWAP_B) => {
                            *route = a.clone();
                            route["id"] = json!(SWAP_B);
                            route["alias_of"] = json!(SWAP_A);
                        }
                        _ => {}
                    }
                }
            });
        });
        let report = check(&inputs).expect("the swapped successor reads");
        // Both moves must be reported, and each must be reported *as its own finding* — a message
        // about one that happens to mention the other is what let this rule go unpinned before.
        for moved in [SWAP_A, SWAP_B] {
            assert!(
                report
                    .failures()
                    .iter()
                    .any(|failure| failure.contains(&format!("route {moved} served by"))),
                "{moved} moved and is declared only by an entry {PREDECESSOR} already served: {}",
                report.failure_text()
            );
        }
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
