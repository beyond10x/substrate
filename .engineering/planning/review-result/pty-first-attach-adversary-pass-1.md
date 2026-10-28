---
format: aep.planning-md/3
id: review-result:pty-first-attach-adversary-pass-1
kind: review-result
status: active
title: Adversary review of the lease sweep clock fix, pass 1
relations:
- reviews: story:pty-session-refuses-first-attach-under-load
revision: 1
---
# Adversary review of the lease sweep clock fix (PR 185)

Reviewed `fix/pty-first-attach` at `75fb06ed` (base `cbf4fd88`), 2026-10-28.

| id | severity | where | finding | outcome |
|---|---|---|---|---|
| F1 | S3, pre-existing | `crates/substrate-store/src/leases.rs` renewals; clock read in `sessions.rs` and `execs.rs` before the transaction | Session and exec renewals judged the stored lease by a clock read before the store transaction, with no workspace lock. A renewal that read its clock before another renewal committed was refused `lease.expired` (409, not retriable) on an active lease. No child was killed. Red case `concurrent_renewals_refuse_a_live_lease`, red on head and on base. | fixed: both renewals read the clock inside their immediate transaction; a reading that is absent there is `lease.clock-unavailable` (501), as before |

Attacked and not broken:

- every lease write takes its clock before the write and holds the store's one connection, so the claim's in-transaction reading is never older than a lease it sees;
- the new claim cannot miss a due lease: its reading is never earlier than the sweep clock, and an absent reading skips one round instead of ending the sweep;
- `admit_workspace` and the workspace-lease checks at exec, session and workspace paths read their clock after the workspace lock that every workspace-lease write holds;
- the expiry `observed_at` is never earlier than the sweep clock or the lease issue, and no vector pins it;
- the PR's own case fails on the base sources with the load run's signature.
