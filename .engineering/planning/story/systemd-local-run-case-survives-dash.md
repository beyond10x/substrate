---
format: aep.planning-md/3
id: story:systemd-local-run-case-survives-dash
kind: story
status: draft
title: The systemd local-run lane case holds its leader under dash (#230)
revision: 2
---
# The systemd local-run lane case holds its leader under dash (#230)

Issue #230: on a Debian-family host with a reachable user manager, `bash scripts/delegated-lane.sh`
failed 1 of 19 at `crates/substrate-host/tests/ess_confinement_systemd.rs:183`, `pids.max`
NotFound. A test defect, not an adapter or host defect: the run's `/bin/sh` script forked 32
`sleep 60 &` in the leader under `processes: 8`, and a fork refused at `pids.max` is fatal to a
non-interactive dash (`Cannot fork`, exit 2; bash retries and continues). The leader exited, the
adapter killed the tree and removed the run's cgroup as it promises, and the test read a removed
directory. No contract or capability change, so no ADR or design document (invariant 8).

## Outcome

The script forks in a subshell and the leader waits on a sleep started before the forks, so the
refusal ends only the subshell under any POSIX `sh`; `pids.max`, `pids.events max`, the running
state and the kill are observed as the case intends.

## Acceptance

- Fails first: under dash the old script exits 2 at the first refused fork, and the new script's
  leader survives it (`ulimit -p 1` in the subshell, recorded in the pull request).
- The delegated lane on a host whose `/bin/sh` is dash runs the case green (operator host;
  this box has no user manager, where the case is ABSENT).
- Spec: no ESS scenario changes; the suite's scenario is unchanged, only the test's workload.
- Full gate green; the pull request closes #230.
