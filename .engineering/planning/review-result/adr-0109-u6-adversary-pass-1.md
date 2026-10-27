---
format: aep.planning-md/3
id: review-result:adr-0109-u6-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:oci-adapter
relations:
- reviews: story:oci-adapter
revision: 1
---
# Adversary pass 1: story:oci-adapter

Verdict: needs change. Read at the unit head bf4d486 (integration 15cbdba); 6 cases added in
`crates/substrate-host/tests/adversary_oci.rs`, 6 red; lane run against Docker 29.7.2.

## Findings

1. `crates/substrate-host/src/confinement/oci.rs:625` (blocker as filed): a `--network none`
   container connects to a host Unix socket beneath a read-only root; `:ro` does not restrict
   `connect(2)`.
2. `oci.rs:759` (warning): without `--init` the command is PID 1, so `signal(Term)` returns Ok and
   does not end a run that `local` ends with signal 15.
3. `oci.rs:1051` (warning): a signal the adapter did not send (an OOM kill at the memory bound) is
   reported as code 137 with no signal, where `local` and `linux` report signal 9.
4. `oci.rs:1049` (warning): a container the runtime never created (exit 125) is reported Ended
   with code 125 after `run_started`, with no named refusal.
5. `oci.rs:691` (note): refusals at start or render write no audit record.
6. `oci.rs:731` (note): `/dev/shm` is writable though it is no declared mount.
7. `oci.rs:759` (note): without `--pull never` the runtime contacts a registry inside the run.
8. `.github/workflows/gate.yml:115` (note): the lane report and summary do not name the oci lane.

## Checked and not broken

Argv injection through image, env, workdir, mounts and argv; image forms other than
`name@sha256:<64 lowercase hex>`; supplementary groups, `CapEff 0`, seccomp mode 2, read-only root;
pids, memory, swap and CPU bounds; kill, timeout and wait remove the container; ULID names keep two
adapters apart; canonical workspace and workdir; a missing runtime fails the CI lane.

## Decisions taken by the coordinator

Finding 1: socket reach is what the `ipc_socket_allowlist` fact states (`spec/confinement/confinement.yaml:38`);
`oci` does not prove it, so a profile that needs it is refused `confinement.fact-unserved`. The case
is re-pointed to assert that refusal, and design 04 records that a root's sockets are reachable
under `oci`; the runtime's own socket stays refused by name (security F4). Findings 2–8 are fixed
in the adapter: `--init`, `inspect` before removal, `create` then `start` with a named refusal for a
failed create, start-time audit as in S0, `--ipc none`, `--pull never` with a named refusal for an
absent image, and the oci lane in the CI lane report.
