use std::time::Duration;

use b10x_substrate_sdk::{
    ExecMeasurement, ExecutionPolicy, MetricsResourceKind, OutputStream, SdkError, Signal,
    StorageLimit,
};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::State;

const MIN_STORAGE_QUOTA_BYTES: u64 = 1_048_576;
const MAX_STORAGE_QUOTA_BYTES: u64 = 1_099_511_627_776;
const MIN_STORAGE_QUOTA_INODES: u64 = 16;
const MAX_STORAGE_QUOTA_INODES: u64 = 1_048_576;
const MIN_LEASE_TTL_MS: u64 = 1_000;
const MAX_LEASE_TTL_MS: u64 = 86_400_000;

pub fn tools() -> Value {
    json!({"tools": [
        tool("machine_get", "Read the exact advertised contract and capability facts.", object(&[], &[])),
        tool("workspace_create", "Create one empty workspace with an optional explicit storage quota.", object(
            &["operation_id"],
            &[("operation_id", string()), ("max_bytes", bounded_integer(MIN_STORAGE_QUOTA_BYTES, MAX_STORAGE_QUOTA_BYTES)), ("max_inodes", bounded_integer(MIN_STORAGE_QUOTA_INODES, MAX_STORAGE_QUOTA_INODES)), ("lease_ttl_ms", lease_ttl())]
        )),
        tool("workspace_get", "Read a workspace created by this adapter.", id_object("workspace_id", false)),
        tool("workspace_destroy", "Destroy a tracked workspace with a caller operation id.", mutation_object("workspace_id", &[])),
        tool("workspace_file_read", "Read one bounded file page with its complete digest.", object(
            &["workspace_id", "path", "offset", "limit_bytes"],
            &[("workspace_id", string()), ("path", string()), ("offset", integer()), ("limit_bytes", bounded_integer(1, b10x_substrate_sdk::MAX_IO_BYTES))]
        )),
        tool("workspace_file_write", "Write bounded base64 bytes with a caller operation id.", object(
            &["operation_id", "workspace_id", "path", "content_base64"],
            &[("operation_id", string()), ("workspace_id", string()), ("path", string()), ("content_base64", base64_file())]
        )),
        tool("workspace_file_delete", "Delete one file with a caller operation id.", mutation_object("workspace_id", &[("path", string())])),
        tool("workspace_lease_renew", "Renew a workspace lease with a caller operation id.", mutation_object("workspace_id", &[("ttl_ms", lease_ttl())])),
        tool("exec_start", "Start one argv-only, no-egress, explicitly bounded execution.", exec_start_schema()),
        tool("exec_get", "Read a tracked execution observation.", id_object("exec_id", false)),
        tool("exec_wait", "Wait at most 30 seconds for a tracked execution to become terminal.", object(
            &["exec_id", "timeout_ms"],
            &[("exec_id", string()), ("timeout_ms", bounded_integer(1, 30_000))]
        )),
        tool("exec_output_read", "Read one bounded stdout or stderr page.", object(
            &["exec_id", "stream", "offset", "limit_bytes"],
            &[("exec_id", string()), ("stream", enum_string(&["stdout", "stderr"])), ("offset", integer()), ("limit_bytes", bounded_integer(1, b10x_substrate_sdk::MAX_IO_BYTES))]
        )),
        tool("exec_signal", "Signal a tracked execution with a caller operation id.", mutation_object("exec_id", &[("signal", enum_string(&["interrupt", "terminate", "kill"])), ("grace_ms", bounded_integer(0, 30_000))])),
        tool("exec_lease_renew", "Renew an execution lease with a caller operation id.", mutation_object("exec_id", &[("ttl_ms", lease_ttl())])),
        tool("exec_retire", "Retire a terminal execution with a caller operation id.", mutation_object("exec_id", &[])),
        tool("operation_get", "Reconcile a caller operation id after an ambiguous outcome.", id_object("operation_id", false)),
        tool("metrics_get", "Read exact requested exec or workspace measurements.", object(
            &["resource_kind", "resource_id"],
            &[("resource_kind", enum_string(&["exec", "workspace"])), ("resource_id", string())]
        ))
    ]})
}

pub fn resources() -> Value {
    json!({"resources": [{
        "uri": "substrate://machine",
        "name": "Substrate machine facts",
        "description": "Exact advertised contract and verified capability facts.",
        "mimeType": "application/json"
    }]})
}

