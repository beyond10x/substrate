//! The report (design 31 § 6): format `b10x.substrate-remote-conformance-report.v1`, closed at
//! every level, written in deterministic form (sorted keys, two-space indent, trailing newline)
//! and atomically, with its SHA-256 beside it.

use std::collections::BTreeSet;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::probe::sha256_hex;
use crate::registry::Registry;

pub const FORMAT: &str = "b10x.substrate-remote-conformance-report.v1";

/// A case's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Passed,
    Failed,
    Absent,
}

/// Why a case is absent: one of a closed set (§ 3, step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AbsentReason {
    FactAbsent,
    FactUnsatisfied,
    ScopeNotDeclared,
    InputNotDeclared,
    BoundExceeded,
}

#[derive(Debug, Clone, Serialize)]
pub struct Absent {
    pub reason: AbsentReason,
    pub facts: Vec<String>,
    pub inputs: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Requires {
    pub operations: Vec<String>,
    pub facts: Vec<String>,
    pub scopes: Vec<String>,
    pub inputs: Vec<String>,
}

/// The status and code an unserved request was answered with, recorded without judgement.
#[derive(Debug, Clone, Serialize)]
pub struct Observed {
    pub status: u16,
    pub code: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Case {
    pub id: String,
    pub group: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absent: Option<Absent>,
    pub requires: Requires,
    pub diagnostics: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<Observed>,
    /// The branch a dropped response took: `admitted` or `not-admitted` (§ 4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// What detected a restart: `probed-at`, or `probed-at-and-exec` (§ 4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Runner {
    pub version: String,
    pub source_revision: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub started_at: String,
    pub finished_at: String,
    pub label: String,
    pub partial: bool,
    pub only: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    pub origin_sha256: String,
    pub server_name_sha256: String,
    pub tls_version: String,
    pub alpn: String,
    pub leaf_certificate_sha256: String,
    pub trust_roots_sha256: String,
    pub leaf_not_after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Contract {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotRead {
    pub snapshot: String,
    pub probed_at: String,
}

/// What the runner read of the capability snapshot; `None` (`null`) for a read whose values the
/// closed report cannot carry, or that did not happen.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilitySnapshot {
    pub first: Option<SnapshotRead>,
    pub last: Option<SnapshotRead>,
    pub driver: String,
    pub driver_version: String,
    pub config_generation: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bounds {
    pub deadline_s: u64,
    pub io_timeout_s: u64,
    pub restart_timeout_s: u64,
    pub max_generated_events: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Declared {
    pub scopes: Vec<String>,
    pub token_provider: String,
    pub restart_hook: bool,
    pub ops_origin: bool,
    pub forbidden_names: u64,
    pub bounds: Bounds,
}

/// What the report does not prove: a crash window and the in-repository reference that covers it
/// (§ 4), or a registry operation no case of this run names.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum NotCovered {
    Window { case: String, covered_by: String },
    Operation { operation: String },
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Totals {
    pub passed: u64,
    pub failed: u64,
    pub absent: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Passed,
    Failed,
    Incomplete,
}

impl Verdict {
    /// The exit code of a run with this verdict (§ 3; § 11 Q2).
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Failed => 1,
            Self::Incomplete => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub format: String,
    pub runner: Runner,
    pub run: Run,
    pub endpoint: Endpoint,
    pub contract: Contract,
    /// `None` when the first machine read did not decode into the wire types.
    pub capability_snapshot: Option<CapabilitySnapshot>,
    pub declared: Declared,
    pub cases: Vec<Case>,
    pub not_covered: Vec<NotCovered>,
    pub totals: Totals,
    pub verdict: Verdict,
}

/// The crash windows only fault injection inside the daemon reaches (§ 4), then every operation
/// of the pinned registry that no case of `cases` names in `requires.operations`, in registry
/// id order.
pub fn not_covered(registry: &Registry, cases: &[Case]) -> Vec<NotCovered> {
    let named: BTreeSet<&str> = cases
        .iter()
        .flat_map(|case| case.requires.operations.iter().map(String::as_str))
        .collect();
    let windows = [
        "crash-before-dispatch",
        "crash-after-dispatch",
        "restart-no-redispatch",
    ]
    .into_iter()
    .map(|case| NotCovered::Window {
        case: case.to_owned(),
        covered_by: "crates/substrate-daemon/tests/contract_vectors.rs".to_owned(),
    });
    let operations = registry
        .operations()
        .filter(|operation| !named.contains(operation.id.as_str()))
        .map(|operation| NotCovered::Operation {
            operation: operation.id.clone(),
        });
    windows.chain(operations).collect()
}

/// Totals and verdict: failed if any case failed; passed if none failed, the run is not partial
/// and every core case passed; incomplete otherwise.
pub fn judge(cases: &[Case], core_groups: &[&str], partial: bool) -> (Totals, Verdict) {
    let mut totals = Totals::default();
    for case in cases {
        match case.status {
            Status::Passed => totals.passed += 1,
            Status::Failed => totals.failed += 1,
            Status::Absent => totals.absent += 1,
        }
    }
    let core_passed = cases
        .iter()
        .filter(|case| core_groups.contains(&case.group.as_str()))
        .all(|case| case.status == Status::Passed);
    let verdict = if totals.failed > 0 {
        Verdict::Failed
    } else if core_passed && !partial {
        Verdict::Passed
    } else {
        Verdict::Incomplete
    };
    (totals, verdict)
}

/// The report's own schema, embedded so that every report is validated before it is written.
const SCHEMA: &str = include_str!("../schema/remote-conformance-report.v1.schema.json");

/// Whether a server-supplied word fits the schema's `word`: `^[0-9A-Za-z][0-9A-Za-z.+-]{0,31}$`.
/// No `_` and at most 32 bytes, so neither a credential (`identity_access_v1_` and a 43-byte body)
/// nor its body fits (design 31 § 5, point 5).
pub fn word_fits(value: &str) -> bool {
    value.len() <= 32
        && value
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

/// Whether rendered report bytes validate against the report's own schema.
pub fn conforms(bytes: &[u8]) -> bool {
    let Ok(schema) = serde_json::from_str::<Value>(SCHEMA) else {
        return false;
    };
    let Ok(validator) = jsonschema::validator_for(&schema) else {
        return false;
    };
    serde_json::from_slice::<Value>(bytes).is_ok_and(|report| validator.is_valid(&report))
}

/// The value with every object's keys in sorted order.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// The deterministic bytes of a report.
pub fn render(report: &Report) -> serde_json::Result<Vec<u8>> {
    let value = sorted(serde_json::to_value(report)?);
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Creates `out` (mode 0700; it must not exist).
pub fn create_out(out: &Path) -> std::io::Result<()> {
    DirBuilder::new().mode(0o700).create(out)
}

/// Writes `report.json` atomically and `report.json.sha256` in `sha256sum` form.
pub fn write(out: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic(out, "report.json", bytes)?;
    let digest = format!("{}  report.json\n", sha256_hex(bytes));
    write_atomic(out, "report.json.sha256", digest.as_bytes())
}

fn write_atomic(out: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let staging = out.join(format!(".{name}.partial"));
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&staging, out.join(name))?;
    File::open(out)?.sync_all()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Case, NotCovered, Requires, Status, Verdict, judge, not_covered, sorted};

    fn case(group: &str, status: Status) -> Case {
        Case {
            id: format!("{group}.case"),
            group: group.to_owned(),
            status,
            absent: None,
            requires: Requires::default(),
            diagnostics: Vec::new(),
            observed: None,
            branch: None,
            detection: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn the_verdict_needs_every_core_case_passed_and_no_failure() {
        let core = ["machine"];
        let passed = [
            case("machine", Status::Passed),
            case("exec", Status::Absent),
        ];
        assert_eq!(judge(&passed, &core, false).1, Verdict::Passed);
        assert_eq!(judge(&passed, &core, true).1, Verdict::Incomplete);
        let absent_core = [case("machine", Status::Absent)];
        assert_eq!(judge(&absent_core, &core, false).1, Verdict::Incomplete);
        let failed = [
            case("machine", Status::Passed),
            case("exec", Status::Failed),
        ];
        let (totals, verdict) = judge(&failed, &core, false);
        assert_eq!(verdict, Verdict::Failed);
        assert_eq!((totals.passed, totals.failed, totals.absent), (1, 1, 0));
        assert_eq!(
            [Verdict::Passed, Verdict::Failed, Verdict::Incomplete].map(Verdict::exit_code),
            [0, 1, 4]
        );
    }

    /// Finding `uncovered-operations-unlisted`: `not_covered` is derived from the pinned registry
    /// and the cases of the run, so no operation is left out of both.
    #[test]
    fn every_registry_operation_is_named_by_a_case_or_listed_not_covered() {
        let registry = crate::registry::Registry::pinned();
        let mut machine = case("machine", Status::Passed);
        machine.requires.operations = vec!["machine.get".to_owned()];
        let mut exec = case("exec", Status::Absent);
        exec.requires.operations = vec!["exec.start".to_owned(), "exec.get".to_owned()];
        let cases = [machine, exec];
        let listed = not_covered(&registry, &cases);
        let operations: Vec<&str> = listed
            .iter()
            .filter_map(|entry| match entry {
                NotCovered::Operation { operation } => Some(operation.as_str()),
                NotCovered::Window { .. } => None,
            })
            .collect();
        let named: std::collections::BTreeSet<&str> = cases
            .iter()
            .flat_map(|case| case.requires.operations.iter().map(String::as_str))
            .collect();
        let mut count = 0;
        for operation in registry.operations() {
            count += 1;
            assert!(
                named.contains(operation.id.as_str())
                    != operations.contains(&operation.id.as_str()),
                "{} is named by a case or listed, never both or neither",
                operation.id
            );
        }
        assert!(count > 40, "the pinned registry is read, not a hand list");
        assert_eq!(operations.len(), count - named.len());
        let windows = listed
            .iter()
            .filter(|entry| matches!(entry, NotCovered::Window { .. }))
            .count();
        assert_eq!(windows, 3, "the crash windows of § 4 stay listed");
    }

    /// Security review (u1): no credential, no credential body and no `_` fits a word; and a
    /// report outside the schema is caught before it is written.
    #[test]
    fn a_word_never_holds_a_credential_and_a_report_is_checked_against_its_schema() {
        let body = "SENTINELsentinelSENTINELsentinelSENTINELsen";
        assert_eq!(body.len(), 43);
        for refused in [
            format!("identity_access_v1_{body}").as_str(),
            body,
            &body[..33],
            "identity_access_v1_",
            "a_b",
            "",
            "-1",
        ] {
            assert!(!super::word_fits(refused), "{refused}");
        }
        for admitted in ["0.17.0", "host", "kubernetes", "1.0.0+build.7", &body[..32]] {
            assert!(super::word_fits(admitted), "{admitted}");
        }
        let schema: serde_json::Value =
            serde_json::from_str(super::SCHEMA).expect("the embedded schema is JSON");
        jsonschema::draft202012::meta::validate(&schema).expect("the schema meta-validates");
        assert!(!super::conforms(b"{}"));
        assert!(!super::conforms(b"not json"));
    }

    #[test]
    fn rendering_sorts_keys_at_every_level() {
        let value = sorted(json!({"b": {"d": 1, "c": [{"f": 1, "e": 2}]}, "a": 0}));
        assert_eq!(
            serde_json::to_string(&value).expect("render"),
            r#"{"a":0,"b":{"c":[{"e":2,"f":1}],"d":1}}"#
        );
    }
}
