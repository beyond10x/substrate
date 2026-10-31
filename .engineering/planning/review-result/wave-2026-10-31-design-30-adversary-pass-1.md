---
format: aep.planning-md/3
id: review-result:wave-2026-10-31-design-30-adversary-pass-1
kind: review-result
status: active
title: 'Wave 2026-10-31 (design 30): adversary pass 1'
relations:
- reviews: story:confinement-adapter-on-the-wire
- reviews: story:namespace-driver-admits-an-exec-on-observed-confinement
revision: 1
---
# Adversary review, wave 2026-10-31 (design 30), pass 1

Reviewer: aep:adversary, tree `substrate-w1031b-adv`, range `11f002ed...11042902`; cases
committed as `37fde4d1` on `review/w1031b-adv`.

```
unit: substrate wave 2026-10-31 (design 30), range 11f002ed...11042902 at 11042902; cases committed on review/w1031b-adv as 37fde4d1
verdict: NEEDS-CHANGE
cases: executed 113 to 118, red 4
origin: introduced 6 / pre-existing 0 / undecided 0
needs-coordinator: yes, A2 ranking and A4 null leniency
```

Suites, only the targets the cases live in:
- `cargo test -p b10x-substrate-kubernetes --lib`: `test result: FAILED. 114 passed; 1 failed` (A1); exit 101.
- `--test adversary_w1031_requires_shape`: `FAILED. 0 passed; 2 failed`; exit 101.
- `--test adversary_w1031_requires_driver_order`: `FAILED. 0 passed; 1 failed`; exit 101.

A5 is green now and fails against a scratch mutant where `read_run` returns the observation even
when the argv did not read back; the 17 existing probe tests stayed green against that mutant.

```findings
[
{"file":"crates/substrate-kubernetes/src/exec.rs","line":1492,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A1 the namespace driver's applied record writes requires None, so an admitted start never has its checked requires echoed; crates/substrate-kubernetes/src/exec_tests.rs:1856 adversary_w1031_the_applied_record_echoes_the_starts_requires"},
{"file":"crates/substrate-daemon/src/app/operations.rs","line":553,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A2 a duplicate, empty list or empty requires with a stale snapshot answers exec.capability-stale, while an unknown name answers request.schema-invalid; crates/substrate-daemon/tests/adversary_w1031_requires_shape.rs:110 adversary_w1031_a_requires_outside_the_schema_is_schema_invalid_even_with_a_stale_snapshot"},
{"file":"crates/substrate-host/src/process.rs","line":2421,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A3 the host driver checks requires after the read-only root, file, workspace-access and secret-slot checks, so it names a different refusal than the daemon; crates/substrate-host/tests/adversary_w1031_requires_driver_order.rs:73 adversary_w1031_the_driver_checks_requires_before_a_read_only_root"},
{"file":"crates/substrate-wire/src/lib.rs","line":3750,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A4 a null families or facts list, which the closed schema refuses, is read as absent and the start is checked further; crates/substrate-daemon/tests/adversary_w1031_requires_shape.rs:142 adversary_w1031_a_null_requires_list_is_schema_invalid"},
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":785,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A5 no test held the argv gate on the heartbeat and egress observation, and dropping it left the suite green; crates/substrate-kubernetes/src/namespace/probe_tests.rs:1334 adversary_w1031_an_expanded_argv_voids_the_heartbeat_and_the_egress_observation"},
{"file":"docs/design/30-confinement-adapter-on-the-wire.md","line":261,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A6 design 30 section 8 says loom is unaffected, but loom builds and exhaustively destructures AppliedConfinement, which the new requires field and the Option cgroup break when its pin moves"}
]
```

What held: requires checked after the stale check on the exec and session routes; families before
properties, each in published order; no released bundle file changed; the neutral and Linux
admission arms; host facts match `linux::proof`, `network-ingress` never published; the snapshot
binds the adapter; no adapter series on the namespace daemon; probe egress classification,
heartbeat equality and the lease/`valid_until` lapse.
