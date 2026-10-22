---
format: aep.planning-md/3
id: review-result:several-apertures-host-adversary-pass-2
kind: review-result
status: active
title: Adversarial pass on the several-apertures host mechanism
relations:
- reviews: story:several-apertures-per-run
revision: 1
---
## Adversarial pass on the several-apertures host mechanism (commit dab1ebdb7)

Verdict: NEEDS-CHANGE, two findings, both introduced by the unit, both fixed in the wire unit.

| # | Where | Finding | Fixed by |
|---|---|---|---|
| 1 | `crates/substrate-host/src/egress.rs:200` | the fixed `127.0.0.1 localhost` hosts line precedes the per-aperture lines, so a listed aperture at index 1 or later whose host is `localhost` reached the first aperture's destination | `exec.aperture-host-listed-twice` at admission and in `GeneratedResolution::prepare` |
| 2 | `crates/substrate-host/src/process.rs:1942` | `select_apertures(.., Some(&[]))` selected nothing, so a network `aperture` start ran with no network and no refusal | `select_apertures` refuses an empty listed set |

Held: all-or-nothing install (third install failing leaves no listener on 127.0.0.1-3),
independent byte ceilings under 4 concurrent relays per aperture, n-th aperture on 127.0.0.n, the
single-aperture hosts bytes unchanged.

Not tested: each forwarder and relay counts against the run's `pids.max`; the startup probe proves
127.0.0.1 only.