pub fn resource_templates() -> Value {
    json!({"resourceTemplates": [
        template("substrate://workspaces/{workspace_id}", "Workspace observation"),
        template("substrate://workspaces/{workspace_id}/files/{path}?offset={offset}&limit={limit}", "Bounded file page"),
        template("substrate://execs/{exec_id}", "Execution observation"),
        template("substrate://execs/{exec_id}/output/{stream}?offset={offset}&limit={limit}", "Bounded execution output"),
        template("substrate://operations/{operation_id}", "Durable operation record"),
        template("substrate://metrics/{resource_kind}/{resource_id}", "Exact resource metrics")
    ]})
}

pub async fn call(params: Value, state: &State) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return adapter_error("substrate-mcp.invalid-call", "tool name is required");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match dispatch_tool(name, arguments, state).await {
        Ok(mut value) => {
            normalize_refusal_classes(&mut value);
            tool_result(&value, false)
        }
        Err(error) => sdk_error(error),
    }
}

#[allow(clippy::too_many_lines)] // The closed tool vocabulary stays auditable in one exhaustive match.
async fn dispatch_tool(name: &str, arguments: Value, state: &State) -> Result<Value, ToolError> {
    match name {
        "machine_get" => {
            parse::<Empty>(arguments)?;
            Ok(serde_json::to_value(state.client.machine())?)
        }
        "workspace_create" => {
            let args = parse::<WorkspaceCreate>(arguments)?;
            remember_operation(state, &args.operation_id).await;
            if args.max_bytes.is_some() != args.max_inodes.is_some() {
                return Err(ToolError::adapter(
                    "substrate-mcp.storage-limit-incomplete",
                    "max_bytes and max_inodes must be supplied together",
                ));
            }
            let mut builder = state
                .client
                .workspace()
                .empty()
                .label("substrate-mcp", "disposable")
                .operation_id(args.operation_id.clone());
            if let (Some(max_bytes), Some(max_inodes)) = (args.max_bytes, args.max_inodes) {
                builder = builder.storage(StorageLimit {
                    max_bytes,
                    max_inodes,
                });
            }
            if let Some(ttl) = args.lease_ttl_ms {
                builder = builder.lease(Duration::from_millis(ttl));
            }
            let workspace = builder.create().await?;
            state
                .registry
                .lock()
                .await
                .workspaces
                .insert(workspace.id().to_owned());
            with_operation(&args.operation_id, workspace.observation())
        }
        "workspace_get" => {
            let args = parse::<WorkspaceId>(arguments)?;
            require_workspace(state, &args.workspace_id).await?;
            let workspace = state.client.get_workspace(&args.workspace_id).await?;
            Ok(serde_json::to_value(workspace.observation())?)
        }
        "workspace_destroy" => {
            let args = parse::<WorkspaceMutation>(arguments)?;
            require_workspace(state, &args.workspace_id).await?;
            {
                let registry = state.registry.lock().await;
                if registry.execs.values().any(|id| id == &args.workspace_id) {
                    return Err(ToolError::adapter(
                        "substrate-mcp.workspace-not-empty",
                        "retire tracked executions before destroying their workspace",
                    ));
                }
            }
            remember_operation(state, &args.operation_id).await;
            let workspace = state.client.get_workspace(&args.workspace_id).await?;
            let absent = workspace
                .destroy_with_operation_id(Some(args.operation_id.clone()))
                .await?;
            if absent {
                state
                    .registry
                    .lock()
                    .await
                    .workspaces
                    .remove(&args.workspace_id);
            }
            Ok(json!({"operation_id": args.operation_id, "absent": absent}))
        }
        "workspace_file_read" => {
            let args = parse::<FileRead>(arguments)?;
            require_workspace(state, &args.workspace_id).await?;
            let workspace = state.client.get_workspace(&args.workspace_id).await?;
            let page = workspace
                .read_file_v2(&args.path, args.offset, args.limit_bytes)
                .await?;
            let mut value = serde_json::to_value(&page)?;
            replace_bytes(&mut value, &page.bytes)?;
            Ok(value)
        }
        "workspace_file_write" => {
            let args = parse::<FileWrite>(arguments)?;
            require_workspace(state, &args.workspace_id).await?;
            remember_operation(state, &args.operation_id).await;
            let content = base64::engine::general_purpose::STANDARD
                .decode(args.content_base64)
                .map_err(|_| {
                    ToolError::adapter(
                        "substrate-mcp.base64-invalid",
                        "content_base64 is not canonical base64",
                    )
                })?;
            let workspace = state.client.get_workspace(&args.workspace_id).await?;
            let observed = workspace
                .write_file_with_operation_id(&args.path, content, Some(args.operation_id.clone()))
                .await?;
            with_operation(&args.operation_id, &observed)
        }
        "workspace_file_delete" => {
            let args = parse::<FileMutation>(arguments)?;
            require_workspace(state, &args.workspace_id).await?;
            remember_operation(state, &args.operation_id).await;
            let workspace = state.client.get_workspace(&args.workspace_id).await?;
            let absent = workspace
                .delete_file_with_operation_id(&args.path, Some(args.operation_id.clone()))
                .await?;
            Ok(json!({"operation_id": args.operation_id, "absent": absent}))
        }
        "workspace_lease_renew" => {
            let args = parse::<LeaseMutation>(arguments)?;
            require_workspace(state, &args.resource_id).await?;
            remember_operation(state, &args.operation_id).await;
            let mut workspace = state.client.get_workspace(&args.resource_id).await?;
            let observed = workspace
                .renew_lease_with_operation_id(
                    Duration::from_millis(args.ttl_ms),
                    Some(args.operation_id.clone()),
                )
                .await?;
            with_operation(&args.operation_id, observed)
        }
        "exec_start" => exec_start(parse(arguments)?, state).await,
        "exec_get" => {
            let args = parse::<ExecId>(arguments)?;
            require_exec(state, &args.exec_id).await?;
            let exec = state.client.get_exec(&args.exec_id).await?;
            Ok(serde_json::to_value(exec.observation())?)
        }
        "exec_wait" => {
            let args = parse::<ExecWait>(arguments)?;
            require_exec(state, &args.exec_id).await?;
            if args.timeout_ms == 0 || args.timeout_ms > 30_000 {
                return Err(ToolError::adapter(
                    "substrate-mcp.wait-bound",
                    "timeout_ms is outside 1..=30000",
                ));
            }
            let mut exec = state.client.get_exec(&args.exec_id).await?;
            Ok(serde_json::to_value(
                exec.wait_for(Duration::from_millis(args.timeout_ms))
                    .await?,
            )?)
        }
        "exec_output_read" => {
            let args = parse::<OutputRead>(arguments)?;
            require_exec(state, &args.exec_id).await?;
            let stream = match args.stream.as_str() {
                "stdout" => OutputStream::Stdout,
                "stderr" => OutputStream::Stderr,
                _ => {
                    return Err(ToolError::adapter(
                        "substrate-mcp.stream-invalid",
                        "stream must be stdout or stderr",
                    ));
                }
            };
            let exec = state.client.get_exec(&args.exec_id).await?;
            let page = exec
                .output_page(stream, args.offset, args.limit_bytes)
                .await?;
            let mut value = serde_json::to_value(&page)?;
            replace_bytes(&mut value, &page.bytes)?;
            Ok(value)
        }
        "exec_signal" => {
            let args = parse::<SignalMutation>(arguments)?;
            require_exec(state, &args.exec_id).await?;
            remember_operation(state, &args.operation_id).await;
            let signal = parse_signal(&args.signal)?;
            let mut exec = state.client.get_exec(&args.exec_id).await?;
            let observed = exec
                .signal_with_operation_id(
                    signal,
                    Duration::from_millis(args.grace_ms),
                    Some(args.operation_id.clone()),
                )
                .await?;
            with_operation(&args.operation_id, observed)
        }
        "exec_lease_renew" => {
            let args = parse::<ExecLeaseMutation>(arguments)?;
            require_exec(state, &args.exec_id).await?;
            remember_operation(state, &args.operation_id).await;
            let mut exec = state.client.get_exec(&args.exec_id).await?;
            let observed = exec
                .renew_lease_with_operation_id(
                    Duration::from_millis(args.ttl_ms),
                    Some(args.operation_id.clone()),
                )
                .await?;
            with_operation(&args.operation_id, observed)
        }
        "exec_retire" => {
            let args = parse::<ExecMutation>(arguments)?;
            require_exec(state, &args.exec_id).await?;
            remember_operation(state, &args.operation_id).await;
            let exec = state.client.get_exec(&args.exec_id).await?;
            let absent = exec
                .retire_with_operation_id(Some(args.operation_id.clone()))
                .await?;
            if absent {
                state.registry.lock().await.execs.remove(&args.exec_id);
            }
            Ok(json!({"operation_id": args.operation_id, "absent": absent}))
        }
        "operation_get" => {
            let args = parse::<OperationId>(arguments)?;
            require_operation(state, &args.operation_id).await?;
            Ok(serde_json::to_value(
                state.client.operation(&args.operation_id).await?,
            )?)
        }
        "metrics_get" => metrics(parse(arguments)?, state).await,
        _ => Err(ToolError::adapter(
            "substrate-mcp.tool-not-found",
            "tool is not in the closed surface",
        )),
    }
}

