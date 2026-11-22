---
format: aep.planning-md/3
id: review-result:private-read-only-sources-adversary
kind: review-result
status: active
title: 'Adversary pass: read-only sources and the daemon''s private paths'
relations:
- reviews: story:read-only-mount-sources-refuse-the-daemons-private-paths
revision: 1
---
# Adversary pass: read-only sources and the daemon's private paths (pull request 256, `f04cb3041`)

Verdict: no blocker; NEEDS-CHANGE for one finding. Seven cases added to the unit module, two red.

- Could not break: `/` as a root, a state directory of `/`, linked ancestors, string-prefix
  siblings, trailing and doubled slashes, `.`, non-UTF-8 entries, 64 slot sources.
- Each admission refusal case asserts 422, class, code, message, address, zero driver starts and
  the recorded state; the control reaches a fixture driver that binds nothing.
- Not checked: the delegated lane, the integration binary, any mutant, hard links and bind
  mounts.
- All findings were answered in round 2 (`4f4260c0c`), which was not reviewed again.

```findings
[
{"file":"crates/substrate-daemon/src/app/operations.rs","line":903,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Admission resolves every private entry on the async worker outside bounded_blocking, so an entry on a mount that stops answering pins a worker per start naming a read-only source."},
{"file":"crates/substrate-daemon/src/runtime.rs","line":751,"category":"mutant","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"No case fails when serve stops installing the read set: every admission case injects it and no lane drives the shipped binary to this refusal."},
{"file":"crates/substrate-daemon/src/runtime.rs","line":1421,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"An entry whose last component links to a file not written yet resolves to itself, so the place the link names is not met; no operator shown to rotate that way."},
{"file":"crates/substrate-daemon/src/runtime.rs","line":1405,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"The written form keeps `..`, so a directory an entry only passes through is refused as containing it; no configuration shown to write `..`."},
{"file":"crates/substrate-daemon/src/runtime.rs","line":3240,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"An existing case pins a slot source's previous target as admitted after re-pointing, which the amendment does not require."}
]
```
