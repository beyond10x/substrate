---
format: aep.planning-md/3
id: review-result:deny-list-security-third-look
kind: review-result
status: active
title: 'Security review, third look: the executable deny-list'
relations:
- reviews: story:executable-deny-list
revision: 1
---
# Security review, third look: the executable deny-list (pull request 243, `36db41964`)

Verdict: no denied file runs; two mount shapes where the driver and the helper disagree.

- The four false-refusal shapes the implementor named hold on a real kernel.
- S9-1 and S9-2 are the findings; both are fixed in `3c96672e7`, and the case with 14 staged
  starts, `security6_exec_denied`, is in the suite and the delegated lane.
- The other direction does not exist: the guard reads only rows of the daemon's own mount table;
  a run-made directory name holding a line break and the text of a row is no row.
- Not run: the delegated lane, any workspace run.
- Round 4 itself was not reviewed again. The helper enforces the list; the driver decides only
  whether a start is refused before dispatch.

```findings
[
{"file":"crates/substrate-host/src/exec_gate.rs","line":1926,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"S9-1 A start is still dispatched and ended by the helper with status 125, mount-covered. The helper asks what every directory it steps reaches (landlock.rs:541), the root of a mount it enters included, and by place that holds mounts attached under another mount of the same file system; the driver asks only the listed directory. Staged: a/back a non-recursive bind of b, two tmpfs stacked on b/sub, a listed, a/tool denied: ran Some(125), 'cannot establish where /opt/k/b/sub is attached: mount-covered'. Design 35 section 6 item 3 says such a start is refused before dispatch. Nothing runs. Case: security6_exec_denied.rs, row 'second mount of a reached file system'."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":858,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"S9-2 The driver is stricter than the helper in a second shape, beside the accepted one. A mount of a type outside the list whose mount point is gone beneath a mount of a listed type laid over the directory above is counted as an entry, because reported_at answers mount-covered for a path that is gone (place.rs:993) and unlisted_entries counts every error. With the deny entry inside the covering mount the start is refused file-system-unlisted; the helper never meets the mount and runs it. Case: security6_exec_denied.rs, rows 'covered from above, mount point gone' (red) and 'mount point kept' (runs)."}
]
```
