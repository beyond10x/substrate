//! The groups that need exec facts or declared inputs (design 31 § 3 and § 4): exec, resources,
//! sessions, restart, ops, and the closing cleanup.

use std::process::Stdio;
use std::time::Duration;

use b10x_substrate_sdk::{
    BaselineEnvironment, Exec, ExecMeasurement, ExecState, ExecUsage, ExecutionPolicy,
    MetricsObservation, MetricsResourceKind, OutputStream, PipeChannel, PipeFrame, PtyWindow,
    Signal,
};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::cases::{decode_machine, exec_input, spec, success, write_input};
use crate::probe::plain_get;
use crate::run::{CaseResult, Ctx, Fail, Head, LABEL_KEY, Note, Spec, ensure, refusal_code};
use crate::token::{Purpose, Reason};

const EXEC_RUN: Spec = spec(
    "exec.run-output",
    "exec",
    &["exec.start", "exec.get", "exec.output.get"],
    &[],
    &[],
);
const EXEC_SIGNAL: Spec = spec(
    "exec.signal",
    "exec",
    &["exec.start", "exec.signal", "exec.get"],
    &[],
    &[],
);
const EXEC_RETIRE: Spec = spec(
    "exec.retire",
    "exec",
    &["exec.start", "exec.retire", "exec.get"],
    &[],
    &[],
);
const EXEC_EGRESS: Spec = spec(
    "exec.no-egress",
    "exec",
    &["exec.start", "exec.get", "exec.output.get"],
    &[],
    &[],
);
const USAGE: Spec = spec(
    "resources.usage-observed",
    "resources",
    &["exec.start", "exec.get"],
    &["exec.resource-usage"],
    &[],
);
const METRICS: Spec = spec(
    "resources.metrics-names-exec",
    "resources",
    &["exec.start", "metrics.get"],
    &["exec.resource-usage"],
    &[],
);
const PIPE: Spec = spec(
    "sessions.pipe-attach",
    "sessions",
    &[
        "session.start",
        "session.authority.mint",
        "session.attach",
        "session.retire",
    ],
    &[],
    &[],
);
const SECOND_ATTACH: Spec = spec(
    "sessions.second-attach-refused",
    "sessions",
    &[
        "session.start",
        "session.authority.mint",
        "session.attach",
        "session.retire",
    ],
    &[],
    &[],
);
const PTY: Spec = spec(
    "sessions.pty-resize",
    "sessions",
    &[
        "session.start",
        "session.authority.mint",
        "session.attach",
        "session.retire",
    ],
    &["sessions.pty"],
    &[],
);
const RESTART_DETECTED: Spec = spec(
    "restart.detected",
    "restart",
    &["machine.get", "workspace.create", "workspace.file.write"],
    &[],
    &["restart-hook"],
);
const RESTART_WORKSPACE: Spec = spec(
    "restart.workspace-readable",
    "restart",
    &["workspace.file.read-v2"],
    &[],
    &["restart-hook"],
);
const RESTART_REPLAY: Spec = spec(
    "restart.replay",
    "restart",
    &["workspace.file.write"],
    &[],
    &["restart-hook"],
);
const RESTART_CURSOR: Spec = spec(
    "restart.cursor",
    "restart",
    &["event.list"],
    &[],
    &["restart-hook"],
);
const RESTART_EXEC_UNKNOWN: Spec = spec(
    "restart.exec-unknown",
    "restart-exec",
    &["exec.start", "exec.get", "operation.get"],
    &[],
    &["restart-hook"],
);
const RESTART_EXEC_ONCE: Spec = spec(
    "restart.exec-no-redispatch",
    "restart-exec",
    &["exec.start", "workspace.file.read-v2"],
    &[],
    &["restart-hook"],
);
const OPS_LIVEZ: Spec = spec("ops.livez", "ops", &[], &[], &["ops-origin"]);
const OPS_READYZ: Spec = spec("ops.readyz", "ops", &[], &[], &["ops-origin"]);
const OPS_METRICS: Spec = spec("ops.metrics-clean", "ops", &[], &[], &["ops-origin"]);
const CLEANUP: Spec = spec(
    "cleanup.absent",
    "cleanup",
    &["workspace.destroy", "workspace.get"],
    &[],
    &[],
);

