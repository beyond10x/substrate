---
format: aep.planning-md/3
id: review-result:adr-0109-u5-security-pass-1
kind: review-result
status: active
title: Security review pass 1 on story:systemd-scope-bounds-provider
relations:
- reviews: story:systemd-scope-bounds-provider
revision: 1
---
# Security review pass 1: story:systemd-scope-bounds-provider

Verdict: needs change. Read at the unit head b0d11fa (integration 1c6d2ec); 6 cases added in
`crates/substrate-host/tests/security_review_systemd.rs`, 3 red.

## Findings

- F1 `crates/substrate-host/src/confinement/systemd.rs:493` (warning): around `local`, a run given
  the session bus in `Command.env` asks the user manager (`systemd-run --user --scope`) to move a
  child out of its cgroup; the child survives KILL and wait and escapes pids, memory, timeout and
  CPU bounds while `whole_tree_kill` is proven.
- F2 `systemd.rs:222` (warning): opening the adapter moves the embedder out of the unit it was
  started under into a scope with `TasksMax=infinity` and no `Slice=` or `MemoryMax`, for the rest
  of its life.
- F3 `systemd.rs:178` (note, infeasible today): `session_bus` is not checked to be a `unix:path=`
  address, so a `unixexec:` address runs a program at open.

## Verified to hold

`busctl` and `systemctl` argv-only from fixed paths with a cleared environment; unit names from a
ULID; the local policy, reason and `declared_insecure` survive the wrapper; admission refusals on
both inner variants audited; the linux floor holds inside the scope (no user bus, no scope
creation, no cgroupfs, no nested user namespace, only `lo`); `probe_cgroup` checks unchanged.

## Decisions taken by the coordinator

F1: a systemd-around-local run is refused `exec.environment-refused` for
`DBUS_SESSION_BUS_ADDRESS` and `XDG_RUNTIME_DIR`, and the module documents the limit of the fact.
F2: the scope carries the enclosing unit's `TasksMax` and `MemoryMax`, or the open is refused
`confinement.adapter-unserved` when they cannot be read. F3: only `unix:path=` addresses are
accepted; anything else is refused `confinement.adapter-unserved`.
