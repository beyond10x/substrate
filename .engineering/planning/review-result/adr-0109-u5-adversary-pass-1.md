---
format: aep.planning-md/3
id: review-result:adr-0109-u5-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:systemd-scope-bounds-provider
relations:
- reviews: story:systemd-scope-bounds-provider
revision: 1
---
# Adversary pass 1: story:systemd-scope-bounds-provider

Verdict: needs change. Read at the unit head b0d11fa (integration 1c6d2ec); 8 cases added in
`crates/substrate-host/tests/adversary_systemd.rs`, 5 red; each case runs in a fresh process of
the test binary.

## Findings

- F1 `crates/substrate-host/src/confinement/mod.rs:483` (warning): a systemd profile without its
  inner is admitted; the model refuses it `confinement.adapter-unserved`. Same defect as S0
  adversary finding 4.
- F2 `systemd.rs:590` (warning): `drain_tree` reads only the run cgroup's own `cgroup.procs`, so a
  run that made a nested cgroup is waited the full 10 s, reported Ended, and its cgroup is left.
- F3 `systemd.rs:883` (warning): a leader that moves itself out through `systemd-run --user
  --scope` (given `XDG_RUNTIME_DIR`) outlives its 500 ms timeout and `wait` never returns.
- F4 `systemd.rs:792` (blocker): timeout and CPU budget live only in an embedder task; a run whose
  embedder exits lives on past its timeout in a scope no later embedder cleans up.
- F5 `systemd.rs:286` (warning): opening systemd moves the embedder out of an operator-delegated
  root; `linux` opened afterwards with that root serves only workspace and audit.
- F6 `systemd.rs:614` (note): no unit case exercises `memory.max` or `memory.oom.group`.

## Checked and not broken

Fork past `pids.max`; a background grandchild ends with its leader; swap 0; an already-open linux
adapter keeps running; no `INVOCATION_ID` or bus address in the run; unreachable bus refused;
inner mismatch, filesystem view and `host_network` refusals; a second `systemd{linux}` refused;
bounds outside range refused and audited; the lane fails when the scenario count is absent.

## Decisions taken by the coordinator

F1: fixed in `decide` by the S0 fix pass; the systemd test sets `inner`. F2: drain on
`cgroup.events` `populated 0` and remove the subtree depth-first. F3: the security pass's refusal
of `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` closes the path the case uses; `wait` also
bounds the reader wait and reports `Unknown`, and the module doc states the case. F4: the leader
gets `PR_SET_PDEATHSIG(SIGKILL)`; `open` kills and stops leftover `substrate-confinement-*` scopes
whose embedder is gone and reports how many; the doc states the interval until the next open. F5:
the result is a named refusal (`family-unserved limits`); the module doc says an embedder that opened
systemd uses `systemd{inner: linux}`, and case 5b asserts the named refusal. F6: the case stays.
