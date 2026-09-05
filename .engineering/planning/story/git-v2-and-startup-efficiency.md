---
format: aep.planning-md/1
id: story:git-v2-and-startup-efficiency
kind: story
status: implemented
title: Fetch Git v2 and reduce workspace startup overhead
tags:
- coding-workspace-speed
relations:
- derived_from: story:materialize-connector-git-sources
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: Dockerfile
- confidence: inferred
  path: THIRD_PARTY_LICENSES.html
- confidence: inferred
  path: crates/b10x-substrate-sdk
- confidence: inferred
  path: crates/substrate-daemon/Cargo.toml
- confidence: inferred
  path: crates/substrate-host
- confidence: inferred
  path: crates/substrate-mcp/Cargo.toml
- confidence: inferred
  path: crates/substrate-store/Cargo.toml
- confidence: inferred
  path: crates/substrate-wire/Cargo.toml
- confidence: inferred
  path: docs/design
- confidence: inferred
  path: xtask/Cargo.toml
revision: 8
---
## Acceptance

Materialization negotiates Git v2 through the configured Connectors proxy using a pinned gix blocking HTTPS fetch, retaining the exact commit, one branch and 50 commits. Ambient configuration, helpers, redirects, tags, submodules, LFS and external pack URLs remain disabled; authorization stays transient. Preserve transfer/storage bounds, fsync, atomic install and recovery. Combine sync and accounting. Reuse bounded HTTP connections with request-specific authority and unchanged TLS/terminal exporter checks.

## Implementation and evidence

Designs 22 and 23 and Atlas ADR 0034 authorize this transport-only change. Source candidate 0.7.3 adds gix 0.87.1, statically linked curl, one synchronization/accounting walk, and a credential-free RemoteEndpoint factory. Frozen wire bundle 0.16.0 is unchanged.

Real HTTPS tests verify the exact commit and depth 50, usable history, one connection across three v2 requests, unrelated refs/tags excluded, no persisted authority, deadline/transfer/storage limits, cancellation and named TLS/ref/moved-commit refusals. A 10,000-ref fixture measured 690,429 legacy discovery bytes against 214 v2 discovery bytes. Nine fresh-session fixture runs measured a 141ms median and 144ms nearest-rank p95 for complete materialization. The production materializer also passed through the Connectors production TLS router and broker against real Git; this is fixture evidence, not hosted latency.

SDK tests verify per-request authority on reused TLS, invalid caller refusal after an authorized caller, no mutation replay by the pool, contract verification before body consumption, cancellation releasing capacity, at most 16 active connections and 30-second idle expiry. Fifteen SDK unit tests and all-targets release Clippy pass. Existing remote and operation-ledger tests pass; delegated cgroup cases retain their environment prerequisites.

Both final Docker images start with networking disabled. The daemon reaches readiness as its nonroot user with a configured Git source; the MCP image completes real JSON-RPC initialization and EOF. No shared libcurl dependency remains. License notices were generated under the existing policy; one upstream whitespace line is retained to preserve the notice fixed point.

The final combined `bash scripts/gate.sh` completed with exit 0, including release workspace tests and Clippy, all 16 frozen bundles, JSON classification, links, secrets, advisories, licenses and toolchain checks. Provider publication, consumer exact pins and production deployment observations remain distinct delivery evidence.
