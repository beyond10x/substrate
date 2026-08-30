//! `cargo xtask check-json` — closed JSON classification for every released contract bundle.
//!
//! Ported from `scripts/contract_json_gate.py`, which was shared live machinery rather than a
//! frozen per-version artefact: the four `check-contract-bundle*.py` checkers imported
//! `check_json_authority` from it, and `scripts/test_contract_json_gate.py` was its self-test and a
//! gate step of its own. Anything that runs in a b10x foundation repository is Rust
//! (`atlas/AGENTS.md` § *Language*).
//!
//! Invariant 7, which this verb is: **every created JSON authority has exactly one schema
//! classification and validates in the gate; unclassified JSON fails closed.** For every `*.json`
//! beneath a released bundle directory, in sorted path order:
//!
//! 1. it parses, rejects duplicate object keys, and is in deterministic source form — the exact
//!    bytes `json.dumps(indent=2, sort_keys=True, ensure_ascii=False)` produces;
//! 2. under `schemas/` it declares the pinned Draft 2020-12 meta-schema and meta-validates;
//! 3. anywhere else it declares exactly one `$schema` resolving inside the bundle and under
//!    `schemas/`, whose target is itself a Draft 2020-12 authority — and it validates against that
//!    target, both with the bundle's own closed subset validator and with the pinned standards
//!    validator.
//!
//! `0.1.0` is the one exception, and it is one the predecessor carried too: it predates bundled
//! schemas for its five fixed root
//! authorities, so those are classified *externally* by [`fixed_authority_schemas`] rather than by
//! rewriting immutable bytes (invariant 6).
//!
//! **Two validators run over each classified document, because the predecessor ran two and they
//! catch different things.** The standards validator (`jsonschema`, pinned `=0.49.9`) is a complete
//! Draft 2020-12 implementation and is the authority on the standard. [`validate`] is the bundle's
//! own closed subset validator, and it enforces `x-b10x-max-depth` — a keyword the standard knows
//! nothing about and which a conforming implementation is *required* to ignore. Dropping it would
//! have been a silent weakening, which is the one thing this port must not do.
//!
//! Where the predecessor shelled out to `crates/substrate-contract-check` for the standards pass
//! (`scripts/contract_json_gate.py:195`), this runs the same `jsonschema` calls in process: the
//! subprocess existed to get a Rust validator into a Python program, and there is no longer a
//! Python program.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use anyhow::{Context, Result};
use clap::Parser;
use fancy_regex::{Regex, RegexBuilder};
use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value, json};

use crate::repo;
use crate::report::Report;

/// The one meta-schema a bundle schema authority may declare.
const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
/// The URI namespace classified schemas are registered under while they are validated. A
/// wire-visible b10x identifier, reserved and unroutable by RFC 6761 `.invalid`.
const RESOURCE_ROOT: &str = "https://b10x.invalid/substrate-wire";

/// `cargo xtask check-json [<version>...]`.
#[derive(Debug, Parser)]
pub struct Args {
    /// Bundle versions to classify (default: every directory under the bundle root).
    pub versions: Vec<String>,
    /// Released bundle root (default `contracts/substrate-wire`).
    #[arg(long, value_name = "DIR")]
    pub contracts_root: Option<PathBuf>,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let root = repo::root()?;
    let contracts = args
        .contracts_root
        .clone()
        .unwrap_or_else(|| root.join("contracts/substrate-wire"));
    Ok(check(&root, &contracts, &args.versions)?.emit())
}

/// Classify every named bundle, or every bundle directory there is.
///
/// # Errors
///
/// Returns an error only when the bundle root cannot be listed. A bundle that reads but does not
/// hold produces a [`Report`] of failures, so the gate prints all of them rather than the first.
pub fn check(repo_root: &Path, contracts: &Path, versions: &[String]) -> Result<Report> {
    let versions = if versions.is_empty() {
        bundle_versions(contracts)?
    } else {
        versions.to_vec()
    };

    let mut failures = Vec::new();
    let mut inventory = Vec::new();
    let mut total = 0usize;
    for version in &versions {
        let bundle = contracts.join(version);
        // The predecessor read a missing directory as an empty one: `Path.rglob` on an absent path
        // yields nothing, so a mistyped version classified zero documents and passed. A named
        // guarantee that cannot be checked is a refusal here (invariant 3).
        if !bundle.is_dir() {
            failures.push(format!(
                "substrate-wire/{version}: no bundle directory at {}",
                bundle.display()
            ));
            continue;
        }
        let (count, local) = check_bundle(&bundle, version, repo_root);
        inventory.push(format!("{version} {count}"));
        total += count;
        failures.extend(
            local
                .into_iter()
                .map(|failure| format!("substrate-wire/{version}: {failure}")),
        );
    }

    if failures.is_empty() {
        return Ok(Report::passed(format!(
            "contract JSON classified: {} ({total} documents in {} bundles)",
            inventory.join(", "),
            inventory.len()
        )));
    }
    Ok(Report::failed(failures))
}

/// Every directory directly under the released bundle root, in sorted order.
fn bundle_versions(contracts: &Path) -> Result<Vec<String>> {
    let mut versions = Vec::new();
    let entries =
        fs::read_dir(contracts).with_context(|| format!("cannot list {}", contracts.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot list {}", contracts.display()))?;
        if entry.path().is_dir() {
            versions.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    versions.sort();
    Ok(versions)
}

/// One bundle directory: how many JSON documents it holds, and every failure, in order.
///
/// This is `check_json_authority` (`scripts/contract_json_gate.py:275`). The predecessor took the
/// subset validator as a callback because each checker's `validate` closed over its own
/// module-global `BUNDLE`; here the bundle is a parameter, which is the same thing without the
/// global.
#[allow(clippy::too_many_lines)] // One pass over one bundle, in the predecessor's exact order.
pub fn check_bundle(bundle: &Path, version: &str, repo_root: &Path) -> (usize, Vec<String>) {
    let mut failures = Vec::new();
    let documents = Documents::new(repo_root);
    let embedded = if version == "0.1.0" {
        fixed_authority_schemas(version)
    } else {
        BTreeMap::new()
    };
    let bundle_resolved = resolve_path(bundle);
    let schemas_resolved = resolve_path(&bundle.join("schemas"));

    let mut count = 0usize;
    let mut records: Vec<Record> = Vec::new();
    let mut resources: Vec<Resource> = Vec::new();
    let mut resource_uris: BTreeMap<String, String> = BTreeMap::new();

    for path in json_paths(bundle) {
        count += 1;
        let relative = path
            .strip_prefix(bundle)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .into_owned();
        let Some(document) = documents.load(&path, &mut failures) else {
            continue;
        };

        if relative.starts_with("schemas/") {
            if declared_meta_schema(&document) != Some(DRAFT_2020_12) {
                failures.push(format!(
                    "{relative}: schema authority must declare the pinned Draft 2020-12 meta-schema"
                ));
            }
            records.push(Record::Meta {
                label: relative,
                schema: (*document).clone(),
            });
            continue;
        }

        let mut contract = embedded.get(&relative).cloned().map(Rc::new);
        let mut schema_path = bundle.join(format!(".embedded/{relative}.schema"));
        if let Some(fixed) = contract.as_ref() {
            records.push(Record::Meta {
                label: format!("fixed:{relative}"),
                schema: (**fixed).clone(),
            });
        }

        if contract.is_none() {
            let Some(declaration) = document
                .as_object()
                .and_then(|object| object.get("$schema"))
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                failures.push(format!(
                    "{relative}: unclassified JSON authority (missing exact schema mapping)"
                ));
                continue;
            };
            let target = resolve_path(&parent_of(&path).join(&declaration));
            if !target.starts_with(&bundle_resolved) {
                failures.push(format!(
                    "{relative}: declared schema escapes bundle: {declaration}"
                ));
                continue;
            }
            if !target.starts_with(&schemas_resolved) {
                failures.push(format!(
                    "{relative}: declared schema is not under schemas/: {declaration}"
                ));
                continue;
            }
            let Some(declared) = documents.load(&target, &mut failures) else {
                failures.push(format!(
                    "{relative}: declared schema is unavailable: {declaration}"
                ));
                continue;
            };
            if declared_meta_schema(&declared) != Some(DRAFT_2020_12) {
                failures.push(format!(
                    "{relative}: declared target is not a Draft 2020-12 schema authority: {declaration}"
                ));
                continue;
            }
            contract = Some(declared);
            schema_path = target;
        }

        let contract = contract.expect("classified above, or the loop continued");
        let errors = validate(
            &document,
            &contract,
            &schema_path,
            bundle,
            &documents,
            "$",
            &mut failures,
        );
        failures.extend(
            errors
                .into_iter()
                .map(|error| format!("{relative}: classified schema validation: {error}")),
        );

        let key = format!(
            "{}#{}",
            resolve_path(&schema_path).display(),
            serde_json::to_string(&*contract).unwrap_or_default()
        );
        let schema_uri = if let Some(uri) = resource_uris.get(&key) {
            uri.clone()
        } else {
            let uri = format!(
                "{RESOURCE_ROOT}/{version}/classified/{}",
                resource_uris.len()
            );
            let resolved = match dereference_schema(
                &contract,
                &schema_path,
                &bundle_resolved,
                &documents,
                &[],
                &mut failures,
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    failures.push(format!(
                        "{relative}: standards schema resolution failed: {error}"
                    ));
                    continue;
                }
            };
            resource_uris.insert(key, uri.clone());
            resources.push(Resource {
                uri: uri.clone(),
                schema: resolved,
            });
            uri
        };
        records.push(Record::Instance {
            label: relative,
            schema_uri,
            instance: (*document).clone(),
        });
    }

    failures.extend(standards_errors(&resources, &records));
    (count, failures)
}

fn declared_meta_schema(document: &Value) -> Option<&str> {
    document
        .as_object()
        .and_then(|object| object.get("$schema"))
        .and_then(Value::as_str)
}

/// `Path.parent` — the directory a relative declaration is resolved against.
fn parent_of(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new("")).to_path_buf()
}

