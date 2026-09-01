# b10x-substrate-sdk

An asynchronous, builder-oriented Rust client for creating, controlling, and observing Substrate
workspaces and processes. It can connect to an existing Unix socket or own a separate daemon child.

```rust,no_run
use std::time::Duration;
use b10x_substrate_sdk::{Client, ExecutionPolicy};

# async fn example() -> Result<(), b10x_substrate_sdk::SdkError> {
let client = Client::builder()
    .unix_socket("run/substrate.sock")
    .connect()
    .await?;
let workspace = client.workspace().empty().create().await?;
let policy = ExecutionPolicy::builder()
    .timeout(Duration::from_secs(10))
    .cpu_time(Duration::from_secs(2))
    .memory_bytes(64 * 1024 * 1024)
    .processes(16)
    .output_bytes(64 * 1024)
    .build()?;
let output = workspace
    .command("/usr/bin/printf")
    .arg("hello\n")
    .policy(policy)
    .run()
    .await?;
assert_eq!(output.stdout, b"hello\n");
# Ok(())
# }
```

Every execution bound is explicit: the SDK deliberately supplies no product-policy defaults.
Managed linked mode remains a separate re-executed child and is opt-in through the `linked-daemon`
feature.

Current development source additionally covers the complete promoted contract: guarded v2 file
operations, bounded output pages, PTY sessions and resize, pull/stream metrics, reconciliation
snapshots, caller-supplied operation ids for every mutation, and exact capability-fact absence.
All public observations are serializable for protocol adapters such as MCP servers.

The SDK and its wire contract are under development and are not stable. See the
[public Rust SDK guide](https://beyond10x.github.io/substrate/docs/guides/rust-sdk) for lifecycle,
error, and deployment details.

Remote mode uses the same handles and requires every trust input explicitly:

```rust,no_run
use b10x_substrate_sdk::{AccessToken, Client, SdkError};

# async fn example() -> Result<(), b10x_substrate_sdk::SdkError> {
let client = Client::builder()
    .https_endpoint("https://127.0.0.1:8443/")
    .trust_roots("/etc/my-service/substrate-ca.pem")
    .server_identity("substrate.example.com")
    .token_provider(|_reason| async {
        // Replace this illustrative source with the deployment's Identity client.
        let value = std::env::var("SUBSTRATE_ACCESS_TOKEN")
            .map_err(|_| SdkError::TokenUnavailable)?;
        AccessToken::new(value)
    })
    .connect()
    .await?;
let _machine = client.machine();
# Ok(())
# }
```

There are no ambient system roots, redirects, proxies, plaintext fallback, credential store, or
certificate-verification bypass. Each request obtains authority from the provider; a hosted
session attachment additionally mints a fresh one-use authority bound to its TLS 1.3 channel.
