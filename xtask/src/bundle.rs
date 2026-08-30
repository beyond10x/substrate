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
    // A path is pinned as firmly as an id, and moving one is invisible to everything above: the id
    // difference is empty, so the counts are the predecessor's own and nothing is dropped. A
    // deliberate move stays expressible — `docs/design/16-sessions-are-not-pipe-sessions.md` needs
    // one — by keeping the predecessor's path answering through an entry whose `alias_of` names the
    // operation that moved. That declaration is *read* here, not a switch: an alias serving some
    // other path leaves the old one unanswered and fails exactly as an undeclared move does.
    for (id, was) in &previous.served {
        let Some(now) = current.served.get(id) else {
            continue;
        };
        if current.still_answers(was, &previous_ids) {
            continue;
        }
        failures.push(format!(
            "operations.json: route {id} served by {predecessor} at {} is served at {}; a \
             successor that moves a path keeps {} answering through a new entry whose alias_of is \
             {id}, serving it with the same {} — or it has moved a path a consumer pinned",
            was.path,
            now.path,
            was.path,
            ANSWERED_BY.join(", ")
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

/// What a request to one route is answered by.
///
/// These are the registry fields that decide whether a request a consumer already sends is answered
/// at all, and answered the same way: the wrong `method` is a 405, the wrong `required_scope` is a
/// 403 against a token that used to work, and `idempotency`, `input_binding`, `input_schema` and
/// `result_schema` are the request envelope the consumer builds and the response it parses. An
/// entry that differs in any of them is serving something else at that URL.
///
/// It stops there deliberately. `risk`, `effects` and `exposure` describe an operation to a reader
/// rather than to a client; `capability_predicates` decide a refusal the driver owns, not the shape
/// of the exchange; and `address_schema` follows from the path, which is compared exactly. The
/// stronger rule — that a declared alias is byte-identical to the operation it stands in for in
/// every field but `id`, `path` and `alias_of` — is a claim about one successor's own additions
/// (`docs/design/16-sessions-are-not-pipe-sessions.md`), and belongs in that version's arm of
/// [`check_additions`], not in the cross-version compatibility check.
const ANSWERED_BY: [&str; 6] = [
    "method",
    "required_scope",
    "idempotency",
    "input_binding",
    "input_schema",
    "result_schema",
];

/// One entry of a bundle's operation registry, kept whole so a declaration can be compared field by
/// field against the operation it stands in for.
#[derive(Debug, Clone)]
struct Route {
    id: String,
    path: String,
    entry: Value,
}

impl Route {
    fn field(&self, name: &str) -> Option<&str> {
        self.entry.get(name).and_then(Value::as_str)
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
                entry: entry.clone(),
            };
            if let Some(target) = entry.get("alias_of").and_then(Value::as_str) {
                routes
                    .aliased
                    .entry(target.to_owned())
                    .or_default()
                    .push(route.clone());
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

    /// Whether a request the predecessor answered at `was.path` is still answered there, the same
    /// way, by this successor.
    ///
    /// Either the operation has not moved, or the path it left is served by a **declaration**, and
    /// a declaration is three things read out of the registry, never one:
    ///
    /// 1. **The declaring entry is new.** An id the predecessor already served may not stand in for
    ///    somebody else's move — two existing operations naming each other as `alias_of` would
    ///    trade paths and each "declare" the other, with every id preserved and nothing added.
    /// 2. **It is served at exactly the path the operation left**, so the URL a consumer pinned
    ///    still resolves.
    /// 3. **It answers there as the predecessor did**, in every field of [`ANSWERED_BY`]. An entry
    ///    parked on the old path under another method or another scope resolves the URL and refuses
    ///    the request, which is the withdrawal this check exists to catch, not a declaration of it.
    fn still_answers(&self, was: &Route, previous_ids: &BTreeSet<&str>) -> bool {
        if self
            .served
            .get(&was.id)
            .is_some_and(|now| now.path == was.path)
        {
            return true;
        }
        self.aliased
            .get(&was.id)
            .into_iter()
            .flatten()
            .any(|alias| {
                !previous_ids.contains(alias.id.as_str())
                    && alias.path == was.path
                    && ANSWERED_BY
                        .iter()
                        .all(|field| alias.field(field) == was.field(field))
            })
    }
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
