//! The case engine (design 31 § 3): selection from facts, scopes, inputs and bounds; one
//! sequential run under one deadline; closed diagnostic codes.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use b10x_substrate_sdk::{AccessToken, AccessTokenReason, Client, SdkError};
use serde_json::Value;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::probe::{Fault, Probe, Reply};
use crate::registry::{FactStanding, Registry, needed_fact_standing};
use crate::report::{Absent, AbsentReason, Case, Observed, Requires, Status};
use crate::token::{Purpose, Reason, SharedSource, TokenError};

/// The groups absence cannot satisfy (§ 3).
pub const CORE_GROUPS: [&str; 6] = ["machine", "tls", "auth", "workspace", "recovery", "cleanup"];

/// Every group, in run order.
pub const GROUPS: [&str; 17] = [
    "machine",
    "tls",
    "tls-name",
    "auth",
    "auth-scope",
    "workspace",
    "unserved",
    "recovery",
    "exec",
    "resources",
    "sessions",
    "events",
    "retention-gap",
    "restart",
    "restart-exec",
    "ops",
    "cleanup",
];

/// The label key every resource the run creates carries.
pub const LABEL_KEY: &str = "conformance-run";

/// One case: what it calls, the facts and inputs it needs beyond that.
pub struct Spec {
    pub id: &'static str,
    pub group: &'static str,
    pub operations: &'static [&'static str],
    pub facts: &'static [&'static str],
    pub inputs: &'static [&'static str],
    /// An unserved-inventory case: it runs because its operation's facts fail (§ 3, step 4).
    pub unserved: bool,
}

/// The event head: the cursor after every retained event, its generation and sequence.
pub struct Head {
    pub cursor: String,
    pub generation: u64,
    pub through_seq: u64,
}

/// A failed case's diagnostic codes.
#[derive(Debug)]
pub struct Fail(pub Vec<String>);

/// A diagnostic or wire code the closed report admits: `^[a-z][a-z0-9.-]{0,127}$`.
pub fn code_ok(code: &str) -> bool {
    let mut bytes = code.bytes();
    code.len() <= 128
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

impl Fail {
    pub fn new(code: &'static str) -> Self {
        Self(vec![code.to_owned()])
    }

    /// Adds an observed wire code when the report can carry it.
    #[must_use]
    pub fn with(mut self, code: Option<&str>) -> Self {
        if let Some(code) = code {
            self.0.push(if code_ok(code) {
                code.to_owned()
            } else {
                "code-unrepresentable".to_owned()
            });
        }
        self
    }
}

impl From<Fault> for Fail {
    fn from(fault: Fault) -> Self {
        Self::new(fault.code())
    }
}

/// Maps an SDK error to codes, never to its text.
pub fn sdk(error: &SdkError) -> Fail {
    match error {
        SdkError::Refusal(refusal) => Fail::new("refused").with(Some(&refusal.code)),
        SdkError::Transport(_) => Fail::new("transport-failed"),
        SdkError::Protocol(_) => Fail::new("protocol-error"),
        SdkError::TokenUnavailable => Fail::new("token-unavailable"),
        SdkError::UnknownOperation { .. } => Fail::new("operation-unknown"),
        SdkError::EventGap { code, .. } => Fail::new("event-gap").with(Some(code)),
        SdkError::ContractMismatch { .. } => Fail::new("contract-mismatch"),
        _ => Fail::new("sdk-error"),
    }
}

/// The refusal code of an SDK error, if it is a refusal.
pub fn refusal_code(error: &SdkError) -> Option<&str> {
    match error {
        SdkError::Refusal(refusal) => Some(&refusal.code),
        _ => None,
    }
}

pub fn ensure(holds: bool, code: &'static str) -> Result<(), Fail> {
    if holds { Ok(()) } else { Err(Fail::new(code)) }
}

/// A reply with this status, the pinned contract pair, and (for an error) this code.
pub fn expect(reply: &Reply, status: u16, code: Option<&str>) -> Result<(), Fail> {
    if reply.status != status {
        return Err(Fail::new("unexpected-status").with(reply.error_code().as_deref()));
    }
    if !reply.contract_pinned() {
        return Err(Fail::new("contract-header-missing"));
    }
    if let Some(code) = code {
        let observed = reply.error_code();
        if observed.as_deref() != Some(code) {
            return Err(Fail::new("unexpected-code").with(observed.as_deref()));
        }
    }
    Ok(())
}

/// What a passing (or dynamically absent) case adds to its report entry.
#[derive(Debug, Default)]
pub struct Note {
    pub observed: Option<Observed>,
    pub branch: Option<&'static str>,
    pub detection: Option<&'static str>,
    pub absent: Option<Absent>,
}

pub type CaseResult = Result<Note, Fail>;

/// The restart hook (§ 4): argv-only, its own environment, no inherited descriptor.
pub struct Hook {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, OsString)>,
}