/// Every `*.json` beneath `bundle`, sorted by full path exactly as `sorted(Path.rglob(...))` sorts
/// it. Symlinked directories are not descended into, matching `recurse_symlinks=False`.
///
/// A *directory* named `something.json` counts, and is then read as a document and refused —
/// `rglob` matched entries, not files, so that is what the predecessor did, and a directory is a
/// place a document could otherwise be parked where nothing classifies it.
fn json_paths(bundle: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_json(bundle, &mut found);
    found.sort_by(|left, right| {
        left.as_os_str()
            .as_encoded_bytes()
            .cmp(right.as_os_str().as_encoded_bytes())
    });
    found
}

fn collect_json(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            found.push(path.clone());
        }
        if kind.is_dir() {
            collect_json(&path, found);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------------------------

/// Every JSON document the gate has read: parsed once, cached, with its failures recorded.
///
/// A load that *fails* is deliberately not cached, so a document referenced twice reports its
/// failure twice — the predecessor's behaviour (`Documents.load` caches only after a successful
/// parse), and what makes a broken shared schema visible at each of its referents.
pub struct Documents {
    repo_root: PathBuf,
    cache: RefCell<BTreeMap<PathBuf, Rc<Value>>>,
}

impl Documents {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            repo_root: resolve_path(repo_root),
            cache: RefCell::new(BTreeMap::new()),
        }
    }

    /// The document at `path`, or `None` with the reason recorded in `failures`.
    pub fn load(&self, path: &Path, failures: &mut Vec<String>) -> Option<Rc<Value>> {
        let path = resolve_path(path);
        if let Some(cached) = self.cache.borrow().get(&path) {
            return Some(cached.clone());
        }
        let label = self.display(&path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                failures.push(format!(
                    "{label}: invalid JSON: {}",
                    os_error_text(&error, &path)
                ));
                return None;
            }
        };
        let value = match strict_parse(&text) {
            Ok(value) => value,
            Err(message) => {
                failures.push(format!("{label}: invalid JSON: {message}"));
                return None;
            }
        };
        let mut rendered = serde_json::to_string_pretty(&value).unwrap_or_default();
        rendered.push('\n');
        if text != rendered {
            failures.push(format!("{label}: JSON is not in deterministic source form"));
        }
        let value = Rc::new(value);
        self.cache.borrow_mut().insert(path, value.clone());
        Some(value)
    }

    /// Repository-relative where possible, absolute otherwise — the predecessor's `display`.
    fn display(&self, resolved: &Path) -> String {
        resolved
            .strip_prefix(&self.repo_root)
            .unwrap_or(resolved)
            .to_string_lossy()
            .into_owned()
    }
}

/// `str(OSError)` as `CPython` renders it, so a missing or unreadable schema reads the same.
fn os_error_text(error: &std::io::Error, path: &Path) -> String {
    let rendered = error.to_string();
    let reason = rendered
        .split(" (os error ")
        .next()
        .unwrap_or(rendered.as_str())
        .to_owned();
    match error.raw_os_error() {
        Some(code) => format!(
            "[Errno {code}] {reason}: {}",
            python_repr_str(&path.to_string_lossy())
        ),
        None => reason,
    }
}

// ---------------------------------------------------------------------------------------------
// Strict parsing: a duplicate object key is a refusal, not a last-one-wins merge
// ---------------------------------------------------------------------------------------------

thread_local! {
    /// The duplicate key that aborted the most recent parse. A `serde` error carries a position
    /// suffix the predecessor's `ValueError` did not, so the key travels beside it, not inside it.
    static DUPLICATE_KEY: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn strict_parse(text: &str) -> Result<Value, String> {
    DUPLICATE_KEY.with(|cell| cell.borrow_mut().take());
    match serde_json::from_str::<Strict>(text) {
        Ok(Strict(value)) => Ok(value),
        Err(error) => Err(DUPLICATE_KEY
            .with(|cell| cell.borrow_mut().take())
            .map_or_else(
                || error.to_string(),
                |key| format!("duplicate object key {}", python_repr_str(&key)),
            )),
    }
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(item)) = sequence.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let Strict(value) = map.next_value()?;
            if object.insert(key.clone(), value).is_some() {
                DUPLICATE_KEY.with(|cell| *cell.borrow_mut() = Some(key));
                return Err(de::Error::custom("duplicate object key"));
            }
        }
        Ok(Value::Object(object))
    }
}

// ---------------------------------------------------------------------------------------------
// The bundle's own closed subset validator
// ---------------------------------------------------------------------------------------------

/// Validate `instance` against `contract`, returning one message per violation.
///
/// The keyword set is the bundle's, not the standard's: deliberately small, ignoring what it does
/// not recognise exactly as the standard requires, and adding `x-b10x-max-depth`, which no
/// standards validator is allowed to enforce.
pub fn validate(
    instance: &Value,
    contract: &Value,
    schema_path: &Path,
    bundle: &Path,
    documents: &Documents,
    location: &str,
    failures: &mut Vec<String>,
) -> Vec<String> {
    validate_at(
        instance,
        contract,
        schema_path,
        bundle,
        documents,
        location,
        failures,
        0,
    )
}

