---
status: accepted
date: 2026-08-31
---

# ADR 0022: the Rust SDK remains a wire client

## Context

Rust consumers need an ergonomic way to create, control and observe Substrate workspaces and
processes. The repository already exposes wire types, daemon composition and a driver port, but
none is a client SDK. In particular, direct driver calls bypass the durable operation ledger,
peer-credential authentication, events and recovery.

A consumer may also need a single executable containing the daemon implementation. Collapsing
that implementation into the consumer process would make external and embedded behavior
different and erase the process boundary the local authentication model relies on.

## Decision

**The SDK is a typed client of the public Unix-socket contract.** It verifies the daemon-advertised
contract before returning a client, owns ergonomic public models and builders, requires explicit
execution limits, preserves named refusals, and automates ambiguous-operation reconciliation only
under the original caller-minted operation id.

**Managed mode owns a separate daemon child.** It may spawn the native daemon or link the daemon
crate and re-execute the current application into a child entrypoint. All resource operations in
both modes cross the authenticated Unix socket. The owner supervises shutdown and reaping; durable
state is retained unless temporary mode was explicitly selected.

**The runtime chain and SDK are approved public development packages.** Their registry package
names use the `b10x-substrate-*` prefix and exact release versions. The repository gate permits
only that closed set and continues to refuse accidental publication of every tooling package.
Crates are published manually from a fully gated annotated tag with an operator-held scoped token;
no crates.io credential is added to GitHub.

The first SDK supports only the daemon-advertised `substrate-wire/0.4.0` workspace, file, exec,
raw-pipe, event and recovery surface. It does not make that development contract stable and does
not expose later v2 or PTY additions before the daemon claims them.

## Consequences

A Rust caller gets a small builder-oriented API without learning HTTP or manufacturing optimistic
execution state. External and linked local deployments retain identical authentication,
durability, refusal and observation semantics.

The linked feature carries the complete daemon dependency chain and is therefore opt-in. Registry
publication becomes an explicit release responsibility, while the release workflow remains free
of long-lived registry credentials. The SDK cannot serve later development surfaces until a
separate consumer-coordinated contract-header change.

The full API and lifecycle closure are recorded in
[design 18](../docs/design/18-rust-sdk-and-managed-daemon.md).

## Distribution amendment

[ADR 0030](0030-rust-crates-are-source-distributed-and-non-publishable.md) replaces only this
record's registry-publication decision. The runtime chain and SDK keep their fixed package names
and exact internal edges but are source-distributed with `publish = false`; GitHub and GHCR remain
the release surfaces.