async fn exec_start(args: ExecStart, state: &State) -> Result<Value, ToolError> {
    require_workspace(state, &args.workspace_id).await?;
    let Some(program) = args.argv.first() else {
        return Err(ToolError::adapter(
            "substrate-mcp.argv-empty",
            "argv must contain a program",
        ));
    };
    remember_operation(state, &args.operation_id).await;
    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_millis(args.timeout_ms))
        .cpu_time(Duration::from_millis(args.cpu_millis))
        .memory_bytes(args.memory_bytes)
        .processes(args.processes)
        .output_bytes(args.output_bytes)
        .build()?;
    let workspace = state.client.get_workspace(&args.workspace_id).await?;
    let mut builder = workspace
        .command(program)
        .args(args.argv.iter().skip(1))
        .policy(policy)
        .operation_id(args.operation_id.clone());
    if args.measure_resource_usage {
        builder = builder.measure(ExecMeasurement::ResourceUsage);
    }
    if let Some(ttl) = args.lease_ttl_ms {
        builder = builder.lease(Duration::from_millis(ttl));
    }
    let exec = builder.start().await?;
    state
        .registry
        .lock()
        .await
        .execs
        .insert(exec.id().to_owned(), args.workspace_id);
    with_operation(&args.operation_id, exec.observation())
}

