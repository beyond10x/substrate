# Substrate raw-pipe session v0alpha1

**Classification: source-typed development contract.** This is not a released Substrate bundle or
a compatibility promise, and it does not alter the immutable 0.1.0 or 0.2.0 bytes.

The Rust source types `PipeSessionStartInput`, `PipeClientFrame`, and `PipeServerFrame` define the
closed first byte-plane vocabulary. Client frames carry ordered stdin, half-close, or bounded signal
intent. Server frames keep stdout and stderr distinct and carry truncation, terminal observation, or
protocol failure. The host implementation applies bounded frames and queues and uses the existing
bubblewrap, empty-environment, no-egress, cgroup-limit, whole-tree kill, and terminal-observation
path. It refuses when delegated confinement is unavailable.

The current slice is host-level only. A durable leased session resource, single-attachment Unix
socket route, replay/reconnect semantics, deterministic successor bundle, and independent consumer
remain required before Agent can call this a released `substrate-confined` backend.

The model-free compatibility fixture is intentionally a future cross-repository test: launch a fake
app-server, exchange JSONL over these pipes, apply backpressure, cancel its descendant tree, and
reconcile its terminal observation without a model, credential, or network.