/// What one run is configured with.
pub struct Settings {
    pub scopes: Vec<String>,
    pub only: Option<BTreeSet<String>>,
    pub io: Duration,
    pub deadline: Instant,
    pub restart_timeout: Duration,
    pub probe_shell: String,
    pub max_generated_events: u64,
    pub hook: Option<Hook>,
    pub ops_authority: Option<String>,
    pub forbidden_names: Vec<String>,
    pub label: String,
    pub endpoint: String,
    pub trust_roots: PathBuf,
    pub server_name: String,
}

/// An SDK client on the endpoint whose credentials come from the declared provider.
pub async fn connect(settings: &Settings, tokens: &SharedSource) -> Result<Client, SdkError> {
    let source = std::sync::Arc::clone(tokens);
    let scopes = settings.scopes.clone();
    Client::builder()
        .https_endpoint(settings.endpoint.clone())
        .trust_roots(settings.trust_roots.clone())
        .server_identity(settings.server_name.clone())
        .token_provider(move |reason: AccessTokenReason| {
            let source = std::sync::Arc::clone(&source);
            let scopes = scopes.clone();
            async move {
                let reason = match reason {
                    AccessTokenReason::Request => Reason::Request,
                    AccessTokenReason::RefreshAfterAuthorizationFailure => Reason::Refresh,
                };
                let mut token = source
                    .fetch(reason, Purpose::Primary, &scopes)
                    .await
                    .map_err(|_| SdkError::TokenUnavailable)?;
                // Moved, not copied: the provider's buffer is left empty and the SDK zeroes its own.
                AccessToken::new(std::mem::take(&mut *token))
            }
        })
        .connect()
        .await
}

/// What a case requires: its operations, the fact-only predicates and scopes the pinned registry
/// gives them, and the facts and inputs the case names itself.
pub fn requires(registry: &Registry, spec: &Spec) -> Requires {
    let mut facts: BTreeSet<String> = spec.facts.iter().map(|fact| (*fact).to_owned()).collect();
    let mut scopes = BTreeSet::new();
    for id in spec.operations {
        if let Some(scope) = registry.scope(id) {
            scopes.insert(scope.to_owned());
            if let Some(operation) = registry.operation(id) {
                facts.extend(
                    operation
                        .capability_predicates
                        .iter()
                        .filter(|predicate| predicate.input_pointer.is_none())
                        .map(|predicate| predicate.fact.clone()),
                );
            }
        }
    }
    Requires {
        operations: spec.operations.iter().map(|id| (*id).to_owned()).collect(),
        facts: facts.into_iter().collect(),
        scopes: scopes.into_iter().collect(),
        inputs: spec
            .inputs
            .iter()
            .map(|input| (*input).to_owned())
            .collect(),
    }
}

/// The run's state between cases.
#[derive(Default)]
pub struct State {
    /// The workspace the workspace, unserved and recovery groups share.
    pub workspace: Option<String>,
    pub workspaces: Vec<String>,
    pub execs: Vec<String>,
    pub sessions: Vec<String>,
    pub operations: Vec<String>,
    pub restart: Option<crate::cases_exec::Restarted>,
    pub restart_ran: bool,
    pub expired: bool,
}