async fn metrics(args: MetricsGet, state: &State) -> Result<Value, ToolError> {
    let kind = match args.resource_kind.as_str() {
        "exec" => {
            require_exec(state, &args.resource_id).await?;
            MetricsResourceKind::Exec
        }
        "workspace" => {
            require_workspace(state, &args.resource_id).await?;
            MetricsResourceKind::Workspace
        }
        _ => {
            return Err(ToolError::adapter(
                "substrate-mcp.resource-kind-invalid",
                "resource_kind must be exec or workspace",
            ));
        }
    };
    Ok(serde_json::to_value(
        state.client.metrics(kind, args.resource_id).await?,
    )?)
}

#[allow(clippy::too_many_lines)] // The six closed URI shapes stay in one precedence-ordered parser.
pub async fn read_resource(params: Value, state: &State) -> Result<Value, ToolError> {
    let args = parse::<ResourceRead>(params)?;
    let value = if args.uri == "substrate://machine" {
        serde_json::to_value(state.client.machine())?
    } else if let Some(rest) = args.uri.strip_prefix("substrate://workspaces/")
        && let Some((workspace_id, target)) = rest.split_once("/files/")
    {
        let workspace_id = percent_decode(workspace_id)?;
        require_workspace(state, &workspace_id).await?;
        let (encoded_path, query) = target.split_once('?').ok_or_else(|| {
            ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "file resource URI requires offset and limit",
            )
        })?;
        let path = percent_decode(encoded_path)?;
        let (offset, limit) = offset_limit(query)?;
        let workspace = state.client.get_workspace(workspace_id).await?;
        let page = workspace.read_file_v2(path, offset, limit).await?;
        let mut value = serde_json::to_value(&page)?;
        replace_bytes(&mut value, &page.bytes)?;
        value
    } else if let Some(id) = args.uri.strip_prefix("substrate://workspaces/") {
        if id.contains('/') || id.contains('?') {
            return Err(ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "workspace resource URI is invalid",
            ));
        }
        let id = percent_decode(id)?;
        require_workspace(state, &id).await?;
        let workspace = state.client.get_workspace(id).await?;
        serde_json::to_value(workspace.observation())?
    } else if let Some(rest) = args.uri.strip_prefix("substrate://execs/")
        && let Some((exec_id, target)) = rest.split_once("/output/")
    {
        let exec_id = percent_decode(exec_id)?;
        require_exec(state, &exec_id).await?;
        let (stream, query) = target.split_once('?').ok_or_else(|| {
            ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "output resource URI requires offset and limit",
            )
        })?;
        let stream = match stream {
            "stdout" => OutputStream::Stdout,
            "stderr" => OutputStream::Stderr,
            _ => {
                return Err(ToolError::adapter(
                    "substrate-mcp.uri-invalid",
                    "output resource stream is invalid",
                ));
            }
        };
        let (offset, limit) = offset_limit(query)?;
        let exec = state.client.get_exec(exec_id).await?;
        let page = exec.output_page(stream, offset, limit).await?;
        let mut value = serde_json::to_value(&page)?;
        replace_bytes(&mut value, &page.bytes)?;
        value
    } else if let Some(id) = args.uri.strip_prefix("substrate://execs/") {
        if id.contains('/') || id.contains('?') {
            return Err(ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "exec resource URI is invalid",
            ));
        }
        let id = percent_decode(id)?;
        require_exec(state, &id).await?;
        let exec = state.client.get_exec(id).await?;
        serde_json::to_value(exec.observation())?
    } else if let Some(id) = args.uri.strip_prefix("substrate://operations/") {
        if id.contains('/') || id.contains('?') {
            return Err(ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "operation resource URI is invalid",
            ));
        }
        let id = percent_decode(id)?;
        require_operation(state, &id).await?;
        serde_json::to_value(state.client.operation(id).await?)?
    } else if let Some(rest) = args.uri.strip_prefix("substrate://metrics/") {
        let (kind, id) = rest.split_once('/').ok_or_else(|| {
            ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "metrics resource URI is invalid",
            )
        })?;
        let id = percent_decode(id)?;
        let kind = match kind {
            "exec" => {
                require_exec(state, &id).await?;
                MetricsResourceKind::Exec
            }
            "workspace" => {
                require_workspace(state, &id).await?;
                MetricsResourceKind::Workspace
            }
            _ => {
                return Err(ToolError::adapter(
                    "substrate-mcp.uri-invalid",
                    "metrics resource kind is invalid",
                ));
            }
        };
        serde_json::to_value(state.client.metrics(kind, id).await?)?
    } else {
        return Err(ToolError::adapter(
            "substrate-mcp.uri-unserved",
            "resource URI is not in the closed surface",
        ));
    };
    let mut value = value;
    normalize_refusal_classes(&mut value);
    let text = serde_json::to_string(&value).map_err(ToolError::from)?;
    Ok(json!({"contents": [{"uri": args.uri, "mimeType": "application/json", "text": text}]}))
}

