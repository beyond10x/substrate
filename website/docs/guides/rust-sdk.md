---
title: Rust SDK
description: Create, control, and observe Substrate workspaces with builders, typed refusals, and an optional managed daemon child.
---

# Use Substrate from Rust

`b10x-substrate-sdk` is the high-level asynchronous Rust client. It speaks to the daemon over an
owner-private Unix socket, verifies the daemon's advertised contract, and returns typed workspace,
process, event, operation, and refusal observations.

The SDK and wire contract are development releases below 1.0. Current development source verifies
the explicitly promoted `substrate-wire/0.13.0` name and inner manifest digest before it serves an
operation. Missing, older, unknown, and wrong-digest daemon claims are refused; a newer Rust type in
the workspace does not by itself advance that pair.

## Connect and run an argv-only command

Add the client and Tokio to your application:

```toml
[dependencies]
b10x-substrate-sdk = "=0.4.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
ulid = "3"
```

That dependency is the latest crates.io SDK release. The parity APIs later on this page describe
current development source and will ship in a successor release; they are not retroactive claims
about `0.4.0`.

Then connect, create an empty workspace, and state every execution limit explicitly:

```rust
use std::time::Duration;

use b10x_substrate_sdk::{Client, ExecutionPolicy};

#[tokio::main]
async fn main() -> Result<(), b10x_substrate_sdk::SdkError> {
    let client = Client::builder()
        .unix_socket("run/substrate.sock")
        .connect()
        .await?;

    let workspace = client
        .workspace()
        .empty()
        .label("purpose", "example")
        .create()
        .await?;

    workspace.write_file("input.txt", b"hello from Rust\n").await?;

    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(15))
        .cpu_time(Duration::from_secs(2))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(64 * 1024)
        .build()?;

    let output = workspace
        .command("/usr/bin/sha256sum")
        .arg("/workspace/input.txt")
        .policy(policy)
        .run()
        .await?;

    print!("{}", String::from_utf8_lossy(&output.stdout));
    workspace.destroy().await?;
    Ok(())
}
```

The command builder sends the argument vector as written. It does not invoke a shell or offer a
shell-string shortcut. The policy builder has no defaults: wall time, cumulative CPU, memory plus
swap, process count, and retained output must all be chosen by the application.

If the host cannot prove the required confinement, `run()` returns `SdkError::Refusal` with the
daemon's canonical class, code, address, retry fact, and operation id. A non-zero program exit is
instead a successful observation in `RunOutput`.

## Preserve capability absence

Use the exact fact set when deciding whether to offer an operation. `None` means the daemon did not
prove the guarantee; it does not mean a negative fact was observed.

```rust
let machine = client.machine();
if machine.facts.sessions_pty == Some(true) {
    // A PTY request may now be attempted. Dispatch can still return a named refusal.
}

if let Some(usage) = machine.facts.exec_resource_usage {
    println!("memory peak available: {}", usage.memory_peak);
} else {
    println!("this deployment did not prove the complete usage counter set");
}
```

The older convenience booleans remain available, but they deliberately collapse information. Use
`Machine::facts` for admission, schema projection, or an agent-facing adapter.

## Drive a terminal and observe usage

PTY is a mode of the leased session resource, not a shell-string exec. State the initial window and
all channel bounds, attach once, and resize through the typed channel:

```rust
use b10x_substrate_sdk::{ExecMeasurement, PtyWindow};

let session = workspace
    .pty_session("/usr/bin/bash", PtyWindow { columns: 100, rows: 30 })
    .policy(policy)
    .measure(ExecMeasurement::ResourceUsage)
    .lease(Duration::from_secs(30))
    .input_limit_bytes(1024 * 1024)
    .frame_limit_bytes(16 * 1024)
    .queued_frames(8)
    .start()
    .await?;

let mut terminal = session.attach().await?;
terminal.resize(PtyWindow { columns: 120, rows: 40 }).await?;
terminal.write(b"printf 'hello from a pty\\n'\n").await?;
```

