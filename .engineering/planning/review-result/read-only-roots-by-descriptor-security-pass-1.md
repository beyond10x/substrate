---
format: aep.planning-md/3
id: review-result:read-only-roots-by-descriptor-security-pass-1
kind: review-result
status: active
title: Security review of read-only roots bound by descriptor
relations:
- reviews: story:read-only-roots-bound-by-descriptor
revision: 1
---
## Security review of read-only roots bound by descriptor (commit d8e09385d)

A first adversarial pass stopped before writing a case and is not counted. The security review
below replaced it.

| Invariant | Verdict |
|---|---|
| openat2 with `RESOLVE_NO_SYMLINKS`, mounted `--ro-bind-fd` | enforced |
| no root host path in the bubblewrap argv | enforced |
| a symlink at any component is `exec.read-only-root-not-a-directory` (dangling, to a file, loop, loop as parent, `/proc/<pid>/root` as parent) | enforced |
| refusal codes unchanged; EACCES is `exec.read-only-root-absent` | enforced |
| root descriptors closed in the driver after spawn | not enforced: with `wait: true` the driver held them until the exec ended. Fixed in 87aa551 (`drop(read_only_roots)` after spawn) |
| 4 roots, 16 read-only files and 8 secret slots start together | enforced |
| the mounted directory is the one admission inspected after a path swap | enforced (delegated lane) |

The four cases are in `crates/substrate-host/tests/read_only_root_bind_conformance.rs`; delegated
run after the fix: 4 passed, 0 failed.

Not covered: `read_only_files` and the workspace descriptor are held the same way for a waited exec.
That predates this wave.
