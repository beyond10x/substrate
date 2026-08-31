//! Version-2 contract-bundle renderer.
//!
//! Released bundles hash their renderer, so the v1 renderer in `render.rs` is immutable. This
//! successor keeps the established fixed-point algorithm while adding multi-major operation
//! entries and Axum catch-all parameter normalization.
//!
//! The bundle has two kinds of content and the split is the whole design. **Derived** content is
//! recomputed here on every run: the normalized route address schemas, the operation registry, the
//! route-selected request/response/operation unions, the conformance coverage inventory, the
//! canonical request-hash fixtures (computed through `substrate_wire`'s own hashing functions), and
//! the bundle manifest. **Authored** content — prose titles and descriptions, every request,
//! result, resource and envelope schema, and every conformance vector — carries semantics no Rust
//! type holds, so it lives as JSON under `xtask/bundle-source/<version>/documents/`, one file per
//! emitted path.
//!
//! **The schemas are not generated from the Rust types, and cannot be.** `xtask` has no reflection
//! over another crate: `schemars` is not a workspace dependency and adding the derives would be a
//! change to `crates/`. Even with it, `substrate-wire` describes a *later* wire than any frozen
//! bundle — `ExecStartInput::read_only_roots` (`crates/substrate-wire/src/lib.rs:761`) appears
//! nowhere in `contracts/substrate-wire/0.4.0/` — and the constraints the schemas carry are not on
//! the types at all: `argv` is a `Vec<String>` while the schema pins `minLength`/`maxLength`/
//! `maxItems`, whose values live in `crates/substrate-daemon/src/app/operations.rs:245-248`. What
//! the wire crate *does* own is the canonical hashing and the bounds constants, and those are what
//! this renderer takes from it.
//!
//! Authored documents may splice derived content with a single-member marker object:
//! `{"$routes": …}`, `{"$vectors": …}`, `{"$coverage": …}`, `{"$hash": …}`, `{"$files": …}`,
//! `{"$generator": …}`, `{"$compat": …}` and `{"$wire": "CONSTANT"}`. The last binds a schema bound
//! to a `substrate_wire` constant, so changing the constant changes the rendered schema and the
//! `0.4.0` fixed point fails loudly instead of drifting.
//!
//! One derivation the Python had and this renderer does not: the three `oneOf` unions in
//! `schemas/vector.json`. The predecessor built them from its in-memory vector dicts, so their
//! `required` arrays carry Python insertion order — `exec.output-limit-bytes` before
//! `exec.max-current` in `properties/setup/items/oneOf/0`. The emitted bundle is sorted-key JSON
//! and records that order nowhere, so recomputing the unions from the bundle's own vectors would
//! change bytes in a frozen directory. `schemas/vector.json` is therefore authored, and a successor
//! that wants it derived has to fix the union member order first.
//!
//! Why the source tree is not under `contracts/`: every directory there is a released bundle, frozen
//! by AGENTS.md invariant 6, and every JSON there is classified by the contract JSON gate, which
//! fails closed on an unclassified file (invariant 7). Authored render input is neither released nor
//! a contract document, so it lives beside the renderer that consumes it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::repo;

const SCHEMA_URI: &str = "https://json-schema.org/draft/2020-12/schema";

pub use crate::render::Args;

/// Renders the bundle and reports the manifest digest of what it wrote.
pub fn run(args: &Args) -> Result<ExitCode> {
    let root = repo::root()?;
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| root.join("target/contract-bundle").join(&args.version));
    refuse_released_tree(&root, &out)?;
    let source = args
        .source
        .clone()
        .unwrap_or_else(|| root.join("xtask/bundle-source"));
    let contracts = args
        .contracts_root
        .clone()
        .unwrap_or_else(|| root.join("contracts/substrate-wire"));
    let rendered = render(&Inputs {
        version: args.version.clone(),
        source_root: source,
        contracts_root: contracts,
        repository_root: root,
        wire: wire_constants(),
    })?;
    write_tree(&out, &rendered, args.force)?;
    println!(
        "render-bundle: {} files, manifest sha256:{}",
        rendered.len(),
        hex::encode(Sha256::digest(
            rendered
                .get("bundle.json")
                .ok_or_else(|| anyhow!("rendered tree has no bundle.json"))?
        ))
    );
    Ok(ExitCode::SUCCESS)
}

