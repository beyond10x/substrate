---
format: aep.planning-md/3
id: review-result:wave-2026-11-05-substrate-u2-adversary-pass-1
kind: review-result
status: active
title: Wave 2026-11-05 substrate U2 adversary pass 1
relations:
- reviews: story:every-egress-reap-retries-through-eintr
revision: 1
---
# Adversary pass 1 — wave 2026-11-05 U2 (story:every-egress-reap-retries-through-eintr)

Diff `43c49c42...04e43ecd`, merged into the integration branch at `ea2d61d9`. Adversary commit
`b7b08150` on `adv/2026-11-05-u2`, merged at `63ce2ccc`. Agent `aep:adversary`. Header and
findings verbatim; prose trimmed to the measured facts.

```
verdict: CONFIRMED (two gaps in the suite; the implementation held against every attack)
cases: executed 47→49, red 0 on the tree (each new case is red on its mutant)
origin: introduced 1, pre-existing 1, undecided 0
```

Commands: `cargo test -p b10x-substrate-host --lib egress::` 49 passed, 11 ignored; fmt clean;
clippy `--lib --tests -D warnings` clean.

Mutants, each run on a scratch copy with the full egress suite:

| mutant | change | result |
|---|---|---|
| MA | `WIFEXITED` dropped | survived the unit's suite; killed by `a_probe_child_ended_by_a_signal_never_reads_as_reached` |
| MB | `None` after any EINTR | survived the unit's suite; killed by `an_interrupted_probe_reap_of_a_child_that_reached_reads_as_reached` |
| MC | the probe always answers false | killed by an existing case |
| MD | the drop reaps with one `waitpid` | killed by the unit's drop case |
| ME | a failed wait returns `Some` | killed by the unit's ECHILD case |

Held:

- `abandon_forwarder`'s behaviour is unchanged.
- No hook reaches a non-test build.
- "Reached" needs a real ended child.
- The drop ends on ECHILD.

Of the production calls, only `reap` and `listen_through_helper`'s loop wait, and that loop
already retries.

Not examined (scope out): `substrate-executor/src/run.rs:350` waits with `waitpid(-1)`.

Resolution: both findings are fixed by the cases merged at `63ce2ccc`.

```findings
[{"file":"crates/substrate-host/src/egress.rs","line":754,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"scope in; the unit's interrupted-probe case asserted only that an exit 1 reads as not reached, so a reap returning None after any EINTR passed all 47 unit cases; fixed by an_interrupted_probe_reap_of_a_child_that_reached_reads_as_reached (b7b08150)"},
 {"file":"crates/substrate-host/src/egress.rs","line":754,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"scope in; no case ended the probe child by a signal, so dropping WIFEXITED survived; fixed by a_probe_child_ended_by_a_signal_never_reads_as_reached (b7b08150)"}]
```
