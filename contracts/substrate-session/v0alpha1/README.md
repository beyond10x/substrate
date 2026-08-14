# Substrate raw-pipe session v0alpha1

**Classification: source-typed development contract.** This is not a released Substrate bundle or
a compatibility promise, and it does not alter the immutable 0.1.0 or 0.2.0 bytes.

The Rust source types `PipeSessionCapabilities`, `PipeSessionStartInput`, `PipeClientFrame`, and
`PipeServerFrame` define the closed first byte-plane vocabulary. Client frames carry ordered stdin,
half-close, or bounded signal intent. Server frames keep stdout and stderr distinct and carry
truncation, terminal observation, or protocol failure. The host implementation applies bounded
frames and queues and uses the existing bubblewrap, empty-environment, no-egress, cgroup-limit,
whole-tree kill, and terminal-observation path. It refuses when delegated confinement is
unavailable.

The development daemon now exposes `GET/POST /v1/pipe-sessions` and
`GET /v1/pipe-sessions/{exec_id}/attach`. Start durably reserves the mandatory leased underlying
exec before dispatch. Attachment is authenticated by the daemon's owner-permissioned Unix socket,
subject-scoped, single-use while live, bounded, and cancelled on loss or protocol failure. The exec
operation/store remains the durable resource authority; a distinct session identity and reconnect
are not claimed.

Agent independently copies and digest-pins this development shape and drives a model-free fake
app-server through a clean-room semantic server. The real delegated-cgroup cross-repository lane,
deterministic successor bundle, and owner-signed release remain required before
`substrate-confined` can be reported as executed conformance.

[`consumer-contract.md`](consumer-contract.md) is the exact reviewable byte copy used by Agent. Both
repositories pin SHA-256
`0d6a2f04e186b7ab0ccdf1111f5f1c59d03d1e6ec25321692cfafa4f183d5fd7`; drift fails their Rust tests.