/// Everything the renderer reads.
pub struct Inputs {
    pub version: String,
    pub source_root: PathBuf,
    pub contracts_root: PathBuf,
    pub repository_root: PathBuf,
    /// The `substrate_wire` constants a `{"$wire": …}` marker may bind to. Production renders pass
    /// [`wire_constants`]; a test passes a perturbed table to prove the binding is live.
    pub wire: BTreeMap<String, Value>,
}

/// The `substrate_wire` constants a schema may bind a bound to.
///
/// Every entry is a value the wire crate owns, so a change there changes the rendered schema and
/// the `0.4.0` fixed point fails instead of drifting silently. Nothing here is a literal repeated
/// from the bundle: the right-hand side is the constant itself.
pub fn wire_constants() -> BTreeMap<String, Value> {
    use substrate_wire as wire;
    [
        ("API_VERSION", json!(wire::API_VERSION)),
        (
            "EXECUTION_CAPSULE_HASH_DOMAIN",
            json!(wire::EXECUTION_CAPSULE_HASH_DOMAIN),
        ),
        (
            "EXECUTION_CAPSULE_MOUNT",
            json!(wire::EXECUTION_CAPSULE_MOUNT),
        ),
        (
            "LEASE_CLOCK_TOLERANCE_MS",
            json!(wire::LEASE_CLOCK_TOLERANCE_MS),
        ),
        ("MAX_CURRENT_EXECS", json!(wire::MAX_CURRENT_EXECS)),
        (
            "MAX_CURRENT_WORKSPACES",
            json!(wire::MAX_CURRENT_WORKSPACES),
        ),
        ("MAX_EVENT_PAGE_ITEMS", json!(wire::MAX_EVENT_PAGE_ITEMS)),
        (
            "MAX_EXECUTION_CAPSULE_BYTES",
            json!(wire::MAX_EXECUTION_CAPSULE_BYTES),
        ),
        (
            "MAX_EXECUTION_CAPSULE_FILES",
            json!(wire::MAX_EXECUTION_CAPSULE_FILES),
        ),
        (
            "MAX_EXECUTION_CAPSULE_FILE_BYTES",
            json!(wire::MAX_EXECUTION_CAPSULE_FILE_BYTES),
        ),
        ("MAX_FILE_BYTES", json!(wire::MAX_FILE_BYTES)),
        ("MAX_IO_BYTES", json!(wire::MAX_IO_BYTES)),
        ("MAX_LEASE_TTL_MS", json!(wire::MAX_LEASE_TTL_MS)),
        ("MAX_LIST_ITEMS", json!(wire::MAX_LIST_ITEMS)),
        ("MAX_PATH_DEPTH", json!(wire::MAX_PATH_DEPTH)),
        ("MAX_READ_ONLY_ROOTS", json!(wire::MAX_READ_ONLY_ROOTS)),
        (
            "MAX_SNAPSHOT_PAGE_ITEMS",
            json!(wire::MAX_SNAPSHOT_PAGE_ITEMS),
        ),
        (
            "MAX_SNAPSHOT_PROVENANCE_EVENTS",
            json!(wire::MAX_SNAPSHOT_PROVENANCE_EVENTS),
        ),
        ("MAX_SECRET_SLOTS", json!(wire::MAX_SECRET_SLOTS)),
        ("MAX_SECRET_SLOT_BYTES", json!(wire::MAX_SECRET_SLOT_BYTES)),
        ("MAX_SECRET_SLOT_FD", json!(wire::MAX_SECRET_SLOT_FD)),
        ("MIN_SECRET_SLOT_FD", json!(wire::MIN_SECRET_SLOT_FD)),
        ("SECRET_SLOTS_ENV", json!(wire::SECRET_SLOTS_ENV)),
        ("MIN_LEASE_TTL_MS", json!(wire::MIN_LEASE_TTL_MS)),
        (
            "OPERATION_LEDGER_GLOBAL_MAX_BYTES",
            json!(wire::OPERATION_LEDGER_GLOBAL_MAX_BYTES),
        ),
        (
            "OPERATION_LEDGER_GLOBAL_MAX_ROWS",
            json!(wire::OPERATION_LEDGER_GLOBAL_MAX_ROWS),
        ),
        (
            "OPERATION_LEDGER_SUBJECT_MAX_BYTES",
            json!(wire::OPERATION_LEDGER_SUBJECT_MAX_BYTES),
        ),
        (
            "OPERATION_LEDGER_SUBJECT_MAX_ROWS",
            json!(wire::OPERATION_LEDGER_SUBJECT_MAX_ROWS),
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect()
}

/// A rendered bundle: emitted path to exact bytes.
pub type Rendered = BTreeMap<String, Vec<u8>>;

/// Renders the whole tree in memory. Nothing here reads a clock or the environment.
pub fn render(inputs: &Inputs) -> Result<Rendered> {
    let source = Source::load(inputs)?;
    let mut rendered = Rendered::new();

    let wire_only: BTreeMap<String, Value> = source
        .wire
        .iter()
        .map(|(name, value)| (format!("$wire/{name}"), value.clone()))
        .collect();
    for (path, value) in &source.documents {
        if path == "bundle.json" || !path.starts_with("vectors/") {
            continue;
        }
        let substituted =
            substitute(value, &wire_only).with_context(|| format!("rendering documents/{path}"))?;
        rendered.insert(path.clone(), canonical_json(&substituted));
    }
    for (path, value) in source.address_schemas()? {
        rendered.insert(path, canonical_json(&value));
    }

    let derived = source.derive()?;
    for (path, value) in &source.documents {
        if path == "bundle.json" || path.starts_with("vectors/") {
            continue;
        }
        let substituted =
            substitute(value, &derived).with_context(|| format!("rendering documents/{path}"))?;
        rendered.insert(path.clone(), canonical_json(&substituted));
    }

    let manifest_files = manifest_files(&rendered);
    let mut derived = derived;
    derived.insert("$files/manifest".to_owned(), Value::Array(manifest_files));
    let bundle = source
        .documents
        .get("bundle.json")
        .ok_or_else(|| anyhow!("documents/bundle.json is absent"))?;
    let bundle = substitute(bundle, &derived).context("rendering documents/bundle.json")?;
    rendered.insert("bundle.json".to_owned(), canonical_json(&bundle));
    Ok(rendered)
}

/// The authored source tree for one version.
struct Source {
    version: String,
    routes: Vec<Value>,
    coverage: Value,
    hash_cases: Vec<Value>,
    vector_order: Vec<String>,
    executable_vectors: BTreeSet<String>,
    documents: BTreeMap<String, Value>,
    predecessor_routes: BTreeSet<String>,
    generator_digest: String,
    wire: BTreeMap<String, Value>,
}

impl Source {
    fn load(inputs: &Inputs) -> Result<Self> {
        let dir = inputs.source_root.join(&inputs.version);
        if !dir.is_dir() {
            bail!(
                "no authored source for {} at {}",
                inputs.version,
                dir.display()
            );
        }
        let routes = read_array(&dir.join("routes.json"))?;
        let coverage = read_json(&dir.join("coverage.json"))?;
        let hash_cases = read_array(&dir.join("hash-cases.json"))?;
        let vector_order = read_string_array(&dir.join("vector-order.json"))?;
        let executable_vectors = read_string_array(&dir.join("executable-vectors.json"))?
            .into_iter()
            .collect();
        let documents = read_documents(&dir.join("documents"))?;

        let bundle = documents
            .get("bundle.json")
            .ok_or_else(|| anyhow!("documents/bundle.json is absent"))?;
        let predecessor = bundle
            .pointer("/compatibility/predecessor")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow!("documents/bundle.json declares no compatibility.predecessor")
            })?;
        let predecessor_routes = route_ids(
            &inputs
                .contracts_root
                .join(predecessor)
                .join("operations.json"),
        )?;
        let generator = bundle
            .pointer("/generator/name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("documents/bundle.json declares no generator.name"))?;
        let generator_digest = digest_of(&inputs.repository_root.join(generator))?;

        let source = Self {
            version: inputs.version.clone(),
            routes,
            coverage,
            hash_cases,
            vector_order,
            executable_vectors,
            documents,
            predecessor_routes,
            generator_digest,
            wire: inputs.wire.clone(),
        };
        source.check_vector_order()?;
        Ok(source)
    }

    fn check_vector_order(&self) -> Result<()> {
        let listed: BTreeSet<&String> = self.vector_order.iter().collect();
        let present: BTreeSet<&String> = self
            .documents
            .keys()
            .filter(|path| path.starts_with("vectors/"))
            .collect();
        if listed != present {
            let missing: Vec<&&String> = present.difference(&listed).collect();
            let extra: Vec<&&String> = listed.difference(&present).collect();
            bail!(
                "vector-order.json disagrees with documents/vectors: missing {missing:?}, extra {extra:?}"
            );
        }
        Ok(())
    }

    fn vectors(&self) -> Vec<&Value> {
        self.vector_order
            .iter()
            .filter_map(|path| self.documents.get(path))
            .collect()
    }

    /// The 26 normalized route address schemas, generated from each route's path template.
    fn address_schemas(&self) -> Result<BTreeMap<String, Value>> {
        let mut schemas = BTreeMap::new();
        for route in &self.routes {
            let path = string_at(route, "path")?;
            let target = string_at(route, "address_schema")?;
            let slug = target
                .strip_prefix("schemas/addresses/")
                .and_then(|rest| rest.strip_suffix(".json"))
                .ok_or_else(|| anyhow!("{target} is not an address schema path"))?;
            let mut properties = Map::new();
            let mut required = Vec::new();
            for parameter in path_parameters(path) {
                let definition = if parameter == "path" {
                    "relative-path".to_owned()
                } else {
                    parameter.replace('_', "-")
                };
                properties.insert(
                    parameter.clone(),
                    json!({ "$ref": format!("../common.json#/$defs/{definition}") }),
                );
                required.push(Value::String(parameter));
            }
            schemas.insert(
                target.to_owned(),
                json!({
                    "$id": format!("urn:b10x:substrate-wire:{}:address:{slug}", self.version),
                    "$schema": SCHEMA_URI,
                    "additionalProperties": false,
                    "properties": Value::Object(properties),
                    "required": Value::Array(required),
                    "title": format!("{slug} normalized route address"),
                    "type": "object",
                }),
            );
        }
        Ok(schemas)
    }

    /// Every marker expansion except the bundle manifest, which needs the rendered bytes.
    fn derive(&self) -> Result<BTreeMap<String, Value>> {
        let mut derived = BTreeMap::new();
        derived.insert("$routes/registry".to_owned(), self.registry()?);
        derived.insert(
            "$routes/count".to_owned(),
            Value::from(self.routes.len() as u64),
        );
        derived.insert(
            "$routes/keyed-operation-kinds".to_owned(),
            json!({ "enum": self.keyed_route_ids()? }),
        );
        derived.insert(
            "$routes/keyed-result-refs".to_owned(),
            self.keyed_result_refs()?,
        );
        derived.insert(
            "$routes/keyed-result-branches".to_owned(),
            self.keyed_result_branches()?,
        );
        derived.insert(
            "$routes/keyed-request-branches".to_owned(),
            self.keyed_request_branches()?,
        );
        derived.insert(
            "$routes/response-branches".to_owned(),
            self.response_branches()?,
        );
        derived.insert("$vectors/conformance".to_owned(), self.conformance());
        derived.insert("$coverage/requirements".to_owned(), self.requirements()?);
        derived.insert("$hash/cases".to_owned(), self.hash_fixtures()?);
        derived.insert(
            "$generator/digest".to_owned(),
            Value::String(self.generator_digest.clone()),
        );
        for (name, value) in &self.wire {
            derived.insert(format!("$wire/{name}"), value.clone());
        }
        let (preserves, adds) = self.compatibility()?;
        derived.insert(
            "$compat/preserves_routes".to_owned(),
            Value::from(preserves),
        );
        derived.insert("$compat/adds_routes".to_owned(), Value::from(adds));
        Ok(derived)
    }

    fn registry(&self) -> Result<Value> {
        let mut operations = Vec::with_capacity(self.routes.len());
        for route in &self.routes {
            let mut entry = route
                .as_object()
                .ok_or_else(|| anyhow!("routes.json entry is not an object"))?
                .clone();
            let path = string_at(route, "path")?;
            let major = path
                .strip_prefix("/v")
                .and_then(|rest| rest.split('/').next())
                .and_then(|major| major.parse::<u64>().ok())
                .ok_or_else(|| anyhow!("route {path} has no /v<major>/ prefix"))?;
            entry.insert("api_major".to_owned(), Value::from(major));
            entry.insert("direction".to_owned(), Value::String("outbound".to_owned()));
            operations.push(Value::Object(entry));
        }
        Ok(Value::Array(operations))
    }

    fn keyed(&self) -> impl Iterator<Item = &Value> {
        self.routes
            .iter()
            .filter(|route| route.get("idempotency").and_then(Value::as_str) == Some("keyed"))
    }

    fn keyed_route_ids(&self) -> Result<Value> {
        let mut ids = Vec::new();
        for route in self.keyed() {
            ids.push(Value::String(string_at(route, "id")?.to_owned()));
        }
        Ok(Value::Array(ids))
    }

    fn keyed_result_refs(&self) -> Result<Value> {
        let mut refs = Vec::new();
        for route in self.keyed() {
            refs.push(json!({ "$ref": result_ref(route)? }));
        }
        Ok(Value::Array(refs))
    }

    fn keyed_result_branches(&self) -> Result<Value> {
        let mut branches = Vec::new();
        for route in self.keyed() {
            branches.push(json!({
                "if": {
                    "properties": {
                        "operation_kind": { "const": string_at(route, "id")? },
                        "outcome": {
                            "properties": { "kind": { "const": "success" } },
                            "required": ["kind"],
                        },
                    },
                    "required": ["operation_kind", "outcome"],
                },
                "then": {
                    "properties": {
                        "outcome": {
                            "properties": { "result": { "$ref": result_ref(route)? } },
                            "required": ["result"],
                        },
                    },
                },
            }));
        }
        Ok(Value::Array(branches))
    }

    fn keyed_request_branches(&self) -> Result<Value> {
        let mut branches = Vec::new();
        for route in self.keyed() {
            branches.push(json!({
                "additionalProperties": false,
                "properties": {
                    "input": { "$ref": input_ref(route)? },
                    "op": { "$ref": "common.json#/$defs/operation-id" },
                },
                "required": ["op", "input"],
                "type": "object",
            }));
        }
        Ok(Value::Array(branches))
    }

    fn response_branches(&self) -> Result<Value> {
        let mut branches = Vec::new();
        for route in &self.routes {
            let path = string_at(route, "path")?;
            let api_version = path
                .strip_prefix('/')
                .and_then(|rest| rest.split('/').next())
                .ok_or_else(|| anyhow!("route {path} has no API version segment"))?;
            let mut required = vec![
                Value::String("api_version".to_owned()),
                Value::String("request_id".to_owned()),
                Value::String("result".to_owned()),
            ];
            if route.get("idempotency").and_then(Value::as_str) == Some("keyed") {
                required.push(Value::String("operation".to_owned()));
            }
            branches.push(json!({
                "additionalProperties": false,
                "properties": {
                    "api_version": { "const": api_version },
                    "operation": { "$ref": "common.json#/$defs/operation-id" },
                    "request_id": { "$ref": "common.json#/$defs/request-id" },
                    "result": { "$ref": result_ref(route)? },
                },
                "required": Value::Array(required),
                "type": "object",
            }));
        }
        Ok(Value::Array(branches))
    }

    fn conformance(&self) -> Value {
        let all: BTreeSet<&String> = self.vector_order.iter().collect();
        let design: Vec<Value> = all
            .iter()
            .filter(|path| !self.executable_vectors.contains(**path))
            .map(|path| Value::String((*path).clone()))
            .collect();
        let executable: Vec<Value> = self
            .executable_vectors
            .iter()
            .map(|path| Value::String(path.clone()))
            .collect();
        json!({ "design_vectors": design, "executable_vectors": executable })
    }

    fn requirements(&self) -> Result<Value> {
        let ids = self
            .coverage
            .get("requirements")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("coverage.json has no requirements array"))?;
        let default = self
            .coverage
            .get("default_source")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("coverage.json has no default_source"))?;
        let sources = self
            .coverage
            .get("sources")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("coverage.json has no sources array"))?;
        let fixtures = self.coverage.get("hash_fixture_evidence");
        let vectors = self.vectors();

        let mut sorted: Vec<&str> = ids.iter().filter_map(Value::as_str).collect();
        sorted.sort_unstable();
        let mut requirements = Vec::with_capacity(sorted.len());
        for requirement in sorted {
            let mut evidence = Vec::new();
            for vector in &vectors {
                let covers = vector
                    .get("covers")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("a vector has no covers array"))?;
                if covers
                    .iter()
                    .any(|value| value.as_str() == Some(requirement))
                {
                    evidence.push(json!({ "id": vector.get("id"), "kind": "vector" }));
                }
            }
            if let Some(ids) = fixtures.and_then(|value| value.get(requirement)) {
                for id in ids.as_array().into_iter().flatten() {
                    evidence.push(json!({ "id": id, "kind": "hash-fixture" }));
                }
            }
            let source = sources
                .iter()
                .find_map(|rule| {
                    let prefix = rule.get(0)?.as_str()?;
                    requirement.starts_with(prefix).then(|| rule.get(1))?
                })
                .and_then(Value::as_str)
                .unwrap_or(default);
            requirements.push(json!({
                "evidence": Value::Array(evidence),
                "id": requirement,
                "source": source,
            }));
        }
        Ok(Value::Array(requirements))
    }

    /// The canonical request-hash fixtures, computed through `substrate_wire`'s own functions.
    fn hash_fixtures(&self) -> Result<Value> {
        let mut cases = Vec::with_capacity(self.hash_cases.len());
        for case in &self.hash_cases {
            cases.push(hash_case(case)?);
        }
        Ok(Value::Array(cases))
    }

    fn compatibility(&self) -> Result<(u64, u64)> {
        let mut current = BTreeSet::new();
        for route in &self.routes {
            current.insert(string_at(route, "id")?.to_owned());
        }
        let preserves = self.predecessor_routes.intersection(&current).count() as u64;
        let adds = current.difference(&self.predecessor_routes).count() as u64;
        Ok((preserves, adds))
    }
}

