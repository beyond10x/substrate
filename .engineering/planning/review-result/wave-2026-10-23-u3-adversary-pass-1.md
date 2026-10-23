---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u3-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u3 (bundle 0.23.0 read-only roots)
relations:
- reviews: story:bundle-publishes-read-only-roots
revision: 1
---
# Adversary pass 1, unit u3 (bundle 0.23.0, read-only roots)

Agent type `aep:adversary`. Branch `unit/2026-10-23-u3-ro-roots-bundle`, working tree on `344f9c56`. Cases in `xtask/tests/read_only_roots_bundle_adversary.rs`. Verdict: NEEDS-CHANGE.

```findings
- {file: xtask/bundle-source/0.23.0/documents/vectors/http, line: 0, category: contract-drift, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "bundle 0.23.0 publishes no refusal vector for exec.read-only-root-invalid/-absent/-not-a-directory; ADR 0010:80 requires them (test bundle_publishes_a_vector_for_each_read_only_root_refusal)"}
- {file: contracts/substrate-wire/0.23.0/schemas/resource.json, line: 593, category: contract-drift, severity: warning, verdict: NEEDS-CHANGE, origin: pre-existing, message: "closed exec applied shapes omit read_only_files and secret_slots that AppliedConfinement emits; a roots+file record fails exec-start.json (test an_exec_record_binding_roots_and_files_validates_against_the_published_result; 0.22.0 also fails)"}
- {file: xtask/src/bundle/read_only_roots.rs, line: 131, category: test-strength, severity: note, verdict: NEEDS-CHANGE, origin: introduced, message: "reserved-mount check is one-directional; an extra reserved /opt passes check-bundle"}
- {file: xtask/src/bundle/read_only_roots.rs, line: 68, category: test-strength, severity: note, verdict: NEEDS-CHANGE, origin: introduced, message: "x-b10x-unique and the sibling-overlap sentinel can both be removed and check-bundle stays green"}
- {file: xtask/src/bundle/read_only_roots.rs, line: 351, category: test-strength, severity: note, verdict: NEEDS-CHANGE, origin: introduced, message: "x-b10x-absent-means checked for presence only; arbitrary content passes"}
- {file: crates/substrate-host/src/process.rs, line: 1708, category: maintainability, severity: note, verdict: CONFIRMED, origin: introduced, message: "host refusal codes are literals, not substrate_wire READ_ONLY_ROOT_* constants"}
- {file: crates/substrate-daemon/tests/writable_roots_contract_adversary.rs, line: 0, category: contract-drift, severity: note, verdict: CONFIRMED, origin: introduced, message: "still reads bundle 0.22.0 (already known to implementor)"}
```

Also INFEASIBLE (F7): the fact's absent-means names `workspace-openat2-unavailable`, which the roots refusal list does not hold; reachable only on ENOSYS after the fact is published.

Coordinator decisions: F1–F6 fixed in this successor (F2 corrects the 0.23.0 applied shapes; frozen bundles unchanged); F7 answered by making absent-means name exactly what the daemon raises; F8 accepted if the test checks 0.22.0 content.

Severities mapped to the store vocabulary from the adversary's report: medium to warning, low and info to note.
