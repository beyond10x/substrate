---
status: accepted
date: 2026-08-14
---

# ADR 0008: pipe sessions have distinct durable identity

## Context

The first raw-pipe daemon slice used the underlying `ex_…` identity in a route named
`pipe-sessions`. That proved the byte path, but it collapsed two different lifecycles. An exec is
the confined process observation. A session additionally owns mode, byte limits, attachment
availability, attachment consumption, and the client-visible lease/reconciliation surface.

Process-local attachment state is not sufficient contract authority. A lost response, upgrade
failure, daemon restart, or concurrent attach must not make a consumed attachment available again
or leave the public session contradicting its underlying exec.

## Decision

Every pipe start durably creates a `ses_…` session and one bound `ex_…` exec in the same transaction
as the accepted `session.start` operation. The operation resource is the session. A session records
its mode, exec, workspace, capability snapshot, admitted input/frame/queue limits, lifecycle state,
attachment state, lease observation, and terminal exit observation.

The v1 pipe lifecycle is closed:

```text
accepted -> ready -> attached -> exited | cancelled | expired | unknown
                   \----------> cancelled | expired | unknown
ready -------------------------> exited | cancelled | expired | unknown
```

Attachment state is `pending`, `available`, `attached`, `consumed`, or `uncertain`. Claiming the
single attachment atomically changes `ready/available` to `attached/attached` before WebSocket
upgrade. The claim is never made available again. Upgrade failure, disconnect, protocol failure,
send timeout, or lifetime expiry triggers whole-tree cancellation and terminal persistence. If
containment cannot be proved, the session becomes `unknown/uncertain` and remains non-attachable.

The public session lease is enforced by the one bound exec lease. The same lease observation is
projected on both resources, but only one durable lease clock/cleanup row exists. Renewal updates
both projections atomically. Lease expiry kills the exec tree and terminalizes the session as
`expired`; it never creates a second independent timer.

Exec and session terminal observations are committed together. A session reports `exited` or
`cancelled` only after the host driver has reconciled the cgroup and the exec has that terminal
state. Restart changes unproved nonterminal exec/session pairs to `unknown` without redispatch.
Retiring a terminal session atomically retires its bound exec and the session. Direct exec
retirement refuses while a session owns that exec.

The development control surface is:

- `GET /v1/pipe-sessions` — inspected pipe capability;
- `POST /v1/pipe-sessions` — keyed session start;
- `GET /v1/pipe-sessions/{session_id}` — observed session;
- `DELETE /v1/pipe-sessions/{session_id}` — keyed terminal retirement;
- `POST /v1/pipe-sessions/{session_id}/signal` — keyed cancellation/signal;
- `POST /v1/pipe-sessions/{session_id}/lease/renew` — keyed lease renewal;
- `GET /v1/pipe-sessions/{session_id}/attach` — direct single attachment.

There is no separate current-session quota in this slice: every session owns exactly one exec and
is bounded by the durable exec quota. Concurrent attachments have their own fixed process-local
limit. PTY, reconnect, network authority redemption, secret slots, and egress remain separate
contracts.

## Consequences

- Process identity and byte-channel identity can evolve independently without ambiguity.
- Replay returns the same session/exec pair; it never starts or attaches a second process.
- Session consumers no longer infer durable attachment state from a daemon-local set.
- Store paths must preserve pairwise state across start, signal, natural exit, lease expiry,
  restart, and retirement.
- The successor bundle and clean-room consumer can bind every frame and terminal observation to a
  session while still checking the exact underlying exec confinement.