fn result_ref(route: &Value) -> Result<String> {
    Ok(format!(
        "results/{}",
        file_name(string_at(route, "result_schema")?)
    ))
}

fn input_ref(route: &Value) -> Result<String> {
    Ok(format!(
        "inputs/{}",
        file_name(string_at(route, "input_schema")?)
    ))
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn string_at<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("route entry has no string {key}"))
}

fn path_parameters(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| {
            segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
                .map(|parameter| parameter.strip_prefix('*').unwrap_or(parameter).to_owned())
        })
        .collect()
}

/// One canonical-hash fixture case: authored request in, `substrate_wire`'s hashing out.
fn hash_case(case: &Value) -> Result<Value> {
    let id = string_at(case, "id")?;
    let method = string_at(case, "method")?;
    let address = string_at(case, "normalized_address")?;
    let raw_input = case
        .get("raw_input")
        .ok_or_else(|| anyhow!("hash case {id} has no raw_input"))?;
    let raw_query_hex = string_at(case, "raw_query")?;
    let raw_query_bytes =
        hex::decode(raw_query_hex).with_context(|| format!("hash case {id} raw_query hex"))?;
    let raw_query = String::from_utf8(raw_query_bytes)
        .with_context(|| format!("hash case {id} raw_query is not UTF-8"))?;

    let (input_mode, canonical_input) = match substrate_wire::canonical_json(raw_input) {
        Ok(canonical) => ("rfc8785-jcs", canonical),
        Err(_) => (
            "rejected-number-json",
            format!("rejected-number-json:{}", structural_json(raw_input)),
        ),
    };
    let canonical_query = substrate_wire::canonical_query(&raw_query)
        .map_err(|error| anyhow!("hash case {id} query does not canonicalize: {error}"))?;
    let (query_mode, query_pairs) = match canonical_query.split_once('\0') {
        Some(("pairs", pairs)) => (
            "pairs",
            serde_json::from_str::<Value>(pairs).context("canonical query pairs")?,
        ),
        Some(("malformed-raw", _)) => ("malformed-raw", Value::Null),
        _ => bail!("hash case {id} produced an untagged canonical query"),
    };

    let mut tuple = Vec::new();
    for field in [
        b"2".as_slice(),
        method.as_bytes(),
        address.as_bytes(),
        canonical_input.as_bytes(),
        canonical_query.as_bytes(),
    ] {
        let length = u32::try_from(field.len()).context("hash field length")?;
        tuple.extend_from_slice(&length.to_be_bytes());
        tuple.extend_from_slice(field);
    }
    let sha256 = hex::encode(Sha256::digest(&tuple));
    let expected = substrate_wire::canonical_request_hash_v2(
        method,
        address,
        raw_input,
        Some(raw_query.as_str()),
    )
    .map_err(|error| anyhow!("hash case {id} does not hash: {error}"))?;
    if sha256 != expected {
        bail!("hash case {id} disagrees with substrate_wire::canonical_request_hash_v2");
    }

    let operation = string_at(case, "operation")?;
    let subject = string_at(case, "subject")?;
    let deployment = string_at(case, "deployment")?;
    let mut fixture = json!({
        "canonical_input_hex": hex::encode(canonical_input.as_bytes()),
        "canonical_query": query_pairs,
        "canonical_query_hex": hex::encode(canonical_query.as_bytes()),
        "excluded": {
            "authorization": format!("Bearer sbt_authorization_{id}"),
            "bearer": format!("sbt_raw_{id}"),
            "deployment": deployment,
            "headers": { "traceparent": format!("00-{id}") },
            "operation": operation,
            "principal": format!("principal:{id}"),
            "request_id": format!("req_{}", id.replace('-', "_")),
            "subject": subject,
        },
        "hash_version": 2,
        "id": id,
        "input_mode": input_mode,
        "ledger_key": { "deployment": deployment, "operation": operation, "subject": subject },
        "method": method,
        "normalized_address": address,
        "query_mode": query_mode,
        "raw_input": raw_input,
        "raw_query": { "data": raw_query_hex, "encoding": "hex" },
        "sha256": sha256,
        "tuple_hex": hex::encode(&tuple),
    });
    if let Some(relation) = case.get("relation") {
        fixture
            .as_object_mut()
            .expect("fixture is an object")
            .insert("relation".to_owned(), relation.clone());
    }
    Ok(fixture)
}

