# Architecture decision records

ADRs record accepted component decisions and are not implementation plans. Each ADR uses Markdown
with YAML frontmatter containing `date` and `status`. Draft design questions remain in
[`docs/design/`](../docs/design/) until accepted.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-substrate-is-standalone-and-flux-free.md) | Substrate is standalone and Flux-free | accepted |
| [0002](0002-substrate-owns-execution-not-product-policy.md) | Substrate owns execution, not product policy | accepted |
| [0003](0003-v1-starts-with-one-minimum-host-slice.md) | V1 starts with one minimum host slice | accepted |
| [0004](0004-the-host-driver-refuses-without-linux-confinement.md) | The host driver refuses without Linux confinement | accepted |
| [0005](0005-operations-are-durable-before-driver-dispatch.md) | Operations are durable before driver dispatch | accepted |
| [0006](0006-substrate-publishes-its-own-contract-bundle.md) | Substrate publishes its own contract bundle | accepted |
| [0007](0007-protocol-processes-use-raw-pipe-sessions.md) | Protocol processes use raw-pipe sessions | accepted |
| [0008](0008-pipe-sessions-have-distinct-durable-identity.md) | Pipe sessions have distinct durable identity | accepted |
| [0009](0009-execution-capsules-are-verified-read-only-inputs.md) | Execution capsules are verified read-only inputs | accepted |
| [0010](0010-declared-host-roots-are-mounted-read-only.md) | Declared host roots are mounted read-only | accepted |
| [0011](0011-delegated-context-and-grant-attribution.md) | Delegated context carries grant attribution | accepted |
| [0012](0012-secret-slots-are-sealed-memfds.md) | Secret slots are sealed memfds | accepted |
| [0013](0013-egress-apertures-are-declared-by-the-operator.md) | Egress apertures are declared by the operator and referenced by name | accepted |
| [0014](0014-apertures-carry-a-declared-byte-ceiling.md) | An egress aperture carries a declared byte ceiling | accepted |
| [0015](0015-declared-host-roots-carry-no-host-ipc.md) | Declared host roots carry no host IPC | accepted |
| [0016](0016-pipe-output-backpressure-is-terminal.md) | Pipe output backpressure is terminal | accepted |
| [0017](0017-delegated-context-is-verified-before-replay.md) | Delegated context is verified before replay | accepted |
| [0018](0018-one-registry-declares-every-served-api-major.md) | One registry declares every served API major | accepted |
| [0019](0019-pty-is-a-second-session-mode.md) | A PTY is a second session mode | accepted |
| [0020](0020-writable-storage-uses-delegated-project-quotas.md) | Writable storage uses delegated project quotas | accepted |
| [0021](0021-execution-metrics-are-explicit-exact-observations.md) | Execution metrics are explicit exact observations | accepted |
| [0022](0022-the-rust-sdk-remains-a-wire-client.md) | The Rust SDK remains a wire client | accepted |
| [0023](0023-workspace-write-access-is-explicit.md) | Workspace write access is explicit | accepted |
| [0024](0024-production-network-control-uses-server-authenticated-tls.md) | Production network control uses server-authenticated TLS | accepted |
| [0025](0025-the-mcp-adapter-is-a-disposable-test-surface.md) | The MCP adapter is a disposable test surface | accepted |