/// How many `$ref` hops one validation may follow before the schema is called circular.
///
/// The predecessor had no limit and no cycle check on this path — only `dereference_schema` had
/// one — so `{"$ref": "a"} -> {"$ref": "b"} -> {"$ref": "a"}` recursed until `CPython` raised
/// `RecursionError` and printed a thousand frames where the gate's own output belonged. A missing
/// guarantee is a named refusal, never a crash (invariant 3).
///
/// 64 is two orders of magnitude above anything a released bundle contains — the deepest real
/// chain is `vector.json` -> `common.json#/$defs/...`, two hops — and shallow enough that the
/// unwind fits a 2 MiB test thread, which 256 did not.
const MAX_REFERENCE_DEPTH: u32 = 64;

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn validate_at(
    instance: &Value,
    contract: &Value,
    schema_path: &Path,
    bundle: &Path,
    documents: &Documents,
    location: &str,
    failures: &mut Vec<String>,
    depth: u32,
) -> Vec<String> {
    let contract = match contract {
        Value::Bool(true) => return Vec::new(),
        Value::Bool(false) => return vec![format!("{location}: false schema rejects instance")],
        Value::Object(object) => object,
        _ => return vec![format!("{location}: schema is not an object or boolean")],
    };
    let mut errors: Vec<String> = Vec::new();

    if let Some(Value::String(reference)) = contract.get("$ref") {
        if depth >= MAX_REFERENCE_DEPTH {
            return vec![format!(
                "{location}: invalid $ref {}: schema references nest more than {MAX_REFERENCE_DEPTH} deep",
                python_repr_str(reference)
            )];
        }
        match resolve_ref(reference, schema_path, bundle, documents, failures) {
            Ok((resolved, resolved_path)) => errors.extend(validate_at(
                instance,
                &resolved,
                &resolved_path,
                bundle,
                documents,
                location,
                failures,
                depth + 1,
            )),
            Err(error) => errors.push(format!(
                "{location}: invalid $ref {}: {error}",
                python_repr_str(reference)
            )),
        }
    }

    if let Some(expected) = contract.get("const")
        && !same_json(instance, expected)
    {
        errors.push(format!(
            "{location}: expected const {}",
            python_repr(expected)
        ));
    }
    if let Some(Value::Array(candidates)) = contract.get("enum")
        && !candidates
            .iter()
            .any(|candidate| same_json(instance, candidate))
    {
        errors.push(format!("{location}: value is outside enum"));
    }

    let accepted: Vec<&str> = match contract.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !accepted.is_empty()
        && !accepted
            .iter()
            .any(|expected| instance_type(instance, expected))
    {
        errors.push(format!(
            "{location}: expected type {}, got {}",
            accepted.join("|"),
            python_type_name(instance)
        ));
        return errors;
    }

    if let Some(Value::Array(branches)) = contract.get("allOf") {
        for (index, branch) in branches.iter().enumerate() {
            errors.extend(validate_at(
                instance,
                branch,
                schema_path,
                bundle,
                documents,
                &format!("{location}[allOf:{index}]"),
                failures,
                depth,
            ));
        }
    }
    if let Some(Value::Array(branches)) = contract.get("anyOf") {
        // Every branch is evaluated, not just up to the first match: the predecessor built the
        // whole list, and a branch's `$ref` load is what reports a broken schema file.
        let matched: Vec<bool> = branches
            .iter()
            .map(|branch| {
                validate_at(
                    instance,
                    branch,
                    schema_path,
                    bundle,
                    documents,
                    location,
                    failures,
                    depth,
                )
                .is_empty()
            })
            .collect();
        if !matched.contains(&true) {
            errors.push(format!("{location}: matches no anyOf branch"));
        }
    }
    if let Some(Value::Array(branches)) = contract.get("oneOf") {
        let matches = branches
            .iter()
            .filter(|branch| {
                validate_at(
                    instance,
                    branch,
                    schema_path,
                    bundle,
                    documents,
                    location,
                    failures,
                    depth,
                )
                .is_empty()
            })
            .count();
        if matches != 1 {
            errors.push(format!(
                "{location}: matches {matches} oneOf branches, expected exactly one"
            ));
        }
    }
    if let Some(forbidden) = contract.get("not")
        && matches!(forbidden, Value::Object(_) | Value::Bool(_))
        && validate_at(
            instance,
            forbidden,
            schema_path,
            bundle,
            documents,
            location,
            failures,
            depth,
        )
        .is_empty()
    {
        errors.push(format!("{location}: matches forbidden schema"));
    }

    if let Some(condition) = contract.get("if")
        && matches!(condition, Value::Object(_) | Value::Bool(_))
    {
        let matched = validate_at(
            instance,
            condition,
            schema_path,
            bundle,
            documents,
            location,
            failures,
            depth,
        )
        .is_empty();
        let selected = if matched {
            contract.get("then")
        } else {
            contract.get("else")
        };
        if let Some(selected) = selected
            && matches!(selected, Value::Object(_) | Value::Bool(_))
        {
            errors.extend(validate_at(
                instance,
                selected,
                schema_path,
                bundle,
                documents,
                location,
                failures,
                depth,
            ));
        }
    }

    if let Value::Object(object) = instance {
        if let Some(Value::Array(required)) = contract.get("required") {
            for key in required {
                let present = key.as_str().is_some_and(|key| object.contains_key(key));
                if !present {
                    errors.push(format!(
                        "{location}: missing required property {}",
                        python_repr(key)
                    ));
                }
            }
        }
        let properties = contract.get("properties").and_then(Value::as_object);
        if let Some(properties) = properties {
            for (key, child) in properties {
                if let Some(value) = object.get(key) {
                    errors.extend(validate_at(
                        value,
                        child,
                        schema_path,
                        bundle,
                        documents,
                        &format!("{location}/{key}"),
                        failures,
                        depth,
                    ));
                }
            }
        }
        let additional = contract.get("additionalProperties");
        for (key, value) in object {
            if properties.is_some_and(|properties| properties.contains_key(key)) {
                continue;
            }
            match additional {
                Some(Value::Bool(false)) => errors.push(format!(
                    "{location}: additional property {} is forbidden",
                    python_repr_str(key)
                )),
                Some(schema @ (Value::Object(_) | Value::Bool(true))) => {
                    errors.extend(validate_at(
                        value,
                        schema,
                        schema_path,
                        bundle,
                        documents,
                        &format!("{location}/{key}"),
                        failures,
                        depth,
                    ));
                }
                _ => {}
            }
        }
        if let Some(names) = contract.get("propertyNames")
            && matches!(names, Value::Object(_) | Value::Bool(_))
        {
            for key in object.keys() {
                errors.extend(validate_at(
                    &Value::String(key.clone()),
                    names,
                    schema_path,
                    bundle,
                    documents,
                    &format!("{location}/<property:{key}>"),
                    failures,
                    depth,
                ));
            }
        }
        let length = i64::try_from(object.len()).unwrap_or(i64::MAX);
        if let Some(minimum) = integer_keyword(contract, "minProperties")
            && length < minimum
        {
            errors.push(format!("{location}: fewer than {minimum} properties"));
        }
        if let Some(maximum) = integer_keyword(contract, "maxProperties")
            && length > maximum
        {
            errors.push(format!("{location}: more than {maximum} properties"));
        }
    }

    if let Value::Array(items) = instance {
        if let Some(schema) = contract.get("items")
            && matches!(schema, Value::Object(_) | Value::Bool(_))
        {
            for (index, item) in items.iter().enumerate() {
                errors.extend(validate_at(
                    item,
                    schema,
                    schema_path,
                    bundle,
                    documents,
                    &format!("{location}/{index}"),
                    failures,
                    depth,
                ));
            }
        }
        let length = i64::try_from(items.len()).unwrap_or(i64::MAX);
        if let Some(minimum) = integer_keyword(contract, "minItems")
            && length < minimum
        {
            errors.push(format!("{location}: fewer than {minimum} items"));
        }
        if let Some(maximum) = integer_keyword(contract, "maxItems")
            && length > maximum
        {
            errors.push(format!("{location}: more than {maximum} items"));
        }
        if contract.get("uniqueItems") == Some(&Value::Bool(true)) {
            for (index, item) in items.iter().enumerate() {
                if items[..index].iter().any(|prior| same_json(item, prior)) {
                    errors.push(format!("{location}/{index}: duplicate item"));
                }
            }
        }
    }

    if let Value::String(text) = instance {
        let length = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
        if let Some(minimum) = integer_keyword(contract, "minLength")
            && length < minimum
        {
            errors.push(format!("{location}: shorter than {minimum} characters"));
        }
        if let Some(maximum) = integer_keyword(contract, "maxLength")
            && length > maximum
        {
            errors.push(format!("{location}: longer than {maximum} characters"));
        }
        if let Some(Value::String(pattern)) = contract.get("pattern") {
            match matches_pattern(pattern, text) {
                Ok(true) => {}
                Ok(false) => errors.push(format!(
                    "{location}: does not match {}",
                    python_repr_str(pattern)
                )),
                Err(error) => errors.push(format!(
                    "{location}: invalid schema regex {}: {error}",
                    python_repr_str(pattern)
                )),
            }
        }
        if contract.get("format").and_then(Value::as_str) == Some("date-time")
            && !is_isoformat(&text.replace('Z', "+00:00"))
        {
            errors.push(format!("{location}: invalid date-time"));
        }
        if let Some(depth) = integer_keyword(contract, "x-b10x-max-depth")
            && i64::try_from(text.split('/').count()).unwrap_or(i64::MAX) > depth
        {
            errors.push(format!("{location}: path has more than {depth} components"));
        }
    }

    if let Value::Number(number) = instance {
        let value = number.as_f64().unwrap_or(f64::NAN);
        if let Some(Value::Number(minimum)) = contract.get("minimum")
            && value < minimum.as_f64().unwrap_or(f64::NAN)
        {
            errors.push(format!(
                "{location}: below minimum {}",
                python_number(minimum)
            ));
        }
        if let Some(Value::Number(maximum)) = contract.get("maximum")
            && value > maximum.as_f64().unwrap_or(f64::NAN)
        {
            errors.push(format!(
                "{location}: above maximum {}",
                python_number(maximum)
            ));
        }
    }

    errors
}