/// Mirrors `substrate_wire`'s private `deterministic_structural_json`
/// (`crates/substrate-wire/src/lib.rs:1693`). Every case that uses it is cross-checked against
/// `canonical_request_hash_v2` in [`hash_case`], so a divergence refuses the render.
fn structural_json(value: &Value) -> String {
    let mut output = String::new();
    render_structural(value, &mut output);
    output
}

fn render_structural(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(true) => output.push_str("true"),
        Value::Bool(false) => output.push_str("false"),
        Value::Number(number) => output.push_str(&number.to_string()),
        Value::String(text) => {
            output.push_str(&serde_json::to_string(text).unwrap_or_default());
        }
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                render_structural(item, output);
            }
            output.push(']');
        }
        Value::Object(entries) => {
            let mut entries: Vec<_> = entries.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
            output.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).unwrap_or_default());
                output.push(':');
                render_structural(item, output);
            }
            output.push('}');
        }
    }
}

/// Replaces every marker object with its derived value.
fn substitute(value: &Value, derived: &BTreeMap<String, Value>) -> Result<Value> {
    if let Some(key) = marker(value) {
        return derived
            .get(&key)
            .cloned()
            .ok_or_else(|| anyhow!("unknown derived marker {key}"));
    }
    Ok(match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute(item, derived))
                .collect::<Result<Vec<_>>>()?,
        ),
        Value::Object(entries) => {
            let mut mapped = Map::new();
            for (key, item) in entries {
                mapped.insert(key.clone(), substitute(item, derived)?);
            }
            Value::Object(mapped)
        }
        other => other.clone(),
    })
}