async fn require_workspace(state: &State, id: &str) -> Result<(), ToolError> {
    if state.registry.lock().await.workspaces.contains(id) {
        Ok(())
    } else {
        Err(ToolError::adapter(
            "substrate-mcp.resource-untracked",
            "workspace does not belong to this disposable adapter",
        ))
    }
}

async fn require_exec(state: &State, id: &str) -> Result<(), ToolError> {
    if state.registry.lock().await.execs.contains_key(id) {
        Ok(())
    } else {
        Err(ToolError::adapter(
            "substrate-mcp.resource-untracked",
            "execution does not belong to this disposable adapter",
        ))
    }
}

async fn require_operation(state: &State, id: &str) -> Result<(), ToolError> {
    if state.registry.lock().await.operations.contains(id) {
        Ok(())
    } else {
        Err(ToolError::adapter(
            "substrate-mcp.operation-untracked",
            "operation id was not submitted through this adapter",
        ))
    }
}

async fn remember_operation(state: &State, id: &str) {
    state.registry.lock().await.operations.insert(id.to_owned());
}

fn parse_signal(value: &str) -> Result<Signal, ToolError> {
    match value {
        "interrupt" => Ok(Signal::Interrupt),
        "terminate" => Ok(Signal::Terminate),
        "kill" => Ok(Signal::Kill),
        _ => Err(ToolError::adapter(
            "substrate-mcp.signal-invalid",
            "signal is outside the closed vocabulary",
        )),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ToolError> {
    serde_json::from_value(value).map_err(|_| {
        ToolError::adapter(
            "substrate-mcp.arguments-invalid",
            "arguments do not match the tool schema",
        )
    })
}

fn with_operation<T: serde::Serialize>(operation_id: &str, value: T) -> Result<Value, ToolError> {
    Ok(json!({"operation_id": operation_id, "observation": serde_json::to_value(value)?}))
}

fn replace_bytes(value: &mut Value, bytes: &[u8]) -> Result<(), ToolError> {
    let object = value.as_object_mut().ok_or_else(|| {
        ToolError::adapter(
            "substrate-mcp.projection-failed",
            "observation is not an object",
        )
    })?;
    object.remove("bytes");
    object.insert(
        "content_base64".to_owned(),
        Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
    );
    Ok(())
}

fn sdk_error(error: ToolError) -> Value {
    tool_result(&json!({"error": error_detail(error)}), true)
}

pub(crate) fn error_detail(error: ToolError) -> Value {
    match error {
        ToolError::Adapter { code, message } => {
            json!({"kind": "adapter", "code": code, "message": message})
        }
        ToolError::Sdk(SdkError::Refusal(refusal)) => json!({
            "kind": "daemon_refusal",
            "class": refusal_class(refusal.class),
            "code": refusal.code,
            "message": refusal.message,
            "retriable": refusal.retriable,
            "address": refusal.address,
            "operation_id": refusal.operation_id
        }),
        ToolError::Sdk(SdkError::UnknownOperation { operation_id }) => json!({
            "kind": "ambiguous_outcome",
            "code": "substrate-mcp.operation-outcome-unknown",
            "operation_id": operation_id
        }),
        ToolError::Sdk(SdkError::ContractMismatch { .. }) => json!({
            "kind": "adapter",
            "code": "substrate-mcp.contract-mismatch",
            "message": "daemon contract does not match this adapter"
        }),
        ToolError::Sdk(_) | ToolError::Json => json!({
            "kind": "adapter",
            "code": "substrate-mcp.internal",
            "message": "adapter operation failed without a safe projection"
        }),
    }
}

fn refusal_class(class: b10x_substrate_sdk::RefusalClass) -> &'static str {
    match class {
        b10x_substrate_sdk::RefusalClass::Refused => "refused",
        b10x_substrate_sdk::RefusalClass::Conflict => "conflict",
        b10x_substrate_sdk::RefusalClass::Unserved => "unserved",
        b10x_substrate_sdk::RefusalClass::Exhausted => "exhausted",
        b10x_substrate_sdk::RefusalClass::Failed => "failed",
    }
}

fn normalize_refusal_classes(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_refusal_classes(value);
            }
        }
        Value::Object(fields) => {
            if let Some(Value::String(class)) = fields.get_mut("class") {
                match class.as_str() {
                    "Refused" | "Conflict" | "Unserved" | "Exhausted" | "Failed" => {
                        class.make_ascii_lowercase();
                    }
                    _ => {}
                }
            }
            for value in fields.values_mut() {
                normalize_refusal_classes(value);
            }
        }
        _ => {}
    }
}