fn integer_keyword(contract: &Map<String, Value>, keyword: &str) -> Option<i64> {
    contract.get(keyword)?.as_i64()
}

thread_local! {
    /// Compiled once per distinct pattern, as CPython's `re` module caches its own.
    static PATTERNS: RefCell<BTreeMap<String, Result<Regex, String>>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// `re.search(pattern, text)` — unanchored, and with lookaround.
///
/// `fancy-regex` rather than `regex`: three bundle patterns use negative lookahead
/// (`^(?!/)(?!.*(?:^|/)\.\.(?:/|$)).+$`, the workspace path that may not escape), which `regex`
/// refuses to compile at all, and one of the three is reached on the released trees. It is already
/// in `Cargo.lock` beneath `jsonschema`, which depends on it for exactly this reason.
fn matches_pattern(pattern: &str, text: &str) -> Result<bool, String> {
    PATTERNS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let compiled = cache.entry(pattern.to_owned()).or_insert_with(|| {
            RegexBuilder::new(pattern)
                // `schemas/common.json`'s canonical base64 file says `[A-Za-z0-9+/]{1398102}==`
                // — a counted repetition over 1 MiB of encoded bytes, which is how the schema
                // states the *decoded* 1 MiB boundary that no length keyword can express. CPython
                // compiles that with a counted-repeat opcode and no size limit; `regex` expands it
                // into states and refuses at its default 10 MiB NFA budget, at which point
                // `fancy-regex` falls back to backtracking and exhausts that budget too. The
                // result would be an exactly-1-MiB file reported as *not matching* a pattern it
                // matches — the boundary silently inverted. 256 MiB is what it needs (measured:
                // 320 ms and 394 MiB peak, once, and only for a document that carries such a file;
                // no released bundle does).
                .delegate_size_limit(256 * 1024 * 1024)
                .delegate_dfa_size_limit(64 * 1024 * 1024)
                .backtrack_limit(1 << 27)
                .build()
                .map_err(|error| error.to_string())
        });
        match compiled {
            Ok(regex) => regex.is_match(text).map_err(|error| error.to_string()),
            Err(error) => Err(error.clone()),
        }
    })
}

/// Resolve a `$ref` against the bundle, refusing anything outside it.
fn resolve_ref(
    reference: &str,
    schema_path: &Path,
    bundle: &Path,
    documents: &Documents,
    failures: &mut Vec<String>,
) -> Result<(Value, PathBuf), String> {
    let (target_text, separator, fragment) = partition(reference, '#');
    let (document, target_path) = if target_text.is_empty() {
        let document = documents
            .load(schema_path, failures)
            .ok_or_else(|| format!("cannot load local reference {reference}"))?;
        (document, schema_path.to_path_buf())
    } else {
        let target_path = resolve_path(&parent_of(schema_path).join(target_text));
        if !target_path.starts_with(resolve_path(bundle)) {
            return Err(format!("reference escapes bundle: {reference}"));
        }
        let document = documents
            .load(&target_path, failures)
            .ok_or_else(|| format!("cannot load reference {reference}"))?;
        (document, target_path)
    };
    let pointed = pointer(
        &document,
        &if separator {
            format!("#{fragment}")
        } else {
            String::new()
        },
        "unsupported JSON pointer fragment",
    )?;
    Ok((pointed, target_path))
}

/// `str.partition` — the prefix, whether the separator was there, and the suffix.
fn partition(text: &str, separator: char) -> (&str, bool, &str) {
    match text.split_once(separator) {
        Some((head, tail)) => (head, true, tail),
        None => (text, false, ""),
    }
}

/// A JSON pointer fragment, resolved the way the predecessor resolved it — including its error
/// vocabulary, because those strings reach the gate's output.
fn pointer(document: &Value, fragment: &str, unsupported: &str) -> Result<Value, String> {
    if fragment.is_empty() || fragment == "#" {
        return Ok(document.clone());
    }
    let Some(path) = fragment.strip_prefix("#/") else {
        return Err(format!("{unsupported} {}", python_repr_str(fragment)));
    };
    let mut current = document;
    for raw in path.split('/') {
        let part = raw.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(object) => object.get(&part).ok_or_else(|| python_repr_str(&part))?,
            Value::Array(items) => {
                let index: i64 = part.parse().map_err(|_| {
                    format!(
                        "invalid literal for int() with base 10: {}",
                        python_repr_str(&part)
                    )
                })?;
                let length = i64::try_from(items.len()).unwrap_or(i64::MAX);
                let offset = if index < 0 { length + index } else { index };
                usize::try_from(offset)
                    .ok()
                    .and_then(|offset| items.get(offset))
                    .ok_or_else(|| "list index out of range".to_owned())?
            }
            _ => return Err(python_repr_str(&part)),
        };
    }
    Ok(current.clone())
}

// ---------------------------------------------------------------------------------------------
// The standards pass
// ---------------------------------------------------------------------------------------------

struct Resource {
    uri: String,
    schema: Value,
}

enum Record {
    Meta {
        label: String,
        schema: Value,
    },
    Instance {
        label: String,
        schema_uri: String,
        instance: Value,
    },
}

