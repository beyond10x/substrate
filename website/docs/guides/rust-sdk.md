---
title: Rust SDK
description: Create, control, and observe Substrate workspaces with builders, typed refusals, and an optional managed daemon child.
---

# Use Substrate from Rust

`b10x-substrate-sdk` is the high-level asynchronous Rust client. It speaks to the daemon over an
owner-private Unix socket, verifies the daemon's advertised contract, and returns typed workspace,
process, event, operation, and refusal observations.

The SDK and wire contract are development releases below 1.0. The first SDK intentionally covers
the contract the daemon advertises today, `substrate-wire/0.4.0`; later development additions are
not made available early merely because their Rust implementation exists.

## Connect and run an argv-only command

Add the client and Tokio to your application:

```toml
[dependencies]
b10x-substrate-sdk = "=0.3.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

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

## Connect to a daemon you own

The default SDK does not start a daemon. If your application should supervise one, point it at the
installed `substrate-daemon` binary and an explicit durable data directory:

```rust
use b10x_substrate_sdk::ManagedDaemon;

# async fn example() -> Result<(), b10x_substrate_sdk::SdkError> {
let daemon = ManagedDaemon::builder()
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
b10x-substrate-sdk = { version = "=0.3.0", features = ["linked-daemon"] }
```

Call the child entrypoint before parsing your application's command line, then select linked mode:

```rust
use b10x_substrate_sdk::{ManagedDaemon, run_daemon_child_if_requested};

#[tokio::main]
async fn main() -> Result<(), b10x_substrate_sdk::SdkError> {
    if run_daemon_child_if_requested().await? {
        return Ok(());
    }

    let daemon = ManagedDaemon::builder()
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