const MARKERS: [&str; 8] = [
    "$compat",
    "$coverage",
    "$files",
    "$generator",
    "$hash",
    "$routes",
    "$vectors",
    "$wire",
];

fn marker(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let (key, selector) = object.iter().next()?;
    if !MARKERS.contains(&key.as_str()) {
        return None;
    }
    Some(format!("{key}/{}", selector.as_str()?))
}

fn manifest_files(rendered: &Rendered) -> Vec<Value> {
    rendered
        .iter()
        .map(|(path, bytes)| {
            let json = Path::new(path)
                .extension()
                .is_some_and(|extension| extension == "json");
            json!({
                "byte_length": bytes.len(),
                "media_type": if json { "application/json" } else { "text/markdown" },
                "path": path,
                "sha256": hex::encode(Sha256::digest(bytes)),
            })
        })
        .collect()
}

/// Sorted keys, two-space indent, one trailing newline, non-ASCII left as UTF-8.
///
/// This is `json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"`, the form every
/// released bundle is checked in as. `serde_json::Map` is a `BTreeMap` here, so key order is the
/// byte order `sort_keys` produces.
fn canonical_json(value: &Value) -> Vec<u8> {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text.into_bytes()
}

fn read_json(path: &Path) -> Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))
}

fn read_array(path: &Path) -> Result<Vec<Value>> {
    match read_json(path)? {
        Value::Array(items) => Ok(items),
        _ => bail!("{} is not a JSON array", path.display()),
    }
}