/// Inline every exact bundle-relative `$ref`, so a rootless URN `$id` cannot rebase it.
///
/// A bundle schema's `$id` is `urn:b10x:substrate-wire:<version>:<name>` — wire-visible, frozen and
/// *rootless*, so a relative `$ref` resolved against it would resolve against the URN rather than
/// against the file tree a reader walks. Inlining removes the question.
fn dereference_schema(
    schema: &Value,
    schema_path: &Path,
    bundle: &Path,
    documents: &Documents,
    stack: &[(PathBuf, String)],
    failures: &mut Vec<String>,
) -> Result<Value, String> {
    let object = match schema {
        Value::Array(items) => {
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(dereference_schema(
                    item,
                    schema_path,
                    bundle,
                    documents,
                    stack,
                    failures,
                )?);
            }
            return Ok(Value::Array(resolved));
        }
        Value::Object(object) => object,
        other => return Ok(other.clone()),
    };

    if let Some(Value::String(reference)) = object.get("$ref") {
        let (target_text, separator, fragment) = partition(reference, '#');
        let target_path = if target_text.is_empty() {
            resolve_path(schema_path)
        } else {
            resolve_path(&parent_of(schema_path).join(target_text))
        };
        if !target_path.starts_with(bundle) {
            return Err(format!(
                "{} is not in the subpath of {}",
                python_repr_str(&target_path.to_string_lossy()),
                python_repr_str(&bundle.to_string_lossy())
            ));
        }
        let identity = (target_path.clone(), fragment.to_owned());
        if stack.contains(&identity) {
            return Err(format!(
                "cyclic schema reference {}",
                python_repr_str(reference)
            ));
        }
        let Some(target_document) = documents.load(&target_path, failures) else {
            return Err(format!(
                "unavailable schema reference {}",
                python_repr_str(reference)
            ));
        };
        let target = pointer(
            &target_document,
            &if separator {
                format!("#{fragment}")
            } else {
                String::new()
            },
            "unsupported schema fragment",
        )?;
        let mut extended = stack.to_vec();
        extended.push(identity);
        let resolved = dereference_schema(
            &target,
            &target_path,
            bundle,
            documents,
            &extended,
            failures,
        )?;
        let mut siblings = Map::new();
        for (key, value) in object {
            if key == "$ref" {
                continue;
            }
            siblings.insert(
                key.clone(),
                dereference_schema(value, schema_path, bundle, documents, stack, failures)?,
            );
        }
        if siblings.is_empty() {
            return Ok(resolved);
        }
        let mut combined = Map::new();
        combined.insert("allOf".to_owned(), Value::Array(vec![resolved]));
        for (key, value) in siblings {
            combined.insert(key, value);
        }
        return Ok(Value::Object(combined));
    }

    let mut resolved = Map::new();
    for (key, value) in object {
        resolved.insert(
            key.clone(),
            dereference_schema(value, schema_path, bundle, documents, stack, failures)?,
        );
    }
    Ok(Value::Object(resolved))
}

/// Meta-validate every schema authority and validate every classified instance with the pinned
/// standards validator — the work `crates/substrate-contract-check` did over a pipe.
fn standards_errors(resources: &[Resource], records: &[Record]) -> Vec<String> {
    let mut registry = jsonschema::Registry::new();
    for resource in resources {
        match registry.add(resource.uri.clone(), resource.schema.clone()) {
            Ok(next) => registry = next,
            // Fails closed: without a complete registry nothing below can be validated, and a
            // partial pass would read as a pass.
            Err(error) => {
                return vec![format!(
                    "standards schema registry rejected {}: {error}",
                    resource.uri
                )];
            }
        }
    }
    let registry = match registry.prepare() {
        Ok(registry) => registry,
        Err(error) => {
            return vec![format!(
                "standards schema registry cannot be prepared: {error}"
            )];
        }
    };

    let mut failures = Vec::new();
    for record in records {
        match record {
            Record::Meta { label, schema } => {
                if schema.get("$schema").and_then(Value::as_str) != Some(DRAFT_2020_12) {
                    failures.push(format!(
                        "{label}: must declare the pinned Draft 2020-12 meta-schema"
                    ));
                    continue;
                }
                if let Err(error) = jsonschema::draft202012::meta::validate(schema) {
                    failures.push(format!("{label}: {error}"));
                }
            }
            Record::Instance {
                label,
                schema_uri,
                instance,
            } => {
                let reference = json!({ "$ref": schema_uri });
                match jsonschema::draft202012::options()
                    .with_registry(&registry)
                    .build(&reference)
                {
                    Ok(validator) => {
                        if let Err(error) = validator.validate(instance) {
                            failures.push(format!("{label}: {error}"));
                        }
                    }
                    Err(error) => {
                        failures.push(format!("{label}: schema compile failed: {error}"));
                    }
                }
            }
        }
    }
    failures
}

// ---------------------------------------------------------------------------------------------
// The five fixed root authorities of 0.1.0
// ---------------------------------------------------------------------------------------------

fn string_schema() -> Value {
    json!({"type": "string", "minLength": 1})
}

fn string_list() -> Value {
    json!({"type": "array", "items": string_schema()})
}

fn sha256_schema() -> Value {
    json!({"type": "string", "pattern": "^[0-9a-f]{64}$"})
}

/// `closed(properties, required)` — an object schema that admits nothing it did not name.
///
/// `required` defaults to the property names *in the order given*, which is what the predecessor's
/// `list(properties)` produced from an insertion-ordered dict.
fn closed(properties: &[(&str, Value)], required: Option<&[&str]>) -> Value {
    let mut map = Map::new();
    for (key, value) in properties {
        map.insert((*key).to_owned(), value.clone());
    }
    let required: Vec<Value> = match required {
        Some(names) => names.iter().map(|name| json!(name)).collect(),
        None => properties.iter().map(|(key, _)| json!(key)).collect(),
    };
    json!({
        "additionalProperties": false,
        "properties": map,
        "required": required,
        "type": "object",
    })
}