pub struct Ctx {
    pub registry: Registry,
    pub facts: Value,
    pub snapshot: String,
    pub probe: Probe,
    pub tokens: SharedSource,
    pub client: Client,
    pub settings: Settings,
    pub state: State,
    pub cases: Vec<Case>,
}

enum Selection {
    Skip,
    Absent(Absent),
    Run,
}

impl Ctx {
    pub fn selected(&self, group: &str) -> bool {
        self.settings.only.as_ref().is_none_or(|only| {
            only.contains(group) || (group == "restart-exec" && only.contains("restart"))
        })
    }

    pub fn fact(&self, name: &str) -> Option<&Value> {
        self.facts.get(name).filter(|value| !value.is_null())
    }

    pub fn fact_true(&self, name: &str) -> bool {
        self.fact(name) == Some(&Value::Bool(true))
    }

    pub fn input_declared(&self, input: &str) -> bool {
        match input {
            "restart-hook" => self.settings.hook.is_some(),
            "ops-origin" => self.settings.ops_authority.is_some(),
            "forbidden-name" => !self.settings.forbidden_names.is_empty(),
            _ => true,
        }
    }

    pub fn requires(&self, spec: &Spec) -> Requires {
        requires(&self.registry, spec)
    }

    fn select(&self, spec: &Spec) -> Selection {
        if !self.selected(spec.group) {
            return Selection::Skip;
        }
        let mut failing = std::collections::BTreeMap::new();
        for id in spec.operations {
            // An operation outside every hosted family is never evaluated (§ 3, step 5).
            if !spec.unserved && self.registry.scope(id).is_some() {
                failing.extend(self.registry.failing_facts(id, &self.facts));
            }
        }
        for fact in spec.facts {
            if let Some(standing) = needed_fact_standing(fact, &self.facts) {
                failing.insert((*fact).to_owned(), standing);
            }
        }
        if !failing.is_empty() {
            let reason = if failing
                .values()
                .any(|standing| *standing == FactStanding::Absent)
            {
                AbsentReason::FactAbsent
            } else {
                AbsentReason::FactUnsatisfied
            };
            return Selection::Absent(Absent {
                reason,
                facts: failing.into_keys().collect(),
                inputs: Vec::new(),
                scopes: Vec::new(),
            });
        }
        let missing_scopes: BTreeSet<String> = spec
            .operations
            .iter()
            .filter_map(|id| self.registry.scope(id))
            .filter(|scope| {
                !self
                    .settings
                    .scopes
                    .iter()
                    .any(|declared| declared == scope)
            })
            .map(ToOwned::to_owned)
            .collect();
        if !missing_scopes.is_empty() {
            return Selection::Absent(Absent {
                reason: AbsentReason::ScopeNotDeclared,
                facts: Vec::new(),
                inputs: Vec::new(),
                scopes: missing_scopes.into_iter().collect(),
            });
        }
        let missing_inputs: Vec<String> = spec
            .inputs
            .iter()
            .filter(|input| !self.input_declared(input))
            .map(|input| (*input).to_owned())
            .collect();
        if !missing_inputs.is_empty() {
            return Selection::Absent(Absent {
                reason: AbsentReason::InputNotDeclared,
                facts: Vec::new(),
                inputs: missing_inputs,
                scopes: Vec::new(),
            });
        }
        Selection::Run
    }

    /// Whether a case would run, without recording it.
    pub fn would_run(&self, spec: &Spec) -> bool {
        matches!(self.select(spec), Selection::Run)
    }

    fn record(
        &mut self,
        spec: &Spec,
        status: Status,
        note: Note,
        diagnostics: Vec<String>,
        elapsed: Duration,
    ) {
        let requires = self.requires(spec);
        self.cases.push(Case {
            id: spec.id.to_owned(),
            group: spec.group.to_owned(),
            status,
            absent: note.absent,
            requires,
            diagnostics,
            observed: note.observed,
            branch: note.branch.map(ToOwned::to_owned),
            detection: note.detection.map(ToOwned::to_owned),
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        });
    }