Request `ExecMeasurement::ResourceUsage` only when the exact fact is present. Then use
`Client::metrics` for a point-in-time observation or `Client::metrics_stream` for latest-wins live
samples. A host that cannot expose every declared counter keeps the fact absent and returns a named
refusal; the SDK does not manufacture partial metrics.

## Guard file changes and recovery

The development SDK covers the v2 compare-and-set byte plane and lets every mutation keep a caller
operation id:

```rust
use b10x_substrate_sdk::ExpectedFileState;

let operation_id = ulid::Ulid::generate().to_string();
let changed = workspace
    .replace_file(
        "src/config.txt",
        b"mode=confined\n",
        ExpectedFileState::Absent,
        true,
        Some(operation_id.clone()),
    )
    .await?;

let recorded = client.operation(&operation_id).await?;
assert_eq!(recorded.id, operation_id);
println!("new digest: {}", changed.after_sha256);
```

`read_file_v2` returns a bounded byte page plus the complete-file digest; `tree` returns a bounded
recursive view. `create_reconciliation_snapshot` and `reconciliation_snapshot_page` provide a
barriered recovery view after an event-history gap. `Exec::output_page` exposes the same explicit
offset and byte limit instead of allocating an unbounded transcript.

## Connect to a daemon you own

The default SDK does not start a daemon. If your application should supervise one, point it at the
installed `substrate-daemon` binary and an explicit durable data directory:

```rust
use b10x_substrate_sdk::ManagedDaemon;

# async fn example() -> Result<(), b10x_substrate_sdk::SdkError> {
    let mut daemon = ManagedDaemon::builder()
    .data_dir("run/my-application-substrate")
    .deployment("my_application")
    .external_binary("/usr/local/bin/substrate-daemon")
    .start()
    .await?;

let client = daemon.client();
let machine = client.machine();
println!("capability snapshot: {}", machine.capability_snapshot);

daemon.shutdown().await?;
# Ok(())
# }
```

Managed mode always starts a separate child process. It admits only the invoking effective user,
waits for a contract-verified readiness response, and owns shutdown and reaping. Explicit shutdown
retains the state database and workspaces. Use `.temporary()` only when removal after shutdown is
actually intended.

## Ship one application executable

The optional `linked-daemon` feature links the daemon so the application can re-execute its own
binary as the child. It does not run the service in-process.

```toml
[dependencies]
b10x-substrate-sdk = { version = "=0.4.0", features = ["linked-daemon"] }
```

Call the child entrypoint before parsing your application's command line, then select linked mode:

```rust
use b10x_substrate_sdk::{ManagedDaemon, run_daemon_child_if_requested};

#[tokio::main]
async fn main() -> Result<(), b10x_substrate_sdk::SdkError> {
    if run_daemon_child_if_requested().await? {
        return Ok(());
    }

    let mut daemon = ManagedDaemon::builder()
        .data_dir("run/my-application-substrate")
        .deployment("my_application")
        .linked_current_exe()
        .start()
        .await?;

    // Resource operations still cross the authenticated Unix socket.
    let _machine = daemon.client().machine();
    daemon.shutdown().await
}
```

The parent holds a liveness pipe to the child. Dropping the owner initiates bounded shutdown;
`shutdown().await` is preferred when the application needs the result. This model preserves peer
credentials, socket permissions, independent failure, and the same wire behavior as an external
daemon.

## Recover and observe

Mutation builders mint one operation id unless you provide one. If a response is lost, the SDK
queries the durable operation ledger and can replay the identical request once under that same id;
it never substitutes a new id. `SdkError::UnknownOperation` carries the id needed for later
reconciliation.

Use `Client::events` for bounded pages, `Client::event_stream` for the cursor-preserving WebSocket
stream, and `Client::operation` to inspect a known mutation. A retention gap is a typed
`SdkError::EventGap`; the SDK does not silently skip to current state.
