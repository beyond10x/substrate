---
format: aep.planning-md/3
id: review-result:wave-2026-11-01-d354-u4-adversary-pass-1
kind: review-result
status: active
title: D354 U4 adversary pass 1
relations:
- reviews: story:pod-sweep-replays-from-the-policys-create
revision: 1
---
# Adversary pass 1 — wave D354 U4 (story:pod-sweep-replays-from-the-policys-create)

Diff `91c8163a...4ef457b8`; adversary commit `21de4b9b` on `unit/d354-u4`, merged into the integration branch at `7b8c1dd9`. Agent `aep:adversary`. Header and findings verbatim; prose trimmed to the measured facts.

```
verdict: green
cases: executed 38→43, red 0
origin: introduced 2, pre-existing 1, undecided 0 (adversary)
```

Commands, each after the wave pre-check: `cargo test -p b10x-substrate-host --lib confinement::pod::cleanup_tests` 43/0; host --lib 411/0; fmt --check clean. Peak target/ 904M, lowest / 83G.

Cases committed (`crates/substrate-host/src/confinement/pod_cleanup_never_created.rs`, `mod d354_adv`):

- `one_entry_without_a_readable_time_is_kept`: an absent, null, `0`, `"yesterday"` or empty time keeps the policy.
- `managed_fields_that_is_not_a_list_is_kept`: an object, string or number keeps it.
- `managed_fields_empty_or_null_is_swept_as_before`: `[]` or `null` is swept, as with no `managedFields`.
- `one_entry_time_is_compared_as_an_instant`: the create's instant in another offset is swept, one second later kept; catches a mutant that compares the times as text.
- `a_written_policy_on_every_page_is_kept`: a two-page list keeps both.

An uncommitted probe (`Watch::Terminated` with two `managedFields` entries, asserting a delete) failed with "PROBE kept" at `pod_cleanup_never_created.rs:458`, exit 101: the basis of finding 1.

```findings
[{"file": "crates/substrate-host/src/confinement/pod.rs", "line": 571, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope out; case none (uncommitted probe failed): a policy written after its create gets a trace with no version, so a replay that would show its run's container terminated is never asked and the policy, swept as stopped before this diff, is now kept unchecked."}, {"file": "crates/substrate-host/src/confinement/pod.rs", "line": 483, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: the kept written policy is logged with the not-shown-stopped warn as the Acceptance asks, but its why is 'no resourceVersion of the run was read to watch from' (gone.rs:298) rather than the write after create, and the info line at pod.rs:568 fires on every open sweep for every written policy, young or present ones included."}, {"file": "docs/design/28-pod-confinement-adapter.md", "line": 275, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "scope out; case none: a write that clears managedFields or only sets deletionTimestamp moves resourceVersion without a shown write, so the never-created rule still relies on the adapter being the only writer there; inferred from Kubernetes semantics, not observed, and accepted by the coordinator decision."}]
```

Resolution: no defect against the Acceptance; the kept policy is kept and logged as it requires. Findings 1 and 2 (a written policy is no longer checked for a stopped container; the warn's reason text) and the pre-existing finding 3 are decided at wave close.