fn adapter_error(code: &'static str, message: &'static str) -> Value {
    sdk_error(ToolError::adapter(code, message))
}

fn tool_result(structured: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&structured).unwrap_or_else(|_| {
        "{\"error\":{\"code\":\"substrate-mcp.projection-failed\"}}".to_owned()
    });
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
        "isError": is_error
    })
}

#[derive(Debug)]
pub enum ToolError {
    Adapter {
        code: &'static str,
        message: &'static str,
    },
    Sdk(SdkError),
    Json,
}

impl ToolError {
    fn adapter(code: &'static str, message: &'static str) -> Self {
        Self::Adapter { code, message }
    }
}

impl From<SdkError> for ToolError {
    fn from(value: SdkError) -> Self {
        Self::Sdk(value)
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(_: serde_json::Error) -> Self {
        Self::Json
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCreate {
    operation_id: String,
    max_bytes: Option<u64>,
    max_inodes: Option<u64>,
    lease_ttl_ms: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceId {
    workspace_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceMutation {
    operation_id: String,
    workspace_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRead {
    workspace_id: String,
    path: String,
    offset: u64,
    limit_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileWrite {
    operation_id: String,
    workspace_id: String,
    path: String,
    content_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileMutation {
    operation_id: String,
    workspace_id: String,
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseMutation {
    operation_id: String,
    #[serde(rename = "workspace_id")]
    resource_id: String,
    ttl_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecStart {
    operation_id: String,
    workspace_id: String,
    argv: Vec<String>,
    timeout_ms: u64,
    cpu_millis: u64,
    memory_bytes: u64,
    processes: u32,
    output_bytes: u64,
    #[serde(default)]
    measure_resource_usage: bool,
    lease_ttl_ms: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecId {
    exec_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecWait {
    exec_id: String,
    timeout_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputRead {
    exec_id: String,
    stream: String,
    offset: u64,
    limit_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignalMutation {
    operation_id: String,
    exec_id: String,
    signal: String,
    grace_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecLeaseMutation {
    operation_id: String,
    exec_id: String,
    ttl_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecMutation {
    operation_id: String,
    exec_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationId {
    operation_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetricsGet {
    resource_kind: String,
    resource_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceRead {
    uri: String,
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "schemas are assembled as owned JSON values"
)]
fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    let read_only = matches!(
        name,
        "machine_get"
            | "workspace_get"
            | "workspace_file_read"
            | "exec_get"
            | "exec_wait"
            | "exec_output_read"
            | "operation_get"
            | "metrics_get"
    );
    let destructive = matches!(
        name,
        "workspace_destroy"
            | "workspace_file_write"
            | "workspace_file_delete"
            | "exec_signal"
            | "exec_retire"
    );
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": true,
            "openWorldHint": false
        }
    })
}

fn template(uri_template: &str, name: &str) -> Value {
    json!({"uriTemplate": uri_template, "name": name, "mimeType": "application/json"})
}

fn string() -> Value {
    json!({"type": "string", "maxLength": 4096})
}
fn integer() -> Value {
    json!({"type": "integer", "minimum": 0})
}
fn bounded_integer(minimum: u64, maximum: u64) -> Value {
    json!({"type": "integer", "minimum": minimum, "maximum": maximum})
}
fn lease_ttl() -> Value {
    bounded_integer(MIN_LEASE_TTL_MS, MAX_LEASE_TTL_MS)
}
fn base64_file() -> Value {
    json!({
        "type": "string",
        "contentEncoding": "base64",
        "maxLength": 1_398_104
    })
}
fn enum_string(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn object(required: &[&str], fields: &[(&str, Value)]) -> Value {
    let properties = fields
        .iter()
        .map(|(name, value)| {
            let value = if *name == "operation_id" {
                operation_id_schema()
            } else {
                value.clone()
            };
            ((*name).to_owned(), value)
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"type": "object", "additionalProperties": false, "required": required, "properties": properties})
}

fn operation_id_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 16,
        "maxLength": 128,
        "pattern": "^[A-Za-z0-9_-]+$"
    })
}

fn id_object(id: &str, mutation: bool) -> Value {
    if mutation {
        mutation_object(id, &[])
    } else {
        object(&[id], &[(id, string())])
    }
}

fn mutation_object(resource_id: &str, extra: &[(&str, Value)]) -> Value {
    let mut fields = vec![("operation_id", string()), (resource_id, string())];
    fields.extend_from_slice(extra);
    let mut required = vec!["operation_id", resource_id];
    required.extend(extra.iter().map(|(name, _)| *name));
    object(&required, &fields)
}

fn exec_start_schema() -> Value {
    object(
        &[
            "operation_id",
            "workspace_id",
            "argv",
            "timeout_ms",
            "cpu_millis",
            "memory_bytes",
            "processes",
            "output_bytes",
        ],
        &[
            ("operation_id", string()),
            ("workspace_id", string()),
            (
                "argv",
                json!({"type": "array", "minItems": 1, "maxItems": 256, "items": string()}),
            ),
            (
                "timeout_ms",
                bounded_integer(
                    1,
                    u64::try_from(b10x_substrate_sdk::MAX_EXEC_DURATION.as_millis())
                        .expect("SDK duration bound fits u64"),
                ),
            ),
            (
                "cpu_millis",
                bounded_integer(
                    1,
                    u64::try_from(b10x_substrate_sdk::MAX_EXEC_DURATION.as_millis())
                        .expect("SDK duration bound fits u64"),
                ),
            ),
            (
                "memory_bytes",
                bounded_integer(
                    b10x_substrate_sdk::MIN_EXEC_MEMORY_BYTES,
                    b10x_substrate_sdk::MAX_EXEC_MEMORY_BYTES,
                ),
            ),
            (
                "processes",
                bounded_integer(1, u64::from(b10x_substrate_sdk::MAX_EXEC_PROCESSES)),
            ),
            (
                "output_bytes",
                bounded_integer(1, b10x_substrate_sdk::MAX_IO_BYTES),
            ),
            (
                "measure_resource_usage",
                json!({"type": "boolean", "default": false}),
            ),
            ("lease_ttl_ms", lease_ttl()),
        ],
    )
}

fn percent_decode(value: &str) -> Result<String, ToolError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(pair) = bytes.get(index + 1..index + 3) else {
                return Err(ToolError::adapter(
                    "substrate-mcp.uri-invalid",
                    "percent escape is incomplete",
                ));
            };
            let text = std::str::from_utf8(pair).map_err(|_| {
                ToolError::adapter("substrate-mcp.uri-invalid", "percent escape is invalid")
            })?;
            let byte = u8::from_str_radix(text, 16).map_err(|_| {
                ToolError::adapter("substrate-mcp.uri-invalid", "percent escape is invalid")
            })?;
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| ToolError::adapter("substrate-mcp.uri-invalid", "URI component is not UTF-8"))
}

fn offset_limit(query: &str) -> Result<(u64, u64), ToolError> {
    let mut offset = None;
    let mut limit = None;
    for field in query.split('&') {
        let (name, value) = field.split_once('=').ok_or_else(|| {
            ToolError::adapter("substrate-mcp.uri-invalid", "resource query is invalid")
        })?;
        let parsed = value.parse::<u64>().map_err(|_| {
            ToolError::adapter(
                "substrate-mcp.uri-invalid",
                "resource query value is not an integer",
            )
        })?;
        match name {
            "offset" if offset.replace(parsed).is_none() => {}
            "limit" if limit.replace(parsed).is_none() => {}
            _ => {
                return Err(ToolError::adapter(
                    "substrate-mcp.uri-invalid",
                    "resource query has an unknown or duplicate field",
                ));
            }
        }
    }
    match (offset, limit) {
        (Some(offset), Some(limit)) => Ok((offset, limit)),
        _ => Err(ToolError::adapter(
            "substrate-mcp.uri-invalid",
            "resource query requires offset and limit",
        )),
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::{
        normalize_refusal_classes, offset_limit, percent_decode, resource_templates, resources,
        tools,
    };

    #[test]
    fn the_closed_surface_has_seventeen_tools_and_six_templates() {
        assert_eq!(tools()["tools"].as_array().expect("tools").len(), 17);
        assert_eq!(
            resource_templates()["resourceTemplates"]
                .as_array()
                .expect("templates")
                .len(),
            6
        );
        for tool in tools()["tools"].as_array().expect("tools") {
            assert_eq!(tool["annotations"]["openWorldHint"], false);
        }
    }

    #[test]
    fn the_closed_surface_schema_is_a_fixed_point() {
        let snapshot = serde_json::to_vec(&serde_json::json!({
            "tools": tools(),
            "resources": resources(),
            "resource_templates": resource_templates(),
            "bounds": {
                "frame_bytes": crate::MAX_FRAME_BYTES,
                "request_id_bytes": crate::MAX_REQUEST_ID_BYTES,
                "concurrent_calls": crate::MAX_CALLS,
                "call_deadline_ms": crate::WAIT_LIMIT.as_millis()
            },
            "unsupported_capabilities": [
                "completions", "elicitation", "logging", "prompts", "sampling", "tasks"
            ]
        }))
        .expect("snapshot");
        assert_eq!(
            hex::encode(Sha256::digest(snapshot)),
            "9d96e0a45babe70c964fbfbf70c0f6c3b2970d5bc002b4d901603973a17925c0"
        );
    }

    #[test]
    fn nested_sdk_refusal_classes_are_projected_as_wire_values() {
        let mut value = serde_json::json!({
            "state": "Refused",
            "refusal": {"class": "Unserved", "code": "exec.sandbox-unavailable"},
            "unrelated": {"class": "custom"}
        });
        normalize_refusal_classes(&mut value);
        assert_eq!(value["refusal"]["class"], "unserved");
        assert_eq!(value["unrelated"]["class"], "custom");
    }

    #[test]
    fn duplicated_storage_schema_bounds_match_the_sdk_contract_type() {
        let minimum = b10x_substrate_sdk::StorageLimit {
            max_bytes: super::MIN_STORAGE_QUOTA_BYTES,
            max_inodes: super::MIN_STORAGE_QUOTA_INODES,
        };
        let maximum = b10x_substrate_sdk::StorageLimit {
            max_bytes: super::MAX_STORAGE_QUOTA_BYTES,
            max_inodes: super::MAX_STORAGE_QUOTA_INODES,
        };
        assert!(minimum.within_contract_bounds());
        assert!(maximum.within_contract_bounds());
        assert!(
            !b10x_substrate_sdk::StorageLimit {
                max_bytes: super::MIN_STORAGE_QUOTA_BYTES - 1,
                max_inodes: super::MIN_STORAGE_QUOTA_INODES,
            }
            .within_contract_bounds()
        );
        assert!(
            !b10x_substrate_sdk::StorageLimit {
                max_bytes: super::MAX_STORAGE_QUOTA_BYTES + 1,
                max_inodes: super::MAX_STORAGE_QUOTA_INODES,
            }
            .within_contract_bounds()
        );
    }

    #[test]
    fn uri_decoding_is_strict_and_preserves_encoded_paths() {
        assert_eq!(
            percent_decode("src%2Flib.rs").expect("decode"),
            "src/lib.rs"
        );
        assert!(percent_decode("bad%2").is_err());
        assert!(percent_decode("bad%GG").is_err());
    }

    #[test]
    fn bounded_resource_queries_are_closed_and_duplicate_sensitive() {
        assert_eq!(offset_limit("offset=3&limit=7").expect("query"), (3, 7));
        assert!(offset_limit("offset=3").is_err());
        assert!(offset_limit("offset=3&offset=4&limit=7").is_err());
        assert!(offset_limit("offset=3&limit=7&other=1").is_err());
    }
}
