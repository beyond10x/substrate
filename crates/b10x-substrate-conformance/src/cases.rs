//! The inventory's core groups and the event groups (design 31 § 3 and § 4): machine, TLS,
//! auth, workspace, unserved, recovery, events and the retention gap.

use std::time::Duration;

use b10x_substrate_sdk::{
    ExpectedFileState, FileEditInput, OperationState, SdkError, TextMatchPolicy, WorkspaceState,
};
use base64::Engine as _;
use serde_json::{Value, json};

use crate::probe::{Reply, sha256_hex};
use crate::report::{Absent, AbsentReason, Observed};
use crate::run::{CaseResult, Ctx, Fail, LABEL_KEY, Note, Spec, ensure, expect, refusal_code};
use crate::token::{Purpose, Reason, TokenError};

pub const fn spec(
    id: &'static str,
    group: &'static str,
    operations: &'static [&'static str],
    facts: &'static [&'static str],
    inputs: &'static [&'static str],
) -> Spec {
    Spec {
        id,
        group,
        operations,
        facts,
        inputs,
        unserved: false,
    }
}

const MACHINE_ENVELOPE: Spec = spec(
    "machine.envelope-decodes",
    "machine",
    &["machine.get"],
    &[],
    &[],
);
const MACHINE_CONTRACT: Spec = spec(
    "machine.contract-pinned",
    "machine",
    &["machine.get"],
    &[],
    &[],
);
const MACHINE_VALID_UNTIL: Spec =
    spec("machine.valid-until", "machine", &["machine.get"], &[], &[]);
pub const MACHINE_STABLE: Spec = spec(
    "machine.snapshot-stable",
    "machine",
    &["machine.get"],
    &[],
    &[],
);
const TLS_PLAINTEXT: Spec = spec("tls.plaintext-unanswered", "tls", &[], &[], &[]);
const TLS_12: Spec = spec("tls.tls12-refused", "tls", &[], &[], &[]);
const TLS_H2: Spec = spec("tls.h2-only-refused", "tls", &[], &[], &[]);
const TLS_NAME: Spec = spec(
    "tls-name.forbidden-names-refused",
    "tls-name",
    &[],
    &[],
    &["forbidden-name"],
);
const AUTH_ABSENT: Spec = spec("auth.credential-absent", "auth", &["machine.get"], &[], &[]);
const AUTH_INVALID: Spec = spec("auth.authority-invalid", "auth", &["machine.get"], &[], &[]);
const AUTH_UNRECORDED: Spec = spec(
    "auth.refused-mutation-unrecorded",
    "auth",
    &["workspace.create", "operation.get"],
    &[],
    &[],
);
const AUTH_PROCESS: Spec = spec(
    "auth.process-scope-denied",
    "auth",
    &[
        "process.start",
        "process.list",
        "process.get",
        "process.stop",
        "process.stop-by-owner",
        "process.lease.renew",
        "process.logs.get",
        "process.events",
    ],
    &[],
    &[],
);
const SCOPE_SESSIONS: Spec = spec(
    "auth-scope.narrowed-sessions-denied",
    "auth-scope",
    &[],
    &[],
    &["narrowed-credential"],
);
const SCOPE_WORKSPACES: Spec = spec(
    "auth-scope.narrowed-workspaces-denied",
    "auth-scope",
    &[],
    &[],
    &["narrowed-credential"],
);
const WS_CREATE: Spec = spec(
    "workspace.create",
    "workspace",
    &["workspace.create", "workspace.get"],
    &[],
    &[],
);
const WS_WRITE: Spec = spec(
    "workspace.write",
    "workspace",
    &["workspace.file.write", "workspace.file.read"],
    &[],
    &[],
);
const WS_READ_V1: Spec = spec(
    "workspace.read-v1",
    "workspace",
    &["workspace.file.read"],
    &[],
    &[],
);
const WS_READ_V2: Spec = spec(
    "workspace.read-v2",
    "workspace",
    &["workspace.file.read-v2"],
    &[],
    &[],
);
const WS_TREE: Spec = spec(
    "workspace.tree",
    "workspace",
    &["workspace.tree.read-v2"],
    &[],
    &[],
);
const WS_REPLACE: Spec = spec(
    "workspace.replace",
    "workspace",
    &["workspace.file.replace-v2", "workspace.file.read-v2"],
    &[],
    &[],
);
const WS_EDIT: Spec = spec(
    "workspace.edit",
    "workspace",
    &["workspace.file.edit-v2", "workspace.file.read-v2"],
    &[],
    &[],
);
const WS_DELETE: Spec = spec(
    "workspace.delete",
    "workspace",
    &["workspace.file.delete", "workspace.file.read"],
    &[],
    &[],
);
const WS_OUT_OF_ROOT: Spec = spec(
    "workspace.out-of-root",
    "workspace",
    &["workspace.file.read"],
    &[],
    &[],
);
const WS_DESTROY: Spec = spec(
    "workspace.destroy",
    "workspace",
    &["workspace.create", "workspace.destroy", "workspace.get"],
    &[],
    &[],
);
const RECOVERY_DISCARDED: Spec = spec(
    "recovery.response-discarded",
    "recovery",
    &[
        "workspace.file.write",
        "operation.get",
        "workspace.file.read-v2",
    ],
    &[],
    &[],
);
const RECOVERY_DROPPED: Spec = spec(
    "recovery.response-dropped",
    "recovery",
    &[
        "workspace.file.write",
        "operation.get",
        "workspace.file.read-v2",
    ],
    &[],
    &[],
);
const RECOVERY_CONFLICT: Spec = spec(
    "recovery.replay-conflict",
    "recovery",
    &["workspace.file.write"],
    &[],
    &[],
);
const EVENTS_ORDER: Spec = spec(
    "events.commit-order",
    "events",
    &[
        "event.list",
        "workspace.create",
        "workspace.file.write",
        "workspace.destroy",
    ],
    &[],
    &[],
);
const EVENTS_VISIBLE: Spec = spec(
    "events.visible-after-state",
    "events",
    &[
        "event.list",
        "workspace.create",
        "workspace.get",
        "operation.get",
    ],
    &[],
    &[],
);
const EVENTS_STREAM: Spec = spec(
    "events.stream-equals-pull",
    "events",
    &["event.list", "event.stream", "workspace.file.write"],
    &[],
    &[],
);
const RETENTION_GAP: Spec = spec(
    "retention-gap.cursor-refused",
    "retention-gap",
    &["event.list", "workspace.file.write"],
    &["events.retention-events"],
    &[],
);