fn read_string_array(path: &Path) -> Result<Vec<String>> {
    read_array(path)?
        .into_iter()
        .map(|value| match value {
            Value::String(text) => Ok(text),
            other => Err(anyhow!(
                "{} holds a non-string entry {other}",
                path.display()
            )),
        })
        .collect()
}

fn read_documents(root: &Path) -> Result<BTreeMap<String, Value>> {
    let mut documents = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .with_context(|| format!("cannot read {}", directory.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot read {}", directory.display()))?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                bail!(
                    "{} is not JSON; the source tree holds documents only",
                    path.display()
                );
            }
            let relative = path
                .strip_prefix(root)
                .expect("entry is beneath the document root")
                .to_string_lossy()
                .into_owned();
            documents.insert(relative, read_json(&path)?);
        }
    }
    Ok(documents)
}

fn route_ids(operations: &Path) -> Result<BTreeSet<String>> {
    let registry = read_json(operations)?;
    let entries = registry
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("{} has no operations array", operations.display()))?;
    Ok(entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect())
}

fn digest_of(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// A released bundle directory is immutable (AGENTS.md invariant 6): the renderer never writes into
/// `contracts/`, whatever `--out` says.
fn refuse_released_tree(root: &Path, out: &Path) -> Result<()> {
    let contracts = root.join("contracts");
    if normalise(out).starts_with(normalise(&contracts)) {
        bail!(
            "refusing to render into {}: every directory under contracts/ is a released bundle and \
             is immutable (AGENTS.md invariant 6)",
            out.display()
        );
    }
    Ok(())
}

fn normalise(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut normalised = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalised.pop();
            }
            other => normalised.push(other),
        }
    }
    normalised
}

fn write_tree(out: &Path, rendered: &Rendered, force: bool) -> Result<()> {
    if out.exists() {
        let occupied = fs::read_dir(out)
            .with_context(|| format!("cannot read {}", out.display()))?
            .next()
            .is_some();
        if occupied && !force {
            bail!("{} is not empty; pass --force to replace it", out.display());
        }
        if occupied {
            fs::remove_dir_all(out).with_context(|| format!("cannot clear {}", out.display()))?;
        }
    }
    for (path, bytes) in rendered {
        let target = out.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        fs::write(&target, bytes).with_context(|| format!("cannot write {}", target.display()))?;
    }
    Ok(())
}