    /// Records a case as failed without running it.
    pub fn record_failed(&mut self, spec: &Spec, code: &'static str) {
        match self.select(spec) {
            Selection::Skip => {}
            Selection::Absent(absent) => {
                self.record(
                    spec,
                    Status::Absent,
                    Note {
                        absent: Some(absent),
                        ..Note::default()
                    },
                    Vec::new(),
                    Duration::ZERO,
                );
            }
            Selection::Run => {
                self.record(
                    spec,
                    Status::Failed,
                    Note::default(),
                    vec![code.to_owned()],
                    Duration::ZERO,
                );
            }
        }
    }

    /// Selects, runs and records one case under the remaining deadline. Returns whether it passed.
    pub async fn case(
        &mut self,
        spec: &Spec,
        body: impl AsyncFnOnce(&mut Self) -> CaseResult,
    ) -> bool {
        match self.select(spec) {
            Selection::Skip => return false,
            Selection::Absent(absent) => {
                self.record(
                    spec,
                    Status::Absent,
                    Note {
                        absent: Some(absent),
                        ..Note::default()
                    },
                    Vec::new(),
                    Duration::ZERO,
                );
                return false;
            }
            Selection::Run => {}
        }
        if self.state.expired || Instant::now() >= self.settings.deadline {
            self.state.expired = true;
            self.record(
                spec,
                Status::Failed,
                Note::default(),
                vec!["deadline-exceeded".to_owned()],
                Duration::ZERO,
            );
            return false;
        }
        let started = Instant::now();
        let deadline = self.settings.deadline;
        let outcome = if let Ok(outcome) = tokio::time::timeout_at(deadline, body(self)).await {
            outcome
        } else {
            self.state.expired = true;
            Err(Fail::new("deadline-exceeded"))
        };
        let elapsed = started.elapsed();
        match outcome {
            Ok(note) if note.absent.is_some() => {
                self.record(spec, Status::Absent, note, Vec::new(), elapsed);
                false
            }
            Ok(note) => {
                self.record(spec, Status::Passed, note, Vec::new(), elapsed);
                true
            }
            Err(fail) => {
                let mut codes = fail.0;
                codes.sort();
                codes.dedup();
                self.record(spec, Status::Failed, Note::default(), codes, elapsed);
                false
            }
        }
    }

    /// Bounds one SDK call by `--io-timeout`.
    pub async fn io<T>(&self, call: impl Future<Output = Result<T, SdkError>>) -> Result<T, Fail> {
        match tokio::time::timeout(self.settings.io, Box::pin(call)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(sdk(&error)),
            Err(_) => Err(Fail::new("io-timeout")),
        }
    }

    /// Bounds one SDK call by `--io-timeout` and keeps its error for inspection.
    pub async fn io_raw<T>(
        &self,
        call: impl Future<Output = Result<T, SdkError>>,
    ) -> Result<Result<T, SdkError>, Fail> {
        tokio::time::timeout(self.settings.io, Box::pin(call))
            .await
            .map_err(|_| Fail::new("io-timeout"))
    }

    pub async fn token(
        &self,
        purpose: Purpose,
        reason: Reason,
    ) -> Result<Zeroizing<String>, TokenError> {
        let scopes = match purpose {
            Purpose::Primary => self.settings.scopes.clone(),
            Purpose::Narrowed => vec!["observe".to_owned()],
        };
        self.tokens.fetch(reason, purpose, &scopes).await
    }