/// A script that answers one input line and exits, using shell builtins only.
const ECHO_ONE: &str = "read -r line; printf 'got %s\\n' \"$line\"";
const MARKER: &str = "conformance-marker.txt";

fn policy() -> Result<ExecutionPolicy, Fail> {
    ExecutionPolicy::builder()
        .timeout(Duration::from_mins(2))
        .cpu_time(Duration::from_secs(10))
        .memory_bytes(268_435_456)
        .processes(32)
        .output_bytes(65_536)
        .build()
        .map_err(|_| Fail::new("policy-invalid"))
}

fn workspace_id(ctx: &Ctx) -> Result<String, Fail> {
    ctx.state
        .workspace
        .clone()
        .ok_or_else(|| Fail::new("workspace-unavailable"))
}

/// Starts `-c script` under the probe shell in the shared workspace.
async fn start(ctx: &mut Ctx, script: &str, measure: bool) -> Result<Exec, Fail> {
    let workspace = ctx.workspace(&workspace_id(ctx)?).await?;
    let operation = ctx.operation_id();
    let mut command = workspace
        .command(ctx.settings.probe_shell.clone())
        .args(["-c", script])
        .allow_environment(BaselineEnvironment::Path)
        .policy(policy()?)
        .operation_id(operation);
    if measure {
        command = command.measure(ExecMeasurement::ResourceUsage);
    }
    let exec = ctx.io(command.start()).await?;
    ctx.state.execs.push(exec.id().to_owned());
    Ok(exec)
}

