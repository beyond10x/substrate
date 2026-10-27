---
format: aep.planning-md/3
id: review-result:adr-0109-fu15-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: egress listener fix and reaper follow-up fixes'
relations:
- reviews: story:egress-third-install-case-holds-under-load
- reviews: story:confine-run-reaps-killed-groups
revision: 1
---
# Security review pass 1: egress listener fix and reaper follow-up fixes

Verdict: nothing found; 4 green cases, 2 notes. Read at `5677d620` (targets `0a7b78d0`, `53b28582`,
`bfdf7873`); cases on branch `review/adr-0109-fu-egress-security` at `7ce13d87`. Executed 13 to 17,
0 red.

## Findings

- N1 `crates/substrate-host/src/egress.rs:618` (note, infeasible): `listen_inside` dropped a
  received listener without shutdown when the helper exited non-zero after sending or `waitpid`
  failed.
- N2 `egress.rs:451` (note, confirmed): with the installing process holding the listener for the
  run, a forwarder that dies mid-run (for example the run cgroup's OOM killer) leaves inside connects
  hanging instead of refused. Liveness only; no route opens.

## Verified to hold

The listener is `MSG_CMSG_CLOEXEC`; forwarder, helper and probe children close every other
descriptor before use; a second run started meanwhile receives no descriptor beyond stdio; the
`Listener::drop` unsafe block is sound; destination pinning, byte ceilings and counters are
unchanged. Reaper: a zombie owner (Z or X) counted as gone can only reach `holds_process`, so a
populated group is kept (case `reaper_review::review_delegated_a_zombie_owners_group_holding_a_process_is_kept_and_named`);
under `--output json` stderr carries exactly one document after an admission refusal and after
SIGINT, with `left_groups` in `error.data`; `left_groups` reasons are fixed strings with a pid and
an `io::Error` text; exit codes 0, 2 and 130 unchanged.

## Outcomes

- N1 and N2 fixed in `c3119cf6` (same change as the adversary's F2 and F1).
- The egress case now asserts the host holds no listener during the run, and moved into
  `tests/security_review_confinement.rs` (`e77afe8d`); the four cases' `executed` lines are required
  by `scripts/delegated-lane.sh` (`1dbac9b0`). Delegated lane EXIT=0. Merged as `633106d7`.
