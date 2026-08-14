# Copied Substrate session development contract

Source owner: `daemonloom/substrate` · source status: `substrate-session/v0alpha1` development

Control addresses consumed by Agent:

- `GET /v1/machine`
- `GET /v1/pipe-sessions`
- `POST /v1/pipe-sessions`
- `GET /v1/pipe-sessions/{exec_id}/attach` as `unix-websocket-json`
- `GET /v1/execs/{exec_id}`
- `POST /v1/execs/{exec_id}/signal`

Start input is a strict object containing `exec`, `input_limit_bytes`, `frame_limit_bytes`, and
`queued_frames`. The embedded exec is a no-wait, explicitly leased, required workspace sandbox with
network `none`, a capability snapshot, argv, shaped environment, and bounded timeout/output/
process/memory/CPU limits.

Client frames are strict tagged JSON objects with contiguous sequence numbers beginning at one:
`stdin(sequence, base64 content)`, `close-input(sequence)`, or
`signal(sequence, INT|TERM|KILL, grace_ms)`.

Server frames are strict tagged JSON objects with contiguous sequence numbers beginning at one:
`output(sequence, stdout|stderr, base64 content)`, `truncated(sequence, stdout|stderr)`,
`exit(sequence, exited|cancelled|expired|unknown, optional exit)`, or
`protocol-error(sequence, code, message)`.

Live output for each stream never exceeds the embedded exec output bound. Substrate continues
draining excess child output without forwarding it, records truncation, and sends `truncated`
before the terminal `exit`. An `exit` frame must carry a terminal state.

Attachment loss, protocol failure, send timeout, and lifetime expiry close the live output receiver
before whole-tree `KILL`; the attachment key is released only after durable terminal persistence.
If containment or persistence cannot be proven, reattachment remains refused and one of the fixed
process-local attachment slots stays consumed until daemon restart and recovery.

The capability response must name contract `substrate-session/v0alpha1`, transport
`unix-websocket-json`, the same machine capability snapshot, mandatory lease, single attachment,
network `none`, and nonzero maximum input/frame/queue bounds. Agent derives confinement only when
the machine observation also reports all namespaces, process/memory/CPU cgroup limits, cgroup kill,
no egress, explicit leases, and a nonzero output bound.

This copy is development compatibility evidence, not an owner-signed release. The Rust consumer
pins its SHA-256 in a test and rejects unknown server-frame fields/discriminators. No direct-process
fallback exists.