/// Refreshes until `done` holds; each read bounded by `--io-timeout`, the wait by the deadline.
async fn until_exec(
    ctx: &Ctx,
    exec: &mut Exec,
    done: impl Fn(&b10x_substrate_sdk::ExecObservation) -> bool,
) -> Result<(), Fail> {
    loop {
        ctx.io(exec.refresh()).await?;
        if done(exec.observation()) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn stdout(ctx: &Ctx, exec: &Exec) -> Result<Vec<u8>, Fail> {
    let page = ctx
        .io(exec.output_page(OutputStream::Stdout, 0, 65_536))
        .await?;
    Ok(page.bytes)
}

fn exited_zero(exec: &Exec) -> bool {
    let observed = exec.observation();
    observed.state == ExecState::Exited
        && observed.exit.as_ref().and_then(|exit| exit.code) == Some(0)
}

#[allow(
    clippy::too_many_lines,
    reason = "one case per inventory row, in run order"
)]
pub async fn exec(ctx: &mut Ctx) {
    let mut finished: Option<String> = None;
    ctx.case(&EXEC_RUN, async |ctx: &mut Ctx| {
        let mut exec = start(ctx, "printf conformance-out", false).await?;
        until_exec(ctx, &mut exec, |observed| observed.state.terminal()).await?;
        ensure(exited_zero(&exec), "exit-unexpected")?;
        ensure(
            stdout(ctx, &exec).await? == b"conformance-out",
            "output-differs",
        )?;
        finished = Some(exec.id().to_owned());
        Ok(Note::default())
    })
    .await;
    ctx.case(&EXEC_SIGNAL, async |ctx: &mut Ctx| {
        let mut exec = start(ctx, "exec sleep 300", false).await?;
        until_exec(ctx, &mut exec, |observed| {
            observed.state != ExecState::Accepted
        })
        .await?;
        ensure(
            exec.observation().state == ExecState::Running,
            "exec-not-running",
        )?;
        ctx.io(exec.signal(Signal::Terminate, Duration::from_secs(2)))
            .await?;
        until_exec(ctx, &mut exec, |observed| observed.state.terminal()).await?;
        ensure(
            exec.observation()
                .exit
                .as_ref()
                .is_some_and(|exit| exit.signal.is_some()),
            "exit-unexpected",
        )?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&EXEC_RETIRE, async |ctx: &mut Ctx| {
        let id = finished
            .clone()
            .ok_or_else(|| Fail::new("exec-unavailable"))?;
        let exec = ctx.io(ctx.client.get_exec(&id)).await?;
        ensure(ctx.io(exec.retire()).await?, "retire-not-applied")?;
        match ctx.io_raw(ctx.client.get_exec(&id)).await? {
            Err(error) if refusal_code(&error) == Some("resource.not-found") => Ok(Note::default()),
            Err(error) => Err(Fail::new("unexpected-refusal").with(refusal_code(&error))),
            Ok(_) => Err(Fail::new("retired-exec-readable")),
        }
    })
    .await;
    ctx.case(&EXEC_EGRESS, async |ctx: &mut Ctx| {
        // Routes, read with builtins only: an outbound connect needs a route that is not on `lo`.
        let script = "while read -r l; do printf 'r4 %s\\n' \"$l\"; done < /proc/net/route; \
                      while read -r l; do printf 'r6 %s\\n' \"$l\"; done < /proc/net/ipv6_route; \
                      printf 'end\\n'";
        let mut exec = start(ctx, script, false).await?;
        until_exec(ctx, &mut exec, |observed| observed.state.terminal()).await?;
        let output = stdout(ctx, &exec).await?;
        let output = String::from_utf8(output).map_err(|_| Fail::new("output-undecodable"))?;
        ensure(output.ends_with("end\n"), "routes-unreadable")?;
        ensure(egress_routes(&output) == 0, "egress-route-present")?;
        Ok(Note::default())
    })
    .await;
}

/// Routes a connect could leave by: IPv4 rows past the header, IPv6 rows not on `lo`.
pub fn egress_routes(output: &str) -> usize {
    let ipv4 = output
        .lines()
        .filter_map(|line| line.strip_prefix("r4 "))
        .filter(|row| !row.starts_with("Iface"))
        .filter(|row| row.split_whitespace().next() != Some("lo"))
        .count();
    let ipv6 = output
        .lines()
        .filter_map(|line| line.strip_prefix("r6 "))
        .filter(|row| row.split_whitespace().last() != Some("lo"))
        .count();
    ipv4 + ipv6
}

pub async fn resources(ctx: &mut Ctx) {
    let mut measured: Option<String> = None;
    ctx.case(&USAGE, async |ctx: &mut Ctx| {
        let mut exec = start(ctx, "printf measured", true).await?;
        until_exec(ctx, &mut exec, |observed| {
            observed.state.terminal()
                && !matches!(observed.usage, None | Some(ExecUsage::Pending { .. }))
        })
        .await?;
        ensure(
            matches!(exec.observation().usage, Some(ExecUsage::Observed(_))),
            "usage-not-observed",
        )?;
        measured = Some(exec.id().to_owned());
        Ok(Note::default())
    })
    .await;
    ctx.case(&METRICS, async |ctx: &mut Ctx| {
        let id = measured
            .clone()
            .ok_or_else(|| Fail::new("exec-unavailable"))?;
        match ctx
            .io(ctx.client.metrics(MetricsResourceKind::Exec, &id))
            .await?
        {
            MetricsObservation::Exec { exec, .. } if exec == id => Ok(Note::default()),
            _ => Err(Fail::new("metrics-name-another-resource")),
        }
    })
    .await;
}

async fn session(
    ctx: &mut Ctx,
    window: Option<PtyWindow>,
) -> Result<b10x_substrate_sdk::PipeSession, Fail> {
    let workspace = ctx.workspace(&workspace_id(ctx)?).await?;
    let operation = ctx.operation_id();
    let builder = match window {
        Some(window) => workspace.pty_session(ctx.settings.probe_shell.clone(), window),
        None => workspace.pipe_session(ctx.settings.probe_shell.clone()),
    };
    let session = ctx
        .io(builder
            .args(["-c", ECHO_ONE])
            .allow_environment(BaselineEnvironment::Path)
            .policy(policy()?)
            .lease(Duration::from_mins(2))
            .input_limit_bytes(4096)
            .frame_limit_bytes(4096)
            .queued_frames(16)
            .operation_id(operation)
            .start())
        .await?;
    ctx.state.sessions.push(session.id().to_owned());
    Ok(session)
}

/// The most session stdout a case reads: the expected answer is one short line.
const MAX_SESSION_OUTPUT: usize = 64 * 1024;

/// Appends one output frame, failing the case once the output passes [`MAX_SESSION_OUTPUT`].
fn append_bounded(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Fail> {
    ensure(
        output.len().saturating_add(bytes.len()) <= MAX_SESSION_OUTPUT,
        "session-output-exceeded",
    )?;
    output.extend_from_slice(bytes);
    Ok(())
}

/// Writes one line and reads frames until the exit frame: the stdout bytes and whether the exit
/// was a clean zero.
async fn answer(ctx: &Ctx, channel: &mut PipeChannel) -> Result<(Vec<u8>, bool), Fail> {
    ctx.io(channel.write(b"conformance\n")).await?;
    let mut output = Vec::new();
    loop {
        match ctx.io(channel.next_frame()).await? {
            Some(PipeFrame::Output {
                stream: OutputStream::Stdout,
                bytes,
                ..
            }) => append_bounded(&mut output, &bytes)?,
            Some(PipeFrame::Exit { state, exit, .. }) => {
                let zero = state == ExecState::Exited && exit.and_then(|exit| exit.code) == Some(0);
                return Ok((output, zero));
            }
            Some(PipeFrame::ProtocolError { code, .. }) => {
                return Err(Fail::new("session-protocol-error").with(Some(&code)));
            }
            Some(_) => {}
            None => return Err(Fail::new("session-closed")),
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub async fn sessions(ctx: &mut Ctx) {
    ctx.case(&PIPE, async |ctx: &mut Ctx| {
        let session = session(ctx, None).await?;
        let mut channel = ctx.io(session.attach()).await?;
        let (output, zero) = answer(ctx, &mut channel).await?;
        ensure(
            output == b"got conformance\n" && zero,
            "session-output-differs",
        )?;
        ensure(ctx.io(session.retire()).await?, "retire-not-applied")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&SECOND_ATTACH, async |ctx: &mut Ctx| {
        let session = session(ctx, None).await?;
        let mut channel = ctx.io(session.attach()).await?;
        match ctx.io_raw(session.attach()).await? {
            Err(error) if refusal_code(&error) == Some("session.not-attachable") => {}
            Err(error) => return Err(Fail::new("unexpected-refusal").with(refusal_code(&error))),
            Ok(_) => return Err(Fail::new("second-attach-admitted")),
        }
        let (output, zero) = answer(ctx, &mut channel).await?;
        ensure(
            output == b"got conformance\n" && zero,
            "session-output-differs",
        )?;
        ensure(ctx.io(session.retire()).await?, "retire-not-applied")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&PTY, async |ctx: &mut Ctx| {
        let session = session(
            ctx,
            Some(PtyWindow {
                columns: 80,
                rows: 24,
            }),
        )
        .await?;
        let mut channel = ctx.io(session.attach()).await?;
        ctx.io(channel.resize(PtyWindow {
            columns: 100,
            rows: 30,
        }))
        .await?;
        let (output, zero) = answer(ctx, &mut channel).await?;
        ensure(
            contains(&output, b"got conformance") && zero,
            "session-output-differs",
        )?;
        ensure(ctx.io(session.retire()).await?, "retire-not-applied")?;
        Ok(Note::default())
    })
    .await;
}

/// What the restart sequence left for the cases that check it.
pub struct Restarted {
    workspace: String,
    write_path: String,
    write_body: Vec<u8>,
    write_result: Value,
    written: Vec<u8>,
    head: Option<Head>,
    exec: Option<RestartExec>,
}

struct RestartExec {
    id: String,
    operation: String,
    body: Vec<u8>,
    result: Value,
}

pub async fn restart(ctx: &mut Ctx) {
    let detected = ctx
        .case(&RESTART_DETECTED, async |ctx: &mut Ctx| sequence(ctx).await)
        .await;
    let dependents = [
        &RESTART_WORKSPACE,
        &RESTART_REPLAY,
        &RESTART_CURSOR,
        &RESTART_EXEC_UNKNOWN,
        &RESTART_EXEC_ONCE,
    ];
    if !detected || ctx.state.restart.is_none() {
        for spec in dependents {
            ctx.record_failed(spec, "restart-not-detected");
        }
        return;
    }
    restart_state(ctx).await;
    restart_execs(ctx).await;
}

/// The workspace, replay and cursor cases after a detected restart.
async fn restart_state(ctx: &mut Ctx) {
    ctx.case(&RESTART_WORKSPACE, async |ctx: &mut Ctx| {
        let restarted = ctx
            .state
            .restart
            .as_ref()
            .ok_or_else(|| Fail::new("restart-not-detected"))?;
        let (workspace, written) = (restarted.workspace.clone(), restarted.written.clone());
        let handle = ctx.workspace(&workspace).await?;
        let read = ctx
            .io(handle.read_file_v2("conformance-restart.txt", 0, 4096))
            .await?;
        ensure(read.bytes == written, "bytes-differ")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&RESTART_REPLAY, async |ctx: &mut Ctx| {
        let restarted = ctx
            .state
            .restart
            .as_ref()
            .ok_or_else(|| Fail::new("restart-not-detected"))?;
        let (path, body, original) = (
            restarted.write_path.clone(),
            restarted.write_body.clone(),
            restarted.write_result.clone(),
        );
        let replayed = success(&ctx.raw("PUT", &path, Some(&body)).await?)?;
        ensure(replayed == original, "replay-differs")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&RESTART_CURSOR, async |ctx: &mut Ctx| {
        let restarted = ctx
            .state
            .restart
            .as_ref()
            .ok_or_else(|| Fail::new("restart-not-detected"))?;
        let Some(head) = restarted
            .head
            .as_ref()
            .map(|head| (head.cursor.clone(), head.generation, head.through_seq))
        else {
            return Err(Fail::new("cursor-unavailable"));
        };
        cursor_continuous(ctx, &head.0, head.1, head.2).await
    })
    .await;
}

/// The exec cases after a detected restart: both failed when the sequence started no exec.
async fn restart_execs(ctx: &mut Ctx) {
    if ctx
        .state
        .restart
        .as_ref()
        .is_some_and(|restarted| restarted.exec.is_none())
    {
        for spec in [&RESTART_EXEC_UNKNOWN, &RESTART_EXEC_ONCE] {
            ctx.record_failed(spec, "exec-not-started");
        }
        return;
    }
    ctx.case(&RESTART_EXEC_UNKNOWN, async |ctx: &mut Ctx| {
        let exec = restart_exec(ctx)?;
        let read = success(
            &ctx.raw("GET", &format!("/v1/execs/{}", exec.0), None)
                .await?,
        )?;
        ensure(read["state"] == "unknown", "exec-not-unknown")?;
        let record = success(&ctx.raw("GET", &format!("/v1/ops/{}", exec.1), None).await?)?;
        ensure(record["state"] == "terminal", "operation-not-terminal")?;
        Ok(Note::default())
    })
    .await;
    ctx.case(&RESTART_EXEC_ONCE, async |ctx: &mut Ctx| {
        let exec = restart_exec(ctx)?;
        let workspace = ctx
            .state
            .restart
            .as_ref()
            .map(|restarted| restarted.workspace.clone())
            .ok_or_else(|| Fail::new("restart-not-detected"))?;
        ensure(markers(ctx, &workspace).await? == 1, "marker-count-differs")?;
        let replayed = success(&ctx.raw("POST", "/v1/execs", Some(&exec.2)).await?)?;
        ensure(replayed == exec.3, "replay-differs")?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        ensure(markers(ctx, &workspace).await? == 1, "marker-count-differs")?;
        Ok(Note::default())
    })
    .await;
}

fn restart_exec(ctx: &Ctx) -> Result<(String, String, Vec<u8>, Value), Fail> {
    ctx.state
        .restart
        .as_ref()
        .and_then(|restarted| restarted.exec.as_ref())
        .map(|exec| {
            (
                exec.id.clone(),
                exec.operation.clone(),
                exec.body.clone(),
                exec.result.clone(),
            )
        })
        .ok_or_else(|| Fail::new("exec-not-started"))
}

/// Marker lines in the restart workspace; zero while the file does not exist.
async fn markers(ctx: &Ctx, workspace: &str) -> Result<usize, Fail> {
    let path =
        format!("/v2/workspaces/{workspace}/files/{MARKER}?mode=file&offset=0&limit_bytes=4096");
    let reply = ctx.raw("GET", &path, None).await?;
    if reply.status == 404 {
        return Ok(0);
    }
    let result = success(&reply)?;
    let data = result["content"]["data"].as_str().unwrap_or_default();
    let bytes = base64_decode(data)?;
    Ok(bytes.split(|byte| *byte == b'\n').count() - 1)
}

fn base64_decode(data: &str) -> Result<Vec<u8>, Fail> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| Fail::new("envelope-undecodable"))
}

async fn cursor_continuous(ctx: &Ctx, cursor: &str, generation: u64, through: u64) -> CaseResult {
    match ctx.events(Some(cursor), 1000).await? {
        Ok(page) => {
            let first = page["items"].as_array().and_then(|items| items.first());
            let continuous = page["generation"].as_u64() == Some(generation)
                && first.is_none_or(|event| {
                    event["generation"].as_u64() == Some(generation)
                        && event["seq"].as_u64() == Some(through + 1)
                });
            ensure(continuous, "sequence-jumped")?;
            Ok(Note::default())
        }
        Err(code) if code == "event.source-mismatch" || code == "event.retention-gap" => {
            Ok(Note::default())
        }
        Err(code) => Err(Fail::new("unexpected-code").with(Some(&code))),
    }
}

/// The wire `probed_at` of one machine read, or `None` while the endpoint does not answer.
async fn probed_at(
    ctx: &Ctx,
) -> Option<(
    chrono::DateTime<chrono::Utc>,
    substrate_wire::CapabilitySnapshot,
)> {
    let reply = ctx.raw("GET", "/v1/machine", None).await.ok()?;
    if reply.status != 200 || !reply.contract_pinned() {
        return None;
    }
    let snapshot = decode_machine(&reply)?;
    Some((snapshot.probed_at, snapshot))
}

#[allow(
    clippy::too_many_lines,
    reason = "the restart sequence is one ordered procedure"
)]
async fn sequence(ctx: &mut Ctx) -> CaseResult {
    let hook = ctx
        .settings
        .hook
        .as_ref()
        .ok_or_else(|| Fail::new("hook-undeclared"))?;
    let (program, arguments, environment) = (
        hook.program.clone(),
        hook.arguments.clone(),
        hook.environment.clone(),
    );
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
    let workspace = created.id().to_owned();
    ctx.state.workspaces.push(workspace.clone());
    let written = b"conformance restart\n".to_vec();
    let write_operation = ctx.operation_id();
    let write_path = format!("/v1/workspaces/{workspace}/files/conformance-restart.txt");
    let write_body = Ctx::mutation(&write_operation, &write_input(&written));
    let write_result = success(&ctx.raw("PUT", &write_path, Some(&write_body)).await?)?;
    let exec = if ctx.would_run(&RESTART_EXEC_UNKNOWN) {
        let script = format!("printf 'm\\n' >> /workspace/{MARKER}; exec sleep 300");
        let operation = ctx.operation_id();
        let input = exec_input(ctx, &workspace, &[&ctx.settings.probe_shell, "-c", &script]);
        let body = Ctx::mutation(&operation, &input);
        let result = success(&ctx.raw("POST", "/v1/execs", Some(&body)).await?)?;
        let id = result["id"]
            .as_str()
            .ok_or_else(|| Fail::new("envelope-undecodable"))?
            .to_owned();
        ctx.state.execs.push(id.clone());
        let until = Instant::now() + ctx.settings.restart_timeout;
        while markers(ctx, &workspace).await? == 0 {
            ensure(Instant::now() < until, "marker-not-written")?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Some(RestartExec {
            id,
            operation,
            body,
            result,
        })
    } else {
        None
    };
    let head = if ctx.fact_true("events.pull") {
        Some(ctx.head_cursor().await?)
    } else {
        None
    };
    let (before, _) = probed_at(ctx)
        .await
        .ok_or_else(|| Fail::new("machine-unreadable"))?;
    let until = Instant::now() + ctx.settings.restart_timeout;
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&arguments)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::harden::standard_streams_only(&mut command);
    let mut child = command.spawn().map_err(|_| Fail::new("hook-failed"))?;
    match tokio::time::timeout_at(until, child.wait()).await {
        Ok(Ok(status)) if status.success() => {}
        _ => return Err(Fail::new("hook-failed")),
    }
    let snapshot = loop {
        if let Some((after, snapshot)) = probed_at(ctx).await
            && after > before
        {
            break snapshot;
        }
        ensure(Instant::now() < until, "restart-not-observed")?;
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    if let Some(ops) = ctx.settings.ops_authority.clone() {
        loop {
            if plain_get(&ops, "/readyz", ctx.settings.io)
                .await
                .is_ok_and(|reply| reply.status == 200)
            {
                break;
            }
            ensure(Instant::now() < until, "restart-not-ready")?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    ctx.state.restart_ran = true;
    ctx.snapshot.clone_from(&snapshot.snapshot);
    ctx.facts =
        serde_json::to_value(&snapshot.facts).map_err(|_| Fail::new("envelope-undecodable"))?;
    ctx.client = ctx.reconnect().await?;
    let mut detection = "probed-at";
    if let Some(exec) = &exec {
        let read = ctx
            .raw("GET", &format!("/v1/execs/{}", exec.id), None)
            .await?;
        if read.result().is_some_and(|exec| exec["state"] == "unknown") {
            detection = "probed-at-and-exec";
        }
    }
    ctx.state.restart = Some(Restarted {
        workspace,
        write_path,
        write_body,
        write_result,
        written,
        head,
        exec,
    });
    Ok(Note {
        detection: Some(detection),
        ..Note::default()
    })
}

pub async fn ops(ctx: &mut Ctx) {
    for (spec, path) in [(&OPS_LIVEZ, "/livez"), (&OPS_READYZ, "/readyz")] {
        ctx.case(spec, async |ctx: &mut Ctx| {
            let ops = ctx
                .settings
                .ops_authority
                .clone()
                .ok_or_else(|| Fail::new("ops-undeclared"))?;
            let reply = plain_get(&ops, path, ctx.settings.io).await?;
            ensure(reply.status == 200, "unexpected-status")?;
            Ok(Note::default())
        })
        .await;
    }
    ctx.case(&OPS_METRICS, async |ctx: &mut Ctx| {
        let ops = ctx
            .settings
            .ops_authority
            .clone()
            .ok_or_else(|| Fail::new("ops-undeclared"))?;
        let reply = plain_get(&ops, "/metrics", ctx.settings.io).await?;
        ensure(reply.status == 200, "unexpected-status")?;
        let text = String::from_utf8(reply.body).map_err(|_| Fail::new("metrics-unparsable"))?;
        ensure(metrics_parse(&text), "metrics-unparsable")?;
        let credential = ctx
            .token(Purpose::Primary, Reason::Request)
            .await
            .map_err(|_| Fail::new("token-unavailable"))?;
        let carries_credential = text.contains(credential.as_str());
        drop(credential);
        ensure(!carries_credential, "metrics-carry-credential")?;
        let state = &ctx.state;
        let named = state
            .workspaces
            .iter()
            .chain(&state.execs)
            .chain(&state.sessions)
            .chain(&state.operations)
            .chain(std::iter::once(&ctx.settings.label))
            .any(|id| text.contains(id.as_str()));
        ensure(!named, "metrics-name-run-resources")?;
        Ok(Note::default())
    })
    .await;
}

/// Prometheus text exposition: every line is blank, a comment, or `name[{labels}] value`.
pub fn metrics_parse(text: &str) -> bool {
    text.lines().all(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return true;
        }
        let (series, rest) = match line.find('{') {
            Some(open) => match line.rfind('}') {
                Some(close) if close > open => (&line[..open], &line[close + 1..]),
                _ => return false,
            },
            None => match line.split_once(' ') {
                Some((name, rest)) => (name, rest),
                None => return false,
            },
        };
        let name_ok = series
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_' || first == b':')
            && series
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':');
        let value = rest.split_whitespace().next();
        name_ok
            && value.is_some_and(|value| {
                value.parse::<f64>().is_ok() || matches!(value, "+Inf" | "-Inf" | "NaN")
            })
    })
}

pub async fn cleanup(ctx: &mut Ctx) {
    if !ctx.selected(CLEANUP.group) {
        // A partial run still leaves nothing behind; it only does not report the case.
        let _ = remove_all(ctx).await;
        return;
    }
    ctx.case(&CLEANUP, async |ctx: &mut Ctx| remove_all(ctx).await)
        .await;
}

async fn remove_all(ctx: &mut Ctx) -> CaseResult {
    let mut codes = Vec::new();
    let paths: Vec<String> = ctx
        .state
        .sessions
        .iter()
        .map(|id| format!("/v1/sessions/{id}"))
        .chain(ctx.state.execs.iter().map(|id| format!("/v1/execs/{id}")))
        .chain(
            ctx.state
                .workspaces
                .iter()
                .map(|id| format!("/v1/workspaces/{id}")),
        )
        .collect();
    for path in paths {
        if let Err(fail) = retire(ctx, &path).await {
            codes.extend(fail.0);
        }
    }
    if codes.is_empty() {
        Ok(Note::default())
    } else {
        Err(Fail(codes))
    }
}

/// Destroys or retires one resource, killing it first while it still runs, then reads it absent.
async fn retire(ctx: &mut Ctx, path: &str) -> Result<(), Fail> {
    let until = Instant::now() + ctx.settings.restart_timeout;
    let mut signalled = false;
    loop {
        let read = ctx.raw("GET", path, None).await?;
        if read.status == 404 && read.error_code().as_deref() == Some("resource.not-found") {
            return Ok(());
        }
        let operation = ctx.operation_id();
        let reply = ctx
            .raw("DELETE", path, Some(&Ctx::mutation(&operation, &json!({}))))
            .await?;
        let code = reply.error_code();
        match (reply.status, code.as_deref()) {
            (200..=299, _) | (404, Some("resource.not-found")) => {}
            (409, Some("exec.not-terminal" | "session.not-terminal")) => {
                ensure(Instant::now() < until, "resource-not-retirable")
                    .map_err(|fail| fail.with(code.as_deref()))?;
                if !signalled {
                    signalled = true;
                    let operation = ctx.operation_id();
                    let kill = json!({ "signal": "KILL", "grace_ms": 0 });
                    let signal = format!("{path}/signal");
                    ctx.raw("POST", &signal, Some(&Ctx::mutation(&operation, &kill)))
                        .await?;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            _ => return Err(Fail::new("cleanup-refused").with(code.as_deref())),
        }
        let read = ctx.raw("GET", path, None).await?;
        let absent =
            read.status == 404 && read.error_code().as_deref() == Some("resource.not-found");
        return ensure(absent, "resource-still-readable");
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SESSION_OUTPUT, append_bounded, egress_routes, metrics_parse};

    /// Security review (u1): session output is bounded; past the bound the case fails by name.
    #[test]
    fn session_output_past_its_bound_fails_by_name() {
        let mut output = Vec::new();
        append_bounded(&mut output, &vec![b'a'; MAX_SESSION_OUTPUT - 1]).expect("within");
        append_bounded(&mut output, b"b").expect("exactly the bound");
        assert_eq!(
            append_bounded(&mut output, b"c").err().map(|fail| fail.0),
            Some(vec!["session-output-exceeded".to_owned()])
        );
        assert_eq!(
            output.len(),
            MAX_SESSION_OUTPUT,
            "nothing past the bound is kept"
        );
    }

    #[test]
    fn only_loopback_routes_mean_no_egress() {
        let isolated = "r4 Iface\tDestination\tGateway\nr6 00000000000000000000000000000001 80 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000001 00000000 00200001       lo\nend\n";
        assert_eq!(egress_routes(isolated), 0);
        let open = "r4 Iface\tDestination\nr4 eth0\t00000000\t0101A8C0\nend\n";
        assert_eq!(egress_routes(open), 1);
    }

    #[test]
    fn metrics_are_prometheus_text() {
        assert!(metrics_parse(
            "# HELP a b\n# TYPE a counter\na_total 3\nb{x=\"y\"} 1.5\n\n"
        ));
        assert!(!metrics_parse("not metrics at all\n"));
        assert!(!metrics_parse("a{x=\"y\" 1\n"));
    }
}