/// The operations whose positive request the inventory sends on its own (§ 3, step 4).
const UNSERVABLE: [Spec; 5] = [
    unserved_spec("unserved.workspace.create", "workspace.create"),
    unserved_spec("unserved.exec.start", "exec.start"),
    unserved_spec("unserved.session.capabilities", "session.capabilities"),
    unserved_spec("unserved.session.start", "session.start"),
    unserved_spec("unserved.event.list", "event.list"),
];

const fn unserved_spec(id: &'static str, operation: &'static str) -> Spec {
    let operations: &'static [&'static str] = match operation.as_bytes() {
        b"workspace.create" => &["workspace.create"],
        b"exec.start" => &["exec.start"],
        b"session.capabilities" => &["session.capabilities"],
        b"session.start" => &["session.start"],
        _ => &["event.list"],
    };
    Spec {
        id,
        group: "unserved",
        operations,
        facts: &[],
        inputs: &[],
        unserved: true,
    }
}

const FILE: &str = "conformance-a.txt";
const FIRST: &[u8] = b"conformance first\n";
const SECOND: &[u8] = b"conformance second\n";
const EDITED: &[u8] = b"conformance edited\n";

pub fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The JSON input of a file write.
pub fn write_input(bytes: &[u8]) -> Value {
    json!({ "content": { "data": base64(bytes), "encoding": "base64" } })
}

/// A credential in the bundle's format that no authority issued.
fn forged_credential() -> Result<zeroize::Zeroizing<String>, Fail> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0_u8; 43];
    getrandom::fill(&mut bytes).map_err(|_| Fail::new("randomness-unavailable"))?;
    let mut credential = zeroize::Zeroizing::new(String::from("identity_access_v1_"));
    credential.extend(
        bytes
            .iter()
            .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()])),
    );
    Ok(credential)
}

pub fn decode_machine(reply: &Reply) -> Option<substrate_wire::CapabilitySnapshot> {
    serde_json::from_slice::<substrate_wire::Success<substrate_wire::CapabilitySnapshot>>(
        &reply.body,
    )
    .ok()
    .map(|success| success.result)
}

/// A 2xx reply with the pinned pair, and its `result`.
pub fn success(reply: &Reply) -> Result<Value, Fail> {
    if !(200..300).contains(&reply.status) {
        return Err(Fail::new("unexpected-status").with(reply.error_code().as_deref()));
    }
    ensure(reply.contract_pinned(), "contract-header-missing")?;
    reply
        .result()
        .ok_or_else(|| Fail::new("envelope-undecodable"))
}

