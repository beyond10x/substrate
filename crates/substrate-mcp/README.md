# `b10x-substrate-mcp`

Private, development-only stdio MCP adapter for exercising one disposable Substrate daemon. It is
not a second daemon ingress and is never published to crates.io.

The binary starts a linked daemon as a separate child, verifies its advertised contract through the
public Rust SDK, serves a closed bounded tool/resource surface, and cleans tracked executions and
workspaces before proving daemon-child absence. It opens no TCP listener and has no OAuth or HTTP
MCP feature.

Build and register the current development binary with Codex:

```console
cargo build --release -p b10x-substrate-mcp --bin substrate-mcp
codex mcp add substrate -- "$PWD/target/release/substrate-mcp"
```

See the [public MCP guide](https://beyond10x.github.io/substrate/docs/guides/mcp-adapter) for the
closed surface, operation-ID discipline and portable/delegated behavior.
