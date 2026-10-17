---
format: aep.planning-md/3
id: approval-record:wave-2026-10-17-adopt-hostproc
kind: approval-record
status: active
title: 'Wave 2026-10-17: the adopt route and host-process supervision'
relations:
- decides: story:exec-in-an-adopted-directory
- decides: story:host-process-start
- decides: story:host-process-cgroup-tree
- decides: story:host-process-lease
- decides: story:host-process-stop-by-owner
- decides: story:host-process-listing
- decides: story:host-process-logs
- decides: story:host-process-deadlines
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-10-17T05:05:39Z", actor: "human:timo", revision: 2}
---
# Wave 2026-10-17: the adopt route and host-process supervision

Integration branch `wave/2026-10-17-adopt-hostproc`, base `3f01d8c8` (main) plus the merge of
`adr/host-process-family`; one full gate on the final head, one pull request, then a release.
Approved by the operator's acceptance of ADR 0035 and ADR 0036 on 2026-10-17.

Width: 5 units. The host-process HTTP routes, wire types and their successor bundle wait for the
finished host-process specification (u3), because their wire types are generated from it; S8, S9
and S12 follow in later waves as ADR 0036 orders them.

| unit | story | tree branch | lands on |
|---|---|---|---|
| u1 | `story:exec-in-an-adopted-directory` (ADR 0035): ESS `spec/workspaces/`, store `origin`, host adopt observation, daemon `workspace.adopt`, destroy forgets, SDK, runtime vector | `unit/2026-10-17-u1-adopt` | `spec/workspaces/`, `crates/substrate-{store,host,daemon,sdk,wire}` (types only) |
| u2 | bundle `0.19.0`: `workspace.adopt` route, workspace resource `origin` and `root_name` | `unit/2026-10-17-u2-bundle` | `xtask/bundle-source/0.19.0/`, `contracts/substrate-wire/0.19.0/`, `scripts/gate.sh`, the advertised bundle pair |
| u3 | host-process ESS: the draft made final with the points below | `unit/2026-10-17-u3-hostproc-spec` | `spec/host-processes/`, `spec/` index |
| u4 | S2, S3, S7, S11 host side: unconfined spawn in its own cgroup, tree kill, deadlines, bounded log, CPU/RSS/port observation | `unit/2026-10-17-u4-hostproc-host` | `crates/substrate-host/src/host_process/` |
| u5 | S1, S4, S5, S6 records: store tables, lease and keep, owner-name uniqueness, end cause, stop-by-owner selection, retention | `unit/2026-10-17-u5-hostproc-store` | `crates/substrate-store/` (new module, one migration) |

u4 and u5 are internal crates with no generated surface; they implement against design 26 and the
specification draft, and u3's final specification is checked against them at integration.

Reviews beside the build: an adversary on u1 (adopt refusals, destroy never removes an adopted
directory, cross-subject oracle); a security review on u4 (unconfined spawn, cgroup containment,
tree kill, log path).

## Spec points design 26 left unmapped

Each takes the option that can be undone: narrowest surface, additive, default off, no persisted
format committed. Where design 26 recommends one, it is taken.

| # | point | option | why |
|---|---|---|---|
| 1 | stream delivery | at least once, each item carrying a per-process `seq` strictly increasing; a reader drops a repeat | design 26 § event stream; exactly once is not expressible and adds nothing a `seq` cannot |
| 2 | refused append | drop; the only refusal is an append after `exited` | nothing else can refuse an append; a later policy is additive |
| 3 | profile for container and adopted records | none chosen now; S8 and S9 are not in this wave | decided in the S8/S9 wave; nothing is built that commits to it |
| 4 | lease expiry instant | computed by the daemon as the start or renewal instant plus the TTL | ESS cannot add a duration to an instant; the instant is an observation, no format change |
| 5 | record removal | no removal operation; ended records are retained for a bounded time and count (design 26 § logs), their stream with them | narrowest surface; a removal route is additive |
| 6 | who may invoke what | the subject is the only authority; owner session is a label beneath it (design 26 § owners) | the design's recommendation; narrowing to the owner session later is additive |
| 7 | name uniqueness | owner name unique among the subject's non-terminal records of the same owner session (design 26 § start); records addressed by id | the design's rule; reuse after a record ends stays possible |
| 8 | lease expiry trigger | the daemon's supervisor clock invokes the expiry operation | ESS has no clock-driven transition; implementation-side only |
| 9 | timeout and expires-at trigger | the same supervisor clock; `ended_by` names which deadline | as 8 |
| 10 | adopted supervision (S9) | not built in this wave; brought back before S9 is built if no undoable option exists | no option is chosen that a later wave cannot change |
| 11 | container label keys | the three labels design 26 § containers names, in Substrate's label prefix; S8 later | the design's recommendation |
| 12 | container timeout | not offered | narrowest; adding it is additive |
| 13 | CPU and cadence | cumulative CPU microseconds read at observation time; no background sampling | as exec usage reports; sampling is additive |
| 14 | log bound | bytes; at the bound the oldest bytes are discarded and the log states how many (design 26 § logs); one wire constant holds the value | the design's recommendation; the log file format stays internal |
| 15 | append for an unknown process | refused inside the daemon; not on the wire | no caller can append; no surface |
| 16 | cursor monotonicity | the daemon assigns the per-process `seq`; the cursor is opaque on the wire; the ordering scenario checks it | design 26 § event stream; an opaque cursor commits no format |

## Specification choices made in the host-process unit (each can be undone)

Taken while aligning `spec/host-processes/` with ADR 0036; none commits a persisted format.

| choice | why |
|---|---|
| `EnforceDeadline` takes a `deadline` input (`timeout` or `expires-at`) and refuses one the record does not carry | ESS cannot pick the earlier of two instants; the daemon computes `deadline_at` and names which one fired |
| changing a keep to a TTL lease clears `keep_reason` | a TTL record has no keep to justify |
| a vanished record's end carries its stored, empty exit code and signal | nothing was observed |
| `EndedByMissing` refuses an `ended` stream item with no cause | every stream ends with exactly one named cause |
| `stopped` is emitted when a stop of any cause finishes, before `ended` | the ADR does not say which moment; finishing is the observable one |
| the S12 `gap` item and cursor decoding stay in the implementation | the model cannot express a log byte offset |
| `env_digest` is set by the daemon at start, not sent by the client | S1: environment values are never recorded |

## Reviews

- `review-result:adversary-adopt-u1-pass-1`: two blockers and a race, fixed.
- `review-result:security-hostproc-u4-pass-1`: one blocker and three warnings, fixed.
