---
format: aep.planning-md/3
id: review-result:private-read-only-sources-security
kind: review-result
status: active
title: 'Security review: read-only sources and the daemon''s private paths'
relations:
- reviews: story:read-only-mount-sources-refuse-the-daemons-private-paths
revision: 1
---
# Security review: read-only sources and the daemon's private paths (pull request 256, `f04cb3041`)

Verdict: no blocker; one warning, three notes. Read, not run; the one allowed build ended at its
bound before a case ran.

- The set is complete for everything a start can reach; both start routes pass the checks; the
  comparison is by component, both directions for a root; refused before dispatch, fact first.
- A second name without host privilege was tried through the process file system in the
  driver's view and was denied.
- The warning, that the set could be left out silently, is closed by construction in round 2
  (`4f4260c0c`); the notes are answered there too. Round 2 was not reviewed again.
- Not verified: a refusal case seen red; the delegated lane; embedders outside the collection.

```findings
[
{"file":"crates/substrate-daemon/src/app/service.rs","line":228,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"private_host_paths is a public field with an empty default that only serve fills, and no case goes through serve, so an App built any other way, or serve without its assignment at runtime.rs:751, admits every private source silently."},
{"file":"adr/0010-declared-host-roots-are-mounted-read-only.md","line":156,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The amendment says a refusal tells nothing about where the host keeps what, but the refusal answers for any caller-chosen path in both directions and the amendments do not state that this probing is accepted."},
{"file":"adr/0010-declared-host-roots-are-mounted-read-only.md","line":110,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Decision 2 lists the aperture directory under the workspace root, while the daemon keeps it beside the state file (runtime.rs:1000)."},
{"file":"crates/substrate-daemon/src/runtime.rs","line":1391,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"The namespace driver's kubeconfig path is a credential file outside the read set; no start reaches it because that driver publishes neither fact."}
]
```