/// The exact schemas the gate classifies a bundle's five fixed root authorities with.
///
/// Only `0.1.0` reaches them: every later bundle declares a `$schema` on every JSON document it
/// holds, including its own manifest, so `check_bundle` builds this map for `0.1.0` alone. The
/// later versions' compatibility and errata blocks are kept because the predecessor carried them
/// and they say what those bundles' fixed authorities were required to contain.
#[allow(clippy::too_many_lines)] // Five schemas written out, so a reader can check them by eye.
pub fn fixed_authority_schemas(version: &str) -> BTreeMap<String, Value> {
    let file_entry = closed(
        &[
            ("byte_length", json!({"type": "integer", "minimum": 0})),
            (
                "media_type",
                json!({"enum": ["application/json", "text/markdown"]}),
            ),
            ("path", string_schema()),
            ("sha256", sha256_schema()),
        ],
        None,
    );
    let mut bundle_properties: Vec<(&str, Value)> = vec![
        ("api_version", json!({"const": "v1"})),
        ("bundle_format", json!({"const": "b10x.contract-bundle.v1"})),
        (
            "files",
            json!({"type": "array", "items": file_entry, "uniqueItems": true}),
        ),
        (
            "generator",
            closed(
                &[
                    ("digest", sha256_schema()),
                    ("name", string_schema()),
                    ("version", string_schema()),
                ],
                None,
            ),
        ),
        ("name", json!({"const": "substrate-wire"})),
        ("origin", json!({"const": "b10x"})),
        ("source_base_commit", json!({"type": ["null", "string"]})),
        ("status", json!({"const": "development"})),
        ("version", json!({"const": version})),
    ];
    if version == "0.2.0" {
        bundle_properties.push((
            "compatibility",
            closed(
                &[
                    ("adds_routes", json!({"const": 7})),
                    ("kind", json!({"const": "additive-v1"})),
                    ("predecessor", json!({"const": "0.1.0"})),
                    ("preserves_routes", json!({"const": 12})),
                ],
                None,
            ),
        ));
    } else if version == "0.3.0" || version == "0.4.0" {
        bundle_properties.push((
            "compatibility",
            closed(
                &[
                    ("adds_routes", json!({"const": 7})),
                    ("kind", json!({"const": "additive-v1"})),
                    ("predecessor", json!({"const": "0.2.0"})),
                    ("preserves_routes", json!({"const": 19})),
                ],
                None,
            ),
        ));
    }

    let mut compatibility_properties: Vec<(&str, Value)> = vec![
        ("contract", json!({"const": "substrate-wire"})),
        ("development_constraints", string_list()),
        ("request_policy", json!({"const": "closed"})),
        ("response_policy", string_schema()),
        ("status", json!({"const": "development"})),
        ("supported_api_majors", json!({"const": [1]})),
        ("version", json!({"const": version})),
    ];
    if version == "0.2.0" {
        let erratum = closed(
            &[
                ("compatibility_impact", string_schema()),
                ("corrected_expectation", string_schema()),
                ("erroneous_expectation", string_schema()),
                ("predecessor_path", string_schema()),
                ("predecessor_sha256", sha256_schema()),
                ("reason", string_schema()),
                ("replacement_path", string_schema()),
                ("replacement_sha256", sha256_schema()),
            ],
            None,
        );
        compatibility_properties.push((
            "errata_from",
            closed(
                &[
                    (
                        "records",
                        json!({"type": "array", "items": erratum, "minItems": 1}),
                    ),
                    ("version", json!({"const": "0.1.0"})),
                ],
                None,
            ),
        ));
    }

    let origin_input = closed(
        &[
            ("digest", json!({"type": ["null", "string"]})),
            ("name", string_schema()),
            ("origin", string_schema()),
            ("release_blocker", string_schema()),
            ("role", string_schema()),
            ("uri", string_schema()),
            ("version", string_schema()),
        ],
        Some(&["name", "origin", "role", "version"]),
    );
    let origins = closed(
        &[
            (
                "bundle",
                json!({"const": format!("substrate-wire@{version}")}),
            ),
            (
                "inputs",
                json!({"type": "array", "items": origin_input, "minItems": 1}),
            ),
            ("origin", json!({"const": "b10x"})),
        ],
        None,
    );
    let archive = closed(
        &[
            ("compression", string_schema()),
            ("format", string_schema()),
            ("gid", json!({"type": "integer", "minimum": 0})),
            ("group_name", json!({"type": "string"})),
            ("mode", string_schema()),
            ("owner_name", json!({"type": "string"})),
            ("path_order", string_schema()),
            ("source_date_epoch", string_schema()),
            ("uid", json!({"type": "integer", "minimum": 0})),
        ],
        None,
    );
    let packaging = closed(
        &[
            ("archive", archive),
            ("json_authority", string_schema()),
            ("release_blockers", string_list()),
            ("status", json!({"const": "development"})),
        ],
        None,
    );
    let normalization = closed(
        &[
            ("dot_segments", string_schema()),
            ("encoded_separator", string_schema()),
            ("path_parameters", string_schema()),
            ("percent_encoding", string_schema()),
            ("query", string_schema()),
            ("repeated_separator", string_schema()),
            ("trailing_separator", string_schema()),
        ],
        None,
    );
    let tuple_schema = closed(
        &[
            ("encoding", string_schema()),
            ("fields", string_list()),
            ("length_unit", string_schema()),
        ],
        None,
    );
    let hashing = closed(
        &[
            ("address_normalization", normalization),
            ("algorithm", json!({"const": "sha256"})),
            ("canonical_input", string_schema()),
            ("excluded", string_list()),
            ("fixtures", json!({"const": "fixtures/canonical-hash.json"})),
            ("format", json!({"const": "b10x.substrate-request-hash.v1"})),
            (
                "ledger_key",
                json!({"const": ["deployment", "subject", "operation"]}),
            ),
            ("tuple", tuple_schema),
        ],
        None,
    );

    let mut result = BTreeMap::new();
    result.insert("bundle.json".to_owned(), closed(&bundle_properties, None));
    result.insert(
        "compatibility.json".to_owned(),
        closed(&compatibility_properties, None),
    );
    result.insert("hashing.json".to_owned(), hashing);
    result.insert("origins.json".to_owned(), origins);
    result.insert("packaging.json".to_owned(), packaging);
    for schema in result.values_mut() {
        if let Some(object) = schema.as_object_mut() {
            object.insert("$schema".to_owned(), json!(DRAFT_2020_12));
        }
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Small equivalences the ported messages depend on
// ---------------------------------------------------------------------------------------------

/// `Path.resolve()`: the longest existing prefix canonicalised, the rest applied lexically.
fn resolve_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    if let Ok(canonical) = absolute.canonicalize() {
        return canonical;
    }
    let mut resolved = PathBuf::from("/");
    for component in absolute.components() {
        match component {
            Component::Normal(part) => {
                resolved.push(part);
                if let Ok(canonical) = resolved.canonicalize() {
                    resolved = canonical;
                }
            }
            Component::ParentDir => {
                resolved.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    resolved
}

/// `same_json` — JSON equality that does not conflate `1` with `1.0`, or `true` with `1`.
fn same_json(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => {
            left.is_f64() == right.is_f64() && left == right
        }
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| same_json(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, value)| right.get(key).is_some_and(|other| same_json(value, other)))
        }
        _ => false,
    }
}

fn instance_type(instance: &Value, expected: &str) -> bool {
    match expected {
        "null" => instance.is_null(),
        "boolean" => instance.is_boolean(),
        "integer" => matches!(instance, Value::Number(number) if !number.is_f64()),
        "number" => instance.is_number(),
        "string" => instance.is_string(),
        "array" => instance.is_array(),
        "object" => instance.is_object(),
        _ => false,
    }
}

fn python_type_name(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `str(value)` for a JSON number, which for a float is `repr` and therefore keeps its `.0`.
fn python_number(number: &Number) -> String {
    if number.is_f64() {
        let value = number.as_f64().unwrap_or(f64::NAN);
        let rendered = format!("{value:?}");
        return match rendered.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => rendered,
        };
    }
    number.to_string()
}

/// `repr(value)` for a JSON value, because the predecessor's messages interpolate `{value!r}`.
fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => python_number(number),
        Value::String(text) => python_repr_str(text),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr_str(key), python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `repr(text)`: single quotes, unless that would need escaping and double quotes would not.
fn python_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut rendered = String::with_capacity(text.len() + 2);
    rendered.push(quote);
    for character in text.chars() {
        match character {
            '\\' => rendered.push_str("\\\\"),
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            other if other == quote => {
                rendered.push('\\');
                rendered.push(other);
            }
            other if (other as u32) < 0x20 || (other as u32) == 0x7f => {
                let _ = write!(rendered, "\\x{:02x}", other as u32);
            }
            other => rendered.push(other),
        }
    }
    rendered.push(quote);
    rendered
}

/// `datetime.fromisoformat` for calendar dates and times, which is every shape a wire timestamp
/// takes. ISO week dates (`2026-W05-3`), which `CPython` 3.11+ also accepts, are not accepted here;
/// no document in any released bundle reaches this branch (measured), and `format: date-time` in
/// these schemas always sits beside an explicit `pattern` that already excludes them.
fn is_isoformat(text: &str) -> bool {
    let Some((index, _)) = text.char_indices().nth(10) else {
        return is_iso_date(text);
    };
    let (date, rest) = text.split_at(index);
    if !is_iso_date(date) {
        return false;
    }
    let time = &rest[rest.chars().next().map_or(0, char::len_utf8)..];
    if time.is_empty() {
        return false;
    }
    let (clock, offset) = match time.rfind(['+', '-']) {
        Some(index) => (&time[..index], Some(&time[index + 1..])),
        None => (time, None),
    };
    if let Some(offset) = offset
        && !is_iso_clock(offset)
    {
        return false;
    }
    is_iso_clock(clock)
}

