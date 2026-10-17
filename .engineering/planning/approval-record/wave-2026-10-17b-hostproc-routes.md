---
format: aep.planning-md/3
id: approval-record:wave-2026-10-17b-hostproc-routes
kind: approval-record
status: draft
title: 'Wave 2026-10-17b: the adopt runtime vector and the host-process routes'
relations:
- decides: story:exec-in-an-adopted-directory
- decides: story:host-process-start
- decides: story:host-process-cgroup-tree
- decides: story:host-process-lease
- decides: story:host-process-stop-by-owner
- decides: story:host-process-listing
- decides: story:host-process-logs
- decides: story:host-process-deadlines
revision: 1
---
# Wave 2026-10-17b: the adopt runtime vector and the host-process routes

Integration branch `wave/2026-10-17-hostproc-routes`, base `00fa634c` (main, release 0.7.17); one
full gate on the final head, one pull request, then a release. Approved with the operator's
acceptance of ADR 0035 and ADR 0036 on 2026-10-17.

Width: 4 units, 2 at the start. u3 and u4 consume the host-process wire types that u2 generates from
`spec/host-processes/`; they branch from u2's first commit (the types) instead of writing a parallel
model, and start when it lands. S8, S9 and S12 follow in later waves as ADR 0036 orders them.

| unit | story | tree branch | lands on |
|---|---|---|---|
| u1 | `story:exec-in-an-adopted-directory` acceptance lines 2 and 3: runtime vector through the shipped daemon (an un-prefixed directory adopted, an exec started in it, output and exit observed; a name outside the root-name set refused before dispatch with its named code), and the delegated isolation cases run again against an adopted workspace | `unit/2026-10-17b-u1-adopt-vector` | `crates/substrate-daemon/tests/runtime_vectors.rs`, the case counts where the repository states them |
| u2 | host-process wire types generated from `spec/host-processes/`, then successor bundle `0.20.0` (routes, schemas, facts, refusals of S1-S7 and S11) | `unit/2026-10-17b-u2-wire` | `crates/substrate-wire/`, `xtask/src/ess.rs`, `xtask/bundle-source/0.20.0/`, `contracts/substrate-wire/0.20.0/`, `scripts/gate.sh`, the advertised bundle pair |
| u3 | daemon routes `POST/GET /v1/processes`, `GET /v1/processes/{id}`, `.../stop`, `POST /v1/processes/stop`, `.../lease/renew`, `.../logs`; the supervisor clock that ends records at lease expiry, timeout and expires-at; restart reconcile | `unit/2026-10-17b-u3-daemon` | `crates/substrate-daemon/src/app/` (new processes module), store wiring, host supervisor wiring |
| u4 | SDK calls and `b10x-substrate proc` client commands over them | `unit/2026-10-17b-u4-client` | `crates/substrate-sdk/`, the client crate, `spec/client/`, `spec/generated/cli` |

The sixteen spec points left unmapped by design 26 keep the options recorded on
`approval-record:wave-2026-10-17-adopt-hostproc`; this wave builds nothing that changes one.

Reviews beside the build: an adversary on u1 (adopted-workspace isolation, refusal before
dispatch); a security review on u3 (posture and peer-uid admission, foreign subject, an unserved
host profile never degrading, lease and deadline enforcement).
