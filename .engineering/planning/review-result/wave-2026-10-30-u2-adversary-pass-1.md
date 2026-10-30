---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u2-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u2 (substrate_kubernetes_collisions_total)
relations:
- reviews: story:kubernetes-collisions-metric
revision: 1
---
# Adversary pass 1, unit u2 (substrate_kubernetes_collisions_total)

Agent type `aep:adversary`. Tree on integration head `1403e5cc8` (unit commit `31cc66e96`). Four
cases committed (`281d7fcc4`, merged as `688e362b`): two in `crates/substrate-kubernetes`
(`workspace_tests.rs`, `exec_tests.rs`), red; two in
`crates/substrate-daemon/tests/collisions_metric_adversary.rs` on the deployment guide, red at the
tree's base and green against `ae938d997`. Verdict: red.

```findings
- file: crates/substrate-kubernetes/src/namespace/workspace.rs
  line: 492
  category: correctness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: One foreign object is counted again on every resolve, and the 250 ms provisional loop re-observes an unknown workspace.create, so the series counts encounters (about 4 per second per object) while its help text and the port doc say objects.
- file: crates/substrate-kubernetes/src/exec.rs
  line: 392
  category: correctness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Exec start meeting a foreign Pod under the exec Pod's name logs it and leaves it untouched but never calls collision(), so the published series does not count the case it exists for.
- file: website/docs/guides/deployment.md
  line: 151
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The guide caps a scrape at 582 samples and does not name substrate_kubernetes_collisions_total, while a Kubernetes daemon serves 583.
- file: crates/substrate-kubernetes/src/namespace/workspace_tests.rs
  line: 845
  category: test-gap
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The acceptance asks for the scrape to show a real collision; the proof is split between a fake-API count and a scrape built from a stubbed count, because design 34 section 10.1 keeps the fake API out of the daemon tests.
- file: crates/substrate-kubernetes/src/namespace/workspace.rs
  line: 457
  category: correctness
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: A bind_uid refusal after a successful create is treated as a collision but never counted; no reaching path found.
```

Held: exposition format (HELP and TYPE before the sample, no labels); a host daemon emits no line
for the series; the counter is one `AtomicU64` that nothing resets; a Kubernetes scrape stays at 583.

Coordinator decision: the series counts objects, each once, keyed on kind, name and uid in a bounded
set; the exec-start arm counts through the same path (fix unit on
`impl/kubernetes-collisions-metric-fix`). The guide finding was already fixed by `ae938d997`. The
split proof is accepted (design 34 section 10.1). The bind arm stays as is.