    /// One raw request under the primary credential, refreshed once after a named hosted 401.
    pub async fn raw(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<Reply, Fail> {
        let token = self
            .token(Purpose::Primary, Reason::Request)
            .await
            .map_err(|_| Fail::new("token-unavailable"))?;
        let reply = self
            .probe
            .exchange(method, path, Some(&token), body)
            .await?;
        drop(token);
        if reply.status == 401
            && reply.error_code().as_deref() == Some(substrate_wire::AUTH_AUTHORITY_INVALID)
        {
            let token = self
                .token(Purpose::Primary, Reason::Refresh)
                .await
                .map_err(|_| Fail::new("token-unavailable"))?;
            return Ok(self
                .probe
                .exchange(method, path, Some(&token), body)
                .await?);
        }
        Ok(reply)
    }

    /// A fresh operation id, remembered so the ops listener can be checked for it.
    pub fn operation_id(&mut self) -> String {
        let id = format!("conf-{}", ulid::Ulid::generate());
        self.state.operations.push(id.clone());
        id
    }

    /// The JSON body of a mutation.
    pub fn mutation(operation: &str, input: &Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "op": operation, "input": input }))
            .unwrap_or_default()
    }

    /// One raw event page: the page, or the refusal code it was answered with.
    pub async fn events(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Result<Value, String>, Fail> {
        let mut path = format!("/v1/events?limit={limit}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&query_escape(cursor));
        }
        let reply = self.raw("GET", &path, None).await?;
        if reply.status == 200 {
            let page = reply
                .result()
                .ok_or_else(|| Fail::new("envelope-undecodable"))?;
            serde_json::from_value::<substrate_wire::EventPage>(page.clone())
                .map_err(|_| Fail::new("envelope-undecodable"))?;
            return Ok(Ok(page));
        }
        reply
            .error_code()
            .map(Err)
            .ok_or_else(|| Fail::new("envelope-undecodable"))
    }

    /// The cursor after every retained event: pages until a page is short.
    pub async fn head_cursor(&self) -> Result<Head, Fail> {
        let mut cursor: Option<String> = None;
        loop {
            let page = self
                .events(cursor.as_deref(), 1000)
                .await?
                .map_err(|code| Fail::new("events-unreadable").with(Some(&code)))?;
            let count = page["items"].as_array().map_or(0, Vec::len);
            let next = page["next_cursor"]
                .as_str()
                .ok_or_else(|| Fail::new("envelope-undecodable"))?
                .to_owned();
            if count < 1000 {
                return Ok(Head {
                    cursor: next,
                    generation: page["generation"].as_u64().unwrap_or(0),
                    through_seq: page["through_seq"].as_u64().unwrap_or(0),
                });
            }
            cursor = Some(next);
        }
    }

    /// Every event after `cursor`, up to `limit` items.
    pub async fn events_after(&self, cursor: &str) -> Result<Vec<Value>, Fail> {
        let page = self
            .events(Some(cursor), 1000)
            .await?
            .map_err(|code| Fail::new("events-unreadable").with(Some(&code)))?;
        Ok(page["items"].as_array().cloned().unwrap_or_default())
    }

    /// The SDK handle of a workspace.
    pub async fn workspace(&self, id: &str) -> Result<b10x_substrate_sdk::Workspace, Fail> {
        self.io(self.client.get_workspace(id)).await
    }

    /// A fresh SDK client, after a restart changed the snapshot it pinned at connect.
    pub async fn reconnect(&self) -> Result<Client, Fail> {
        self.io(connect(&self.settings, &self.tokens)).await
    }
}

/// Percent-encodes a query value outside the unreserved set.
pub fn query_escape(value: &str) -> String {
    use std::fmt::Write as _;
    value.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::{Fail, code_ok, query_escape};

    #[test]
    fn only_closed_codes_enter_a_report() {
        assert!(code_ok("auth.scope-denied"));
        assert!(code_ok("io-timeout"));
        assert!(!code_ok("identity_access_v1_x"));
        assert!(!code_ok("Upper"));
        assert!(!code_ok(""));
        assert_eq!(
            Fail::new("refused").with(Some("has space")).0,
            ["refused", "code-unrepresentable"]
        );
    }

    #[test]
    fn query_values_are_escaped_outside_the_unreserved_set() {
        assert_eq!(query_escape("ev2.scope_a.1.2"), "ev2.scope_a.1.2");
        assert_eq!(query_escape("a b&c"), "a%20b%26c");
    }
}