fn need_workspace(ctx: &Ctx) -> Result<String, Fail> {
    ctx.state
        .workspace
        .clone()
        .ok_or_else(|| Fail::new("workspace-unavailable"))
}

fn is_not_found(error: &SdkError) -> bool {
    refusal_code(error) == Some("resource.not-found")
}

pub async fn machine(ctx: &mut Ctx) {
    ctx.case(&MACHINE_ENVELOPE, async |ctx: &mut Ctx| {
        let reply = ctx.raw("GET", "/v1/machine", None).await?;
        expect(&reply, 200, None)?;
        decode_machine(&reply).ok_or_else(|| Fail::new("envelope-undecodable"))?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&MACHINE_CONTRACT, async |ctx: &mut Ctx| {
        let reply = ctx.raw("GET", "/v1/machine", None).await?;
        ensure(reply.contract_pinned(), "contract-unpinned")?;
        ctx.io(ctx.client.refresh_machine()).await?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&MACHINE_VALID_UNTIL, async |ctx: &mut Ctx| {
        let reply = ctx.raw("GET", "/v1/machine", None).await?;
        let snapshot = decode_machine(&reply).ok_or_else(|| Fail::new("envelope-undecodable"))?;
        ensure(
            snapshot
                .valid_until
                .is_none_or(|until| until > chrono::Utc::now()),
            "valid-until-passed",
        )?;
        Ok(Note::default())
    })
    .await;
}

/// The last read of the snapshot equals the first unless the restart group ran.
pub async fn snapshot_stable(ctx: &mut Ctx, first: &str, last: Option<&str>) {
    ctx.case(&MACHINE_STABLE, async |ctx: &mut Ctx| {
        let last = last.ok_or_else(|| Fail::new("machine-unreadable"))?;
        ensure(ctx.state.restart_ran || first == last, "snapshot-changed")?;
        Ok(Note::default())
    })
    .await;
}

pub async fn tls(ctx: &mut Ctx) {
    ctx.case(&TLS_PLAINTEXT, async |ctx: &mut Ctx| {
        ensure(!ctx.probe.plaintext_answered().await?, "plaintext-answered")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&TLS_12, async |ctx: &mut Ctx| {
        ensure(!ctx.probe.tls12_accepted().await?, "tls12-accepted")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&TLS_H2, async |ctx: &mut Ctx| {
        ensure(!ctx.probe.h2_only_accepted().await?, "h2-only-accepted")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&TLS_NAME, async |ctx: &mut Ctx| {
        for name in ctx.settings.forbidden_names.clone() {
            ensure(
                ctx.probe.name_refused(&name).await?,
                "forbidden-name-verified",
            )?;
        }
        Ok(Note::default())
    })
    .await;
}

pub async fn auth(ctx: &mut Ctx) {
    ctx.case(&AUTH_ABSENT, async |ctx: &mut Ctx| {
        let reply = ctx.probe.exchange("GET", "/v1/machine", None, None).await?;
        expect(&reply, 401, Some(substrate_wire::AUTH_CREDENTIAL_ABSENT))?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&AUTH_INVALID, async |ctx: &mut Ctx| {
        let forged = forged_credential()?;
        let reply = ctx
            .probe
            .exchange("GET", "/v1/machine", Some(&forged), None)
            .await?;
        expect(&reply, 401, Some(substrate_wire::AUTH_AUTHORITY_INVALID))?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&AUTH_UNRECORDED, async |ctx: &mut Ctx| {
        let forged = forged_credential()?;
        let operation = ctx.operation_id();
        let body = Ctx::mutation(
            &operation,
            &json!({ "labels": { LABEL_KEY: ctx.settings.label }, "source": "empty" }),
        );
        let reply = ctx
            .probe
            .exchange("POST", "/v1/workspaces", Some(&forged), Some(&body))
            .await?;
        expect(&reply, 401, Some(substrate_wire::AUTH_AUTHORITY_INVALID))?;
        let record = ctx
            .raw("GET", &format!("/v1/ops/{operation}"), None)
            .await?;
        expect(&record, 404, Some("resource.not-found"))?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&AUTH_PROCESS, async |ctx: &mut Ctx| {
        let routes: Vec<(String, String)> = ctx
            .registry
            .operations()
            .filter(|operation| ctx.registry.scope(&operation.id).is_none())
            .filter(|operation| operation.id.starts_with("process."))
            .map(|operation| (operation.method.clone(), fill_path(&operation.path)))
            .collect();
        ensure(
            routes.len() == AUTH_PROCESS.operations.len(),
            "registry-differs",
        )?;
        for (method, path) in routes {
            let body = (method != "GET").then(|| Ctx::mutation(&ctx.operation_id(), &json!({})));
            let reply = ctx.raw(&method, &path, body.as_deref()).await?;
            expect(&reply, 403, Some(substrate_wire::AUTH_SCOPE_DENIED))?;
        }
        Ok(Note::default())
    })
    .await;
    ctx.case(&SCOPE_SESSIONS, async |ctx: &mut Ctx| {
        narrowed_denied(ctx, "/v1/sessions").await
    })
    .await;
    ctx.case(&SCOPE_WORKSPACES, async |ctx: &mut Ctx| {
        narrowed_denied(ctx, "/v1/workspaces/ws_conformance").await
    })
    .await;
}

/// A registry path with every `{placeholder}` replaced by a fixed segment.
fn fill_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "conformance"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

async fn narrowed_denied(ctx: &mut Ctx, path: &str) -> CaseResult {
    let token = match ctx.token(Purpose::Narrowed, Reason::Request).await {
        Ok(token) => token,
        Err(TokenError::Unsupported) => {
            return Ok(Note {
                absent: Some(Absent {
                    reason: AbsentReason::InputNotDeclared,
                    facts: Vec::new(),
                    inputs: vec!["narrowed-credential".to_owned()],
                    scopes: Vec::new(),
                }),
                ..Note::default()
            });
        }
        Err(TokenError::Failed) => return Err(Fail::new("token-unavailable")),
    };
    let reply = ctx.probe.exchange("GET", path, Some(&token), None).await?;
    drop(token);
    expect(&reply, 403, Some(substrate_wire::AUTH_SCOPE_DENIED))?;
    Ok(Note::default())
}

#[allow(
    clippy::too_many_lines,
    reason = "one case per inventory row, in run order"
)]
pub async fn workspace(ctx: &mut Ctx) {
    ctx.case(&WS_CREATE, async |ctx: &mut Ctx| {
        let operation = ctx.operation_id();
        let created = ctx
            .io(ctx
                .client
                .workspace()
                .empty()
                .label(LABEL_KEY, ctx.settings.label.clone())
                .operation_id(operation)
                .create())
            .await?;
        let id = created.id().to_owned();
        ctx.state.workspaces.push(id.clone());
        ensure(
            created.observation().state == WorkspaceState::Ready,
            "state-unexpected",
        )?;
        let read = ctx.workspace(&id).await?;
        ensure(
            read.observation().labels.get(LABEL_KEY) == Some(&ctx.settings.label),
            "label-missing",
        )?;
        ctx.state.workspace = Some(id);
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_WRITE, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        let observed = ctx.io(workspace.write_file(FILE, FIRST)).await?;
        ensure(
            observed.size == FIRST.len() as u64 && observed.sha256 == sha256_hex(FIRST),
            "file-observation-differs",
        )?;
        let read = ctx.io(workspace.read_file(FILE, 0, 4096)).await?;
        ensure(read.bytes == FIRST, "bytes-differ")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_READ_V1, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        let read = ctx.io(workspace.read_file(FILE, 0, 4096)).await?;
        ensure(
            read.bytes == FIRST && read.eof && read.offset == 0,
            "bytes-differ",
        )?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_READ_V2, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        let read = ctx.io(workspace.read_file_v2(FILE, 0, 4096)).await?;
        ensure(read.bytes == FIRST && read.eof, "bytes-differ")?;
        ensure(
            read.sha256 == sha256_hex(FIRST) && read.size == FIRST.len() as u64,
            "digest-differs",
        )?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_TREE, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        let tree = ctx.io(workspace.tree(100, false)).await?;
        ensure(
            tree.items
                .iter()
                .any(|entry| entry.path == FILE && entry.size == Some(FIRST.len() as u64)),
            "tree-entry-missing",
        )?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_REPLACE, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        let operation = ctx.operation_id();
        let expected = ExpectedFileState::Sha256 {
            sha256: sha256_hex(FIRST),
        };
        let replaced = ctx
            .io(workspace.replace_file(FILE, SECOND, expected, false, Some(operation)))
            .await?;
        ensure(
            replaced.after_sha256 == sha256_hex(SECOND),
            "digest-differs",
        )?;
        let read = ctx.io(workspace.read_file_v2(FILE, 0, 4096)).await?;
        ensure(read.bytes == SECOND, "bytes-differ")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_EDIT, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        let operation = ctx.operation_id();
        let edit = FileEditInput {
            expected_sha256: sha256_hex(SECOND),
            old_text: "second".to_owned(),
            new_text: "edited".to_owned(),
            match_policy: TextMatchPolicy::Exact,
        };
        let edited = ctx
            .io(workspace.edit_file(FILE, edit, Some(operation)))
            .await?;
        ensure(edited.after_sha256 == sha256_hex(EDITED), "digest-differs")?;
        let read = ctx.io(workspace.read_file_v2(FILE, 0, 4096)).await?;
        ensure(read.bytes == EDITED, "bytes-differ")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_DELETE, async |ctx: &mut Ctx| {
        let workspace = ctx.workspace(&need_workspace(ctx)?).await?;
        ensure(
            ctx.io(workspace.delete_file(FILE)).await?,
            "delete-not-applied",
        )?;
        match ctx.io_raw(workspace.read_file(FILE, 0, 4096)).await? {
            Err(error) if is_not_found(&error) => Ok(Note::default()),
            Err(error) => Err(Fail::new("unexpected-refusal").with(refusal_code(&error))),
            Ok(_) => Err(Fail::new("deleted-file-readable")),
        }
    })
    .await;
    ctx.case(&WS_OUT_OF_ROOT, async |ctx: &mut Ctx| {
        let workspace = need_workspace(ctx)?;
        let path = format!("/v1/workspaces/{workspace}/files/%2e%2e%2fetc%2fpasswd");
        let reply = ctx.raw("GET", &path, None).await?;
        expect(&reply, 422, Some("workspace.path-escape"))?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&WS_DESTROY, async |ctx: &mut Ctx| {
        let operation = ctx.operation_id();
        let created = ctx
            .io(ctx
                .client
                .workspace()
                .empty()
                .label(LABEL_KEY, ctx.settings.label.clone())
                .operation_id(operation)
                .create())
            .await?;
        let id = created.id().to_owned();
        ctx.state.workspaces.push(id.clone());
        ensure(ctx.io(created.destroy()).await?, "destroy-not-applied")?;
        match ctx.io_raw(ctx.client.get_workspace(&id)).await? {
            Err(error) if is_not_found(&error) => Ok(Note::default()),
            Err(error) => Err(Fail::new("unexpected-refusal").with(refusal_code(&error))),
            Ok(_) => Err(Fail::new("destroyed-workspace-readable")),
        }
    })
    .await;
}

/// The exec start input every exec, session and unserved request sizes from the facts.
pub fn exec_input(ctx: &Ctx, workspace: &str, argv: &[&str]) -> Value {
    let output_bytes = ctx
        .fact("exec.output-limit-bytes")
        .and_then(Value::as_u64)
        .map_or(65_536, |limit| limit.min(65_536));
    json!({
        "argv": argv,
        "env": { "allow": ["PATH"], "set": {} },
        "limits": {
            "cpu_millis": 10_000,
            "memory_bytes": 268_435_456,
            "output_bytes": output_bytes,
            "processes": 32,
            "timeout_ms": 120_000
        },
        "sandbox": {
            "capability_snapshot": ctx.snapshot,
            "network": "none",
            "profile": "workspace",
            "require": true
        },
        "wait": false,
        "workspace": workspace
    })
}

pub async fn unserved(ctx: &mut Ctx) {
    for spec in &UNSERVABLE {
        let operation = spec.operations[0];
        if ctx.registry.failing_facts(operation, &ctx.facts).is_empty() {
            continue;
        }
        ctx.case(spec, async |ctx: &mut Ctx| {
            unserved_case(ctx, operation).await
        })
        .await;
    }
}

/// One request a positive case would send, against an operation whose facts fail (§ 3, step 4).
async fn unserved_case(ctx: &mut Ctx, operation: &str) -> CaseResult {
    let workspace = ctx
        .state
        .workspace
        .clone()
        .unwrap_or_else(|| "ws_conformance".to_owned());
    let (method, path, input) = match operation {
        "workspace.create" => (
            "POST",
            "/v1/workspaces".to_owned(),
            Some(json!({ "labels": { LABEL_KEY: ctx.settings.label }, "source": "empty" })),
        ),
        "exec.start" => (
            "POST",
            "/v1/execs".to_owned(),
            Some(exec_input(
                ctx,
                &workspace,
                &[&ctx.settings.probe_shell, "-c", "exit 0"],
            )),
        ),
        "session.start" => {
            let mut exec = exec_input(ctx, &workspace, &[&ctx.settings.probe_shell, "-c", "cat"]);
            exec["lease_ttl_ms"] = json!(60_000);
            (
                "POST",
                "/v1/sessions".to_owned(),
                Some(json!({
                    "exec": exec,
                    "frame_limit_bytes": 4096,
                    "input_limit_bytes": 4096,
                    "queued_frames": 16
                })),
            )
        }
        "session.capabilities" => ("GET", "/v1/sessions".to_owned(), None),
        _ => ("GET", "/v1/events?limit=1".to_owned(), None),
    };
    let Some(input) = input else {
        let reply = ctx.raw(method, &path, None).await?;
        return unserved_answer(&reply);
    };
    let cursor = if ctx.fact_true("events.pull") {
        Some(ctx.head_cursor().await?.cursor)
    } else {
        None
    };
    let held: Vec<String> = ctx
        .state
        .workspaces
        .iter()
        .chain(&ctx.state.execs)
        .chain(&ctx.state.sessions)
        .cloned()
        .collect();
    let id = ctx.operation_id();
    let body = Ctx::mutation(&id, &input);
    let reply = ctx.raw(method, &path, Some(&body)).await?;
    let note = unserved_answer(&reply)?;
    let record = ctx.raw("GET", &format!("/v1/ops/{id}"), None).await?;
    if record.status == 404 {
        expect(&record, 404, Some("resource.not-found"))?;
    } else {
        let record = success(&record)?;
        ensure(
            record["state"] == "refused" && record["resource"].is_null(),
            "operation-record-not-refused",
        )?;
    }
    let again = ctx.raw(method, &path, Some(&body)).await?;
    ensure(
        again.status == reply.status && again.error_code() == reply.error_code(),
        "replay-differs",
    )?;
    if let Some(cursor) = cursor {
        for event in ctx.events_after(&cursor).await? {
            if event["resource_kind"] != "operation" {
                let resource = event["resource"].as_str().unwrap_or_default();
                ensure(held.iter().any(|held| held == resource), "resource-created")?;
            }
        }
    }
    Ok(note)
}

fn unserved_answer(reply: &Reply) -> CaseResult {
    ensure(reply.contract_pinned(), "contract-header-missing")?;
    let error = reply
        .error()
        .ok_or_else(|| Fail::new("error-envelope-undecodable"))?;
    ensure(
        error.class == substrate_wire::ErrorClass::Unserved,
        "class-not-unserved",
    )
    .map_err(|fail| fail.with(Some(&error.code)))?;
    let code = if crate::run::code_ok(&error.code) {
        error.code
    } else {
        "code-unrepresentable".to_owned()
    };
    Ok(Note {
        observed: Some(Observed {
            status: reply.status,
            code,
        }),
        ..Note::default()
    })
}

/// Events after `cursor` whose cause is `operation` and that name a resource other than the
/// operation itself.
pub fn effect_events<'a>(events: &'a [Value], operation: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["cause"]["operation"] == operation)
        .filter(|event| event["resource_kind"] != "operation")
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "the three recovery cases share one workspace"
)]
pub async fn recovery(ctx: &mut Ctx) {
    ctx.case(&RECOVERY_DISCARDED, async |ctx: &mut Ctx| {
        let workspace = need_workspace(ctx)?;
        let cursor = if ctx.fact_true("events.pull") {
            Some(ctx.head_cursor().await?.cursor)
        } else {
            None
        };
        let operation = ctx.operation_id();
        let path = format!("/v1/workspaces/{workspace}/files/conformance-r1.txt");
        let bytes = b"conformance discarded\n";
        let body = Ctx::mutation(&operation, &write_input(bytes));
        let original = success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        let record = ctx.io(ctx.client.operation(&operation)).await?;
        ensure(
            record.state == OperationState::Terminal && record.result.as_ref() == Some(&original),
            "operation-record-differs",
        )?;
        let replayed = success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        ensure(replayed == original, "replay-differs")?;
        let handle = ctx.workspace(&workspace).await?;
        let read = ctx
            .io(handle.read_file_v2("conformance-r1.txt", 0, 4096))
            .await?;
        ensure(
            read.bytes == bytes && Some(read.sha256.as_str()) == original["sha256"].as_str(),
            "bytes-differ",
        )?;
        if let Some(cursor) = cursor {
            let events = ctx.events_after(&cursor).await?;
            ensure(
                effect_events(&events, &operation).len() == 1,
                "write-events-not-one",
            )?;
        }
        Ok(Note::default())
    })
    .await;
    ctx.case(&RECOVERY_CONFLICT, async |ctx: &mut Ctx| {
        let workspace = need_workspace(ctx)?;
        let operation = ctx.operation_id();
        let path = format!("/v1/workspaces/{workspace}/files/conformance-r2.txt");
        let body = Ctx::mutation(&operation, &write_input(b"conformance one\n"));
        success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        let other = Ctx::mutation(&operation, &write_input(b"conformance two\n"));
        let reply = ctx.raw("PUT", &path, Some(&other)).await?;
        expect(&reply, 409, Some("operation.request-conflict"))?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&RECOVERY_DROPPED, async |ctx: &mut Ctx| dropped(ctx).await)
        .await;
}

/// The request is flushed and its connection closed before any byte of the response is read.
async fn dropped(ctx: &mut Ctx) -> CaseResult {
    let workspace = need_workspace(ctx)?;
    let cursor = if ctx.fact_true("events.pull") {
        Some(ctx.head_cursor().await?.cursor)
    } else {
        None
    };
    let operation = ctx.operation_id();
    let path = format!("/v1/workspaces/{workspace}/files/conformance-r3.txt");
    let bytes = b"conformance dropped\n";
    let body = Ctx::mutation(&operation, &write_input(bytes));
    let token = ctx
        .token(Purpose::Primary, Reason::Request)
        .await
        .map_err(|_| Fail::new("token-unavailable"))?;
    ctx.probe
        .send_and_drop("PUT", &path, Some(&token), Some(&body))
        .await?;
    drop(token);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut record = ctx
        .raw("GET", &format!("/v1/ops/{operation}"), None)
        .await?;
    let polls = tokio::time::Instant::now() + ctx.settings.io;
    while record.status == 200
        && record
            .result()
            .is_some_and(|result| result["state"] == "accepted")
        && tokio::time::Instant::now() < polls
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
        record = ctx
            .raw("GET", &format!("/v1/ops/{operation}"), None)
            .await?;
    }
    let branch = if record.status == 404 {
        expect(&record, 404, Some("resource.not-found"))?;
        let first = success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        let second = success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        ensure(first == second, "replay-differs")?;
        "not-admitted"
    } else {
        let record = success(&record)?;
        ensure(record["state"] == "terminal", "operation-not-terminal")?;
        let outcome = &record["outcome"];
        ensure(outcome["kind"] == "success", "operation-not-successful")?;
        let replayed = success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        ensure(replayed == outcome["result"], "replay-differs")?;
        "admitted"
    };
    let handle = ctx.workspace(&workspace).await?;
    let read = ctx
        .io(handle.read_file_v2("conformance-r3.txt", 0, 4096))
        .await?;
    ensure(read.bytes == bytes, "bytes-differ")?;
    if let Some(cursor) = cursor {
        let events = ctx.events_after(&cursor).await?;
        ensure(
            effect_events(&events, &operation).len() == 1,
            "write-events-not-one",
        )?;
    }
    Ok(Note {
        branch: Some(branch),
        ..Note::default()
    })
}

pub async fn events(ctx: &mut Ctx) {
    ctx.case(&EVENTS_ORDER, async |ctx: &mut Ctx| commit_order(ctx).await)
        .await;
    ctx.case(&EVENTS_VISIBLE, async |ctx: &mut Ctx| {
        visible_after_state(ctx).await
    })
    .await;
    ctx.case(&EVENTS_STREAM, async |ctx: &mut Ctx| {
        stream_equals_pull(ctx).await
    })
    .await;
    ctx.case(&RETENTION_GAP, async |ctx: &mut Ctx| {
        retention_gap(ctx).await
    })
    .await;
}

async fn commit_order(ctx: &mut Ctx) -> CaseResult {
    let cursor = ctx.head_cursor().await?.cursor;
    let create = ctx.operation_id();
    let created = ctx
        .io(ctx
            .client
            .workspace()
            .empty()
            .label(LABEL_KEY, ctx.settings.label.clone())
            .operation_id(create.clone())
            .create())
        .await?;
    ctx.state.workspaces.push(created.id().to_owned());
    let write = ctx.operation_id();
    ctx.io(created.write_file_with_operation_id(
        "conformance-order.txt",
        b"order\n",
        Some(write.clone()),
    ))
    .await?;
    let destroy = ctx.operation_id();
    ctx.io(created.destroy_with_operation_id(Some(destroy.clone())))
        .await?;
    let events = ctx.events_after(&cursor).await?;
    let sequences: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().unwrap_or(0))
        .collect();
    ensure(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "sequence-not-increasing",
    )?;
    let mut positions = Vec::new();
    for operation in [&create, &write, &destroy] {
        let position = events
            .iter()
            .position(|event| event["cause"]["operation"] == operation.as_str())
            .ok_or_else(|| Fail::new("own-event-missing"))?;
        positions.push(position);
    }
    ensure(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "commit-order-differs",
    )?;
    Ok(Note::default())
}

async fn visible_after_state(ctx: &mut Ctx) -> CaseResult {
    let cursor = ctx.head_cursor().await?.cursor;
    let operation = ctx.operation_id();
    let client = ctx.client.clone();
    let label = ctx.settings.label.clone();
    let create_operation = operation.clone();
    let create = tokio::spawn(async move {
        client
            .workspace()
            .empty()
            .label(LABEL_KEY, label)
            .operation_id(create_operation)
            .create()
            .await
            .map(|workspace| workspace.id().to_owned())
    });
    let until = tokio::time::Instant::now() + ctx.settings.io;
    let mut checked = 0;
    let mut seen_effect = false;
    while !seen_effect && tokio::time::Instant::now() < until {
        let events = ctx.events_after(&cursor).await?;
        for event in &events[checked..] {
            let resource = event["resource"].as_str().unwrap_or_default();
            let read = match event["resource_kind"].as_str() {
                Some("operation") => ctx.raw("GET", &format!("/v1/ops/{resource}"), None).await?,
                Some("workspace") => {
                    ctx.raw("GET", &format!("/v1/workspaces/{resource}"), None)
                        .await?
                }
                _ => continue,
            };
            if event["cause"]["operation"] == operation.as_str() {
                ensure(read.status == 200, "event-before-state")?;
                seen_effect |= event["resource_kind"] == "workspace";
            }
        }
        checked = events.len();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let created = tokio::time::timeout(ctx.settings.io, create)
        .await
        .map_err(|_| Fail::new("io-timeout"))?
        .map_err(|_| Fail::new("task-failed"))?
        .map_err(|error| crate::run::sdk(&error))?;
    ctx.state.workspaces.push(created);
    ensure(seen_effect, "own-event-missing")?;
    Ok(Note::default())
}

async fn stream_equals_pull(ctx: &mut Ctx) -> CaseResult {
    let workspace = need_workspace(ctx)?;
    let cursor = ctx.head_cursor().await?.cursor;
    let handle = ctx.workspace(&workspace).await?;
    ctx.io(handle.write_file("conformance-stream.txt", b"stream\n"))
        .await?;
    let pulled: Vec<(u64, String)> = ctx
        .events_after(&cursor)
        .await?
        .iter()
        .map(|event| {
            (
                event["seq"].as_u64().unwrap_or(0),
                event["transition"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    ensure(!pulled.is_empty(), "own-event-missing")?;
    let mut stream = ctx.io(ctx.client.event_stream(Some(&cursor), 1000)).await?;
    let mut streamed = Vec::new();
    while streamed.len() < pulled.len() {
        let page = ctx
            .io(stream.next_page())
            .await?
            .ok_or_else(|| Fail::new("stream-ended"))?;
        streamed.extend(
            page.events
                .into_iter()
                .map(|event| (event.sequence, event.transition)),
        );
    }
    streamed.truncate(pulled.len());
    ensure(streamed == pulled, "stream-differs")?;
    Ok(Note::default())
}

async fn retention_gap(ctx: &mut Ctx) -> CaseResult {
    let retention = ctx
        .fact("events.retention-events")
        .and_then(Value::as_u64)
        .ok_or_else(|| Fail::new("fact-undecodable"))?;
    if retention > ctx.settings.max_generated_events {
        return Ok(Note {
            absent: Some(Absent {
                reason: AbsentReason::BoundExceeded,
                facts: vec!["events.retention-events".to_owned()],
                inputs: vec!["max-generated-events".to_owned()],
                scopes: Vec::new(),
            }),
            ..Note::default()
        });
    }
    let workspace = need_workspace(ctx)?;
    let cursor = ctx.head_cursor().await?.cursor;
    let handle = ctx.workspace(&workspace).await?;
    for index in 0..=retention {
        ctx.io(handle.write_file("conformance-gap.txt", index.to_string()))
            .await?;
    }
    match ctx.events(Some(&cursor), 1).await? {
        Err(code) if code == "event.retention-gap" => Ok(Note::default()),
        Err(code) => Err(Fail::new("unexpected-code").with(Some(&code))),
        Ok(_) => Err(Fail::new("gap-not-refused")),
    }
}
