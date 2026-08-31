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

The SDK and its wire contract are under development and are not stable. See the
[public Rust SDK guide](https://beyond10x.github.io/substrate/docs/guides/rust-sdk) for lifecycle,
error, and deployment details.