fn is_iso_date(date: &str) -> bool {
    let digits: Vec<char> = date.chars().collect();
    let (year, month, day) = match digits.len() {
        10 if digits[4] == '-' && digits[7] == '-' => (&date[0..4], &date[5..7], &date[8..10]),
        8 => (&date[0..4], &date[4..6], &date[6..8]),
        _ => return false,
    };
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    if !(1..=12).contains(&month) {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let length = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    (1..=length).contains(&day)
}

fn is_iso_clock(clock: &str) -> bool {
    let (clock, fraction) = match clock.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (clock, None),
    };
    if fraction.is_some_and(|fraction| {
        fraction.is_empty() || !fraction.chars().all(|digit| digit.is_ascii_digit())
    }) {
        return false;
    }
    let parts: Vec<&str> = if clock.contains(':') {
        clock.split(':').collect()
    } else {
        match clock.len() {
            2 => vec![&clock[0..2]],
            4 => vec![&clock[0..2], &clock[2..4]],
            6 => vec![&clock[0..2], &clock[2..4], &clock[4..6]],
            _ => return false,
        }
    };
    if parts.is_empty() || parts.len() > 3 {
        return false;
    }
    for (index, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return false;
        }
        let Ok(value) = part.parse::<u32>() else {
            return false;
        };
        let ceiling = if index == 0 { 24 } else { 60 };
        if value >= ceiling {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        Documents, check_bundle, fixed_authority_schemas, python_repr, python_repr_str, validate,
    };
    use crate::repo;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// The repository this test binary belongs to, for the cases that read the real `0.2.0` tree.
    fn repository() -> PathBuf {
        repo::root().expect("repository root")
    }

    fn bundle_0_2_0() -> PathBuf {
        repository().join("contracts/substrate-wire/0.2.0")
    }

    /// `write_json` in `scripts/test_contract_json_gate.py:33` — deterministic source form.
    fn write_json(path: &Path, value: &Value) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let mut text = serde_json::to_string_pretty(value).expect("render");
        text.push('\n');
        fs::write(path, text).expect("write");
    }

    /// `ContractJsonGateTests.run_gate` — the failures one bundle directory produces.
    fn run_gate(root: &Path) -> Vec<String> {
        check_bundle(root, "0.2.0", &repository()).1
    }

    #[test]
    fn unclassified_json_is_rejected() {
        let directory = TempDir::new().expect("temporary directory");
        write_json(
            &directory.path().join("unexpected.json"),
            &json!({"value": "unclassified"}),
        );
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("unclassified JSON authority")),
            "{failures:?}"
        );
    }

    #[test]
    fn every_fixed_authority_is_rejected_by_its_bundled_exact_schema() {
        let bundle = bundle_0_2_0();
        let cases = [
            ("bundle.json", "name"),
            ("compatibility.json", "errata_from"),
            ("coverage.json", "requirements"),
            ("hashing.json", "canonical_query"),
            ("operations.json", "operations"),
            ("origins.json", "inputs"),
            ("packaging.json", "archive"),
            ("runner.json", "protocol"),
        ];
        for (relative, missing) in cases {
            let directory = TempDir::new().expect("temporary directory");
            let root = directory.path();
            let source = bundle.join(relative);
            let mut document: Value =
                serde_json::from_str(&fs::read_to_string(&source).expect("read")).expect("parse");
            let declaration = document["$schema"]
                .as_str()
                .expect("declaration")
                .to_owned();
            document
                .as_object_mut()
                .expect("object")
                .remove(missing)
                .expect("property present");
            write_json(&root.join(relative), &document);
            let schema_target = root
                .join(relative)
                .parent()
                .expect("parent")
                .join(&declaration);
            let source_schema = source.parent().expect("parent").join(&declaration);
            let schema: Value =
                serde_json::from_str(&fs::read_to_string(&source_schema).expect("read schema"))
                    .expect("parse schema");
            write_json(&schema_target, &schema);
            let failures = run_gate(root);
            assert!(
                failures
                    .iter()
                    .any(|item| item.contains(&format!("missing required property '{missing}'"))),
                "{relative}: {failures:?}"
            );
        }
    }

    #[test]
    fn invalid_declared_payload_is_rejected() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path();
        write_json(
            &root.join("schemas/example.json"),
            &json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "additionalProperties": false,
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "type": "object",
            }),
        );
        write_json(
            &root.join("payload.json"),
            &json!({"$schema": "schemas/example.json", "value": 7}),
        );
        let failures = run_gate(root);
        assert!(
            failures
                .iter()
                .any(|item| item.contains("expected type string")),
            "{failures:?}"
        );
    }

    #[test]
    fn schema_invalid_under_declared_meta_schema_is_rejected() {
        let directory = TempDir::new().expect("temporary directory");
        write_json(
            &directory.path().join("schemas/invalid.json"),
            &json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": 7,
            }),
        );
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("not valid under any of the schemas")),
            "{failures:?}"
        );
    }

    #[test]
    fn payload_schema_target_must_itself_be_a_schema_authority() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path();
        write_json(
            &root.join("schemas/not-a-schema.json"),
            &json!({"value": "data"}),
        );
        write_json(
            &root.join("payload.json"),
            &json!({"$schema": "schemas/not-a-schema.json", "value": "test"}),
        );
        let failures = run_gate(root);
        assert!(
            failures.iter().any(
                |item| item.contains("declared target is not a Draft 2020-12 schema authority")
            ),
            "{failures:?}"
        );
    }

    /// base64 of exactly 1 MiB and of 1 MiB + 1 byte have the *same encoded length*; only the
    /// padding tells them apart, so the schema has to say so and the validator has to see it.
    #[test]
    fn file_base64_schema_enforces_decoded_one_mib_boundary() {
        let bundle = bundle_0_2_0();
        let common_path = bundle.join("schemas/common.json");
        let mut failures = Vec::new();
        let documents = Documents::new(&repository());
        let common = documents
            .load(&common_path, &mut failures)
            .expect("common.json");
        let definition = common["$defs"]["canonical-base64-file"].clone();

        let exact = base64(&vec![0u8; 1_048_576]);
        let oversized = base64(&vec![0u8; 1_048_577]);
        assert_eq!(exact.len(), oversized.len());

        assert_eq!(
            validate(
                &Value::String(exact),
                &definition,
                &common_path,
                &bundle,
                &documents,
                "$",
                &mut failures,
            ),
            Vec::<String>::new()
        );
        assert!(
            !validate(
                &Value::String(oversized),
                &definition,
                &common_path,
                &bundle,
                &documents,
                "$",
                &mut failures,
            )
            .is_empty()
        );
    }

    #[test]
    fn event_stream_last_cursor_is_canonical_bounded_or_null() {
        let bundle = bundle_0_2_0();
        let schema_path = bundle.join("schemas/event-stream-frame.json");
        let mut failures = Vec::new();
        let documents = Documents::new(&repository());
        let frame = documents
            .load(&schema_path, &mut failures)
            .expect("frame schema");

        let frame_with = |cursor: Value| {
            json!({
                "code": "event.stream-backpressure",
                "kind": "backpressure",
                "last_cursor": cursor,
                "recovery": "pull",
            })
        };
        for cursor in [
            Value::Null,
            json!("ev2.scope_subject_01.41.0"),
            json!("ev2.scope_subject_01.41.7"),
        ] {
            assert_eq!(
                validate(
                    &frame_with(cursor.clone()),
                    &frame,
                    &schema_path,
                    &bundle,
                    &documents,
                    "$",
                    &mut failures,
                ),
                Vec::<String>::new(),
                "{cursor}"
            );
        }
        for cursor in [
            json!("not-an-event-cursor"),
            json!("ev2.scope_subject_01.41.01"),
            Value::String(format!("ev2.scope_{}.41.7", "a".repeat(500))),
        ] {
            assert!(
                !validate(
                    &frame_with(cursor.clone()),
                    &frame,
                    &schema_path,
                    &bundle,
                    &documents,
                    "$",
                    &mut failures,
                )
                .is_empty(),
                "{cursor}"
            );
        }
    }

    #[test]
    fn every_released_bundle_classifies_every_document_it_holds() {
        let repository = repository();
        let contracts = repository.join("contracts/substrate-wire");
        for (version, expected) in [
            ("0.1.0", 114),
            ("0.2.0", 165),
            ("0.3.0", 194),
            ("0.4.0", 200),
            ("0.5.0", 206),
            ("0.6.0", 213),
        ] {
            let (count, failures) = check_bundle(&contracts.join(version), version, &repository);
            assert_eq!(failures, Vec::<String>::new(), "{version}");
            assert_eq!(count, expected, "{version}");
        }
    }

    #[test]
    fn a_duplicate_object_key_is_a_refusal_and_not_a_merge() {
        let directory = TempDir::new().expect("temporary directory");
        fs::write(
            directory.path().join("duplicate.json"),
            "{\n  \"a\": 1,\n  \"a\": 2\n}\n",
        )
        .expect("write");
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("duplicate object key 'a'")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_document_that_is_not_in_deterministic_source_form_is_a_refusal() {
        let directory = TempDir::new().expect("temporary directory");
        fs::write(directory.path().join("compact.json"), "{\"b\":1,\"a\":2}").expect("write");
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("JSON is not in deterministic source form")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_schema_outside_the_bundle_is_a_refusal() {
        let directory = TempDir::new().expect("temporary directory");
        write_json(
            &directory.path().join("payload.json"),
            &json!({"$schema": "../elsewhere.json"}),
        );
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("declared schema escapes bundle")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_schema_beside_the_document_rather_than_under_schemas_is_a_refusal() {
        let directory = TempDir::new().expect("temporary directory");
        write_json(
            &directory.path().join("beside.json"),
            &json!({"$schema": "https://json-schema.org/draft/2020-12/schema"}),
        );
        write_json(
            &directory.path().join("payload.json"),
            &json!({"$schema": "beside.json"}),
        );
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("declared schema is not under schemas/")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_missing_declared_schema_is_a_refusal() {
        let directory = TempDir::new().expect("temporary directory");
        write_json(
            &directory.path().join("payload.json"),
            &json!({"$schema": "schemas/absent.json"}),
        );
        let failures = run_gate(directory.path());
        assert!(
            failures
                .iter()
                .any(|item| item.contains("declared schema is unavailable")),
            "{failures:?}"
        );
    }

    /// The predecessor followed `$ref` with no cycle check and no depth limit on this path, so a
    /// schema that referred to itself exhausted `CPython`'s stack and printed a thousand frames
    /// where a refusal belonged.
    #[test]
    fn a_cyclic_schema_reference_is_a_named_refusal_and_not_a_crash() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path();
        write_json(
            &root.join("schemas/loop.json"),
            &json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$defs": {
                    "a": {"$ref": "loop.json#/$defs/b"},
                    "b": {"$ref": "loop.json#/$defs/a"},
                },
                "$ref": "loop.json#/$defs/a",
            }),
        );
        write_json(
            &root.join("looped.json"),
            &json!({"$schema": "schemas/loop.json"}),
        );
        let failures = run_gate(root);
        assert!(
            failures
                .iter()
                .any(|item| item.contains("schema references nest more than 64 deep")),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|item| item.contains("cyclic schema reference")),
            "{failures:?}"
        );
    }

    /// `rglob("*.json")` matched entries, not files. A directory named like a document is read as
    /// one and refused, so it cannot become a place JSON lives unclassified.
    #[test]
    fn a_directory_named_like_a_document_is_read_as_one_and_refused() {
        let directory = TempDir::new().expect("temporary directory");
        let root = directory.path();
        write_json(
            &root.join("trap.json/inner.json"),
            &json!({"value": "inside a directory named like a document"}),
        );
        let failures = run_gate(root);
        assert!(
            failures
                .iter()
                .any(|item| item.contains("trap.json: invalid JSON: [Errno 21] Is a directory")),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|item| item.starts_with("trap.json/inner.json: unclassified JSON authority")),
            "{failures:?}"
        );
    }

    /// `x-b10x-max-depth` is not a JSON Schema keyword, so a conforming standards validator has to
    /// ignore it. This is why the subset validator was ported rather than dropped.
    #[test]
    fn the_bundles_own_depth_keyword_is_enforced() {
        let bundle = TempDir::new().expect("temporary directory");
        let documents = Documents::new(bundle.path());
        let mut failures = Vec::new();
        let contract = json!({"type": "string", "x-b10x-max-depth": 3});
        let schema_path = bundle.path().join("schemas/x.json");
        assert_eq!(
            validate(
                &json!("a/b/c"),
                &contract,
                &schema_path,
                bundle.path(),
                &documents,
                "$",
                &mut failures,
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            validate(
                &json!("a/b/c/d"),
                &contract,
                &schema_path,
                bundle.path(),
                &documents,
                "$",
                &mut failures,
            ),
            vec!["$: path has more than 3 components".to_owned()]
        );
    }

    /// The predecessor built these with `list(properties)` for `required`, which is insertion
    /// order; the later versions append their compatibility block *after* `version`.
    #[test]
    fn the_fixed_authorities_keep_their_predecessors_shape() {
        let base = fixed_authority_schemas("0.1.0");
        assert_eq!(
            base.keys().collect::<Vec<_>>(),
            vec![
                "bundle.json",
                "compatibility.json",
                "hashing.json",
                "origins.json",
                "packaging.json"
            ]
        );
        assert_eq!(
            base["bundle.json"]["$schema"],
            json!("https://json-schema.org/draft/2020-12/schema")
        );
        assert!(
            base["bundle.json"]["properties"]
                .get("compatibility")
                .is_none()
        );
        assert_eq!(
            base["origins.json"]["properties"]["bundle"]["const"],
            json!("substrate-wire@0.1.0")
        );

        let successor = fixed_authority_schemas("0.2.0");
        assert_eq!(
            successor["bundle.json"]["required"]
                .as_array()
                .expect("required")
                .last()
                .expect("last"),
            &json!("compatibility")
        );
        assert_eq!(
            successor["compatibility.json"]["required"]
                .as_array()
                .expect("required")
                .last()
                .expect("last"),
            &json!("errata_from")
        );
        assert_eq!(
            fixed_authority_schemas("0.4.0")["bundle.json"]["properties"]["compatibility"]["properties"]
                ["predecessor"]["const"],
            json!("0.2.0")
        );
    }

    #[test]
    fn python_reprs_are_the_ones_the_messages_interpolated() {
        assert_eq!(python_repr(&json!("v1")), "'v1'");
        assert_eq!(python_repr(&Value::Null), "None");
        assert_eq!(python_repr(&json!(true)), "True");
        assert_eq!(python_repr(&json!(7)), "7");
        assert_eq!(python_repr(&json!(7.5)), "7.5");
        assert_eq!(python_repr(&json!([1, "a"])), "[1, 'a']");
        assert_eq!(python_repr(&json!({"a": 1})), "{'a': 1}");
        assert_eq!(python_repr_str("it's"), "\"it's\"");
        assert_eq!(python_repr_str("a\"b"), "'a\"b'");
        assert_eq!(python_repr_str("a\nb"), "'a\\nb'");
    }

    /// Standard base64 with padding — enough for the one boundary case above, and no dependency.
    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let block = u32::from(chunk[0]) << 16
                | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
                | u32::from(chunk.get(2).copied().unwrap_or(0));
            for index in 0..4 {
                if index <= chunk.len() {
                    let shift = 18 - index * 6;
                    out.push(char::from(ALPHABET[((block >> shift) & 0x3F) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
