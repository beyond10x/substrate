---
format: aep.planning-md/3
id: story:host-probe-socket-fits-sun-len
kind: story
status: implemented
title: The host probe's socket fits sun_path under any TMPDIR
relations:
- decomposes: epic:confinement-contract-family
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T05:06:26Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T05:06:26Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-29T12:18:57Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: the host probe's socket fits sun_path under any TMPDIR

## Observation

Fix unit G of `wave/k8s-namespace-1` ran the host crate's lib tests under a TMPDIR of 100 bytes on
2026-10-29. The production probe binds `host.sock` under TMPDIR
(`crates/substrate-host/src/probe.rs:520`); the path exceeds the platform's `sun_path` limit, the
probe fails, and the daemon withholds the confinement floor. Two host lib tests fail under that
TMPDIR: `a_backend_that_cannot_disable_nested_user_namespaces…` and
`every_probe_sandbox_carries…`. Under a short TMPDIR both pass.

Withholding the floor is the named-refusal path (invariant 3), so nothing runs unconfined; but a
daemon started with a long TMPDIR serves no confined exec at all, for a reason unrelated to the
host's capabilities.

## Acceptance

- The probe places its socket on a path whose length is checked against the platform limit (108
  bytes on Linux, 104 on macOS, NUL included) before binding, in a directory the daemon owns and
  that is private to it (for example a short directory under the state directory or
  `XDG_RUNTIME_DIR`), and removes it afterwards.
- When no candidate fits, the probe's failure names the path length and the limit, and the daemon's
  withheld-floor reason says so.
- The two host lib tests above pass under a TMPDIR of 100 bytes; one case asserts it.
- The probe's isolation checks are unchanged.
