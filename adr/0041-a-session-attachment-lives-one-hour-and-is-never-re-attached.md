---
status: accepted
date: 2026-10-23
---

# ADR 0041: a session attachment lives one hour and is never re-attached

Amends [ADR 0008](0008-pipe-sessions-have-distinct-durable-identity.md): its decision names
"lifetime expiry" as a cause of whole-tree cancellation without saying what the lifetime is, or
that a consumed attachment right has no successor. This ADR records both, and the consumer they
were chosen for. It changes no behaviour and no wire byte; it states as a decision what the daemon
and the published capability already do.

## Context

The daemon bounds every session attachment, raw-pipe and PTY alike, by one policy value:
`lifetime: std::time::Duration::from_hours(1)` in `PipeSessionPolicy::production`
(`crates/substrate-daemon/src/app/sessions.rs:77`). The upgraded attachment runs inside
`tokio::time::timeout(policy.lifetime, run_pipe_attachment(..))` (`sessions.rs:1364-1366`); when it
returns without a terminal observation, because the lifetime lapsed, the client closed or dropped
the socket (`sessions.rs:1478`), or an arm answered a `protocol-error` frame and returned
(`sessions.rs:1551-1559`), the daemon calls `terminate_pipe_session` (`sessions.rs:1378-1379`) and,
if that kill is not proven, queues the session for the containment step.

The attachment right is claimed once, durably. `claim_pipe_session_attachment_inner`
(`crates/substrate-store/src/sessions.rs:728`) answers `AlreadyClaimed` for a session whose
attachment state is `attached`, `consumed` or `uncertain`, before it looks at the session's state or
lease, and the route answers that as `session.already-attached` (409)
(`crates/substrate-daemon/src/app/sessions.rs:1261-1267`). A fresh session authority does not
reopen it: the authority is checked first and the attachment state after, so a valid, unredeemed
authority for a claimed session still meets `AlreadyClaimed`.

The figure is already published. The session capability result carries
`attachment_lifetime_ms`, set from the same policy value (`sessions.rs:231`), and
`single_attachment: true` (`sessions.rs:238`); every bundle from `0.17.0` through `0.23.0`
constrains `attachment_lifetime_ms` to the constant `3600000` and `single_attachment` to `true` in
`schemas/results/pipe-session-capabilities.json`, and `cargo xtask check-bundle` pins the first
(`xtask/src/bundle.rs:1618`). The Rust SDK guide states the one-hour lifetime to readers
(`website/docs/guides/rust-sdk.md:312-313`). What no decision recorded was why the figure is one
hour, and that no consumer is meant to outlive it.

## Decision

1. **An attachment lives at most one hour** (3,600,000 ms), measured from the WebSocket upgrade.
   The figure is a daemon policy constant, published as `attachment_lifetime_ms` in the session
   capability; it is not a request field and no client can lengthen it.
2. **The end of an attachment ends the session's process tree.** Lifetime expiry, a client close
   or disconnect, a transport error, and every `protocol-error` the daemon answers each terminate
   the whole tree and persist the terminal observation, as ADR 0008 decides. An unproven kill is
   `unknown`, never assumed (invariant 3).
3. **The attachment right is one-shot.** It is consumed by the first claim and never made
   available again, by a new authority, a new request or a daemon restart (the
   `restart-never-redispatches-or-reattaches-a-pipe-session` vector). There is no re-attach and no
   replay. A client that loses its attachment starts a new session; on a network transport that
   also means a newly minted authority
   ([design 14](../docs/design/14-network-session-authority.md)).
4. **The attachment does not lengthen the process lease.** The session's lease is the bound exec
   lease (ADR 0008), renewed explicitly within `MIN_LEASE_TTL_MS` and `MAX_LEASE_TTL_MS`
   (`crates/substrate-wire/src/lib.rs:288-289`); whichever of the lease and the attachment lifetime
   ends first ends the tree.

**Consumer.** These bounds were chosen for bounded interactive and agent sessions: a protocol
process driven by a client for one task, or a human terminal held for one sitting, where a lost
client means the work is abandoned and a process left running without its client is a leak, not a
feature. Under that consumer, kill-on-loss is the containment guarantee, and one hour bounds what a
forgotten attachment can hold.

**Out of scope.** An interactive session meant to outlive one hour, or to survive a client
disconnect and be re-attached, is not served. Serving it is a new story and a contract change: a
detachable attachment state, a re-attach route or authority, a refusal for each new way it can
fail, and a successor bundle that relaxes the `attachment_lifetime_ms` constant. It is not reached
by changing the policy value, which every published bundle pins.

## Consequences

- A client reads the bound from the capability before it attaches and plans work that fits inside
  it; a longer task is several sessions, each with its own process.
- A daemon that changed the policy value without a successor bundle would publish a capability its
  own contract rejects; the bundle pin in `xtask/src/bundle.rs` keeps the figure and the contract
  together.
- [Design 05](../docs/design/05-streams-sessions-and-endpoints.md) states reconnect as a new
  session rather than a re-attach of the old one.
