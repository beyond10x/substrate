---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u3-adversary-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u3 (bundle 0.23.0 read-only roots)
relations:
- reviews: story:bundle-publishes-read-only-roots
revision: 1
---
# Adversary pass 2, unit u3 (bundle 0.23.0, read-only roots)

Agent type `aep:adversary`. Branch `unit/2026-10-23-u3-ro-roots-bundle`, working tree on `344f9c56` after correction round 1. Cases in `xtask/tests/read_only_roots_bundle_adversary_2.rs` and `crates/substrate-daemon/tests/read_only_roots_adversary_2.rs`. Verdict: red, 5 cases.

```findings
- file: xtask/src/bundle/read_only_roots.rs
  line: 449
  category: checker-strength
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: check_applied_record checks only that the APPLIED_MEMBERS keys exist, so check-bundle accepts secret_slots false, read_only_files maxItems 1, a slot fd maximum of 3, and an expired branch forbidding writable_roots/workdir (red cases 1-3)
- file: xtask/bundle-source/0.23.0/documents/compatibility.json
  line: 21
  category: contract-text
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the text says exec.read-only-root-invalid is checked before any capability, but validate_exec_input runs check_read_only_roots after the other start checks, so 5 roots plus 1 file without exec.read-only-files gets 501 exec.read-only-files-unserved (red case 5)
- file: xtask/src/bundle/read_only_roots.rs
  line: 449
  category: checker-strength
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: check_refusal_vectors does not hold a vector's message to the daemon's; a changed message renders and checks green (red case 4)
- file: xtask/bundle-source/0.23.0/documents/compatibility.json
  line: 21
  category: contract-text
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: secret_slots dates from b5cbb2ad (bundle 0.5.0), so the rejected span is 0.5.0 to 0.22.0 for slots, not 0.18.0 to 0.22.0
- file: xtask/bundle-source/0.23.0/documents/schemas/capability.json
  line: 0
  category: contract-text
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: absent-means omits that a lexically invalid list is 422 exec.read-only-root-invalid even when the fact is absent; sibling facts use the same wording
```

Coordinator decisions: 1 fixed by validating a wire-serialized record at every bound per state; 2 fixed by correcting the text to the daemon's order and pinning that order in case 5; 3 fixed by holding messages to one source; 4 fixed in the text; 5 accepted, no change.
