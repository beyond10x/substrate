---
format: aep.planning-md/3
id: review-result:adr-0109-u10-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: sandbox repository parity'
relations:
- reviews: story:sandbox-repository-parity
revision: 1
---
# Security review, pass 1: sandbox repository parity

Reviews `story:sandbox-repository-parity` at `f6f125b8` (merged as `e2e38489`). Conformance cases:
`crates/b10x-substrate-cli/tests/confine_security_review.rs`. Executed 51 before, 64 after; 7 red.

## Findings

| # | file:line | severity | finding |
|---|---|---|---|
| F1 | `crates/b10x-substrate-cli/src/confine.rs:254` | blocker | The open-time `reconcile_orphans` on the shared `SUBSTRATE_CGROUP_ROOT` kills every `substrate-ex_*` group. A second run SIGKILLed the first (137) and failed `exec.cgroup-not-empty`. A run sharing the daemon's value would kill the daemon's runs (inferred from the same code). |
| F2 | `confine.rs:156` | warning | The digest check applies only to `--backend oci`. On linux, a tagged or pinned `--image` is accepted and dropped. |
| F3 | `confine.rs:155` | warning | `--local` skips the digest check, so a tagged `--image` runs. |
| F4 | `confine.rs:165` | warning | The state directory is created 755. |
| F5 | `confine.rs:168` | note | A SIGTERM-ended run leaves the state directory. |
| F6 | `crates/substrate-host/src/probe.rs:259` | warning | Probe stderr breaks the one-envelope JSON stderr on a delegated host. |
| F7 | `confine.rs:324` | note | The child's exit code passes through unchanged. |

Verified:
- Named refusals hold for `--net` and `--ro`.
- `--local` is the only path to `local`, and needs a visible reason.
- `SUBSTRATE_CONFINEMENT_LOCAL` refuses.
- No CLI variable reaches the child.
- `--ro` is bound read-only by descriptor; siblings are invisible.
- An argv cannot become a bwrap option.
- oci input is checked.
- A hostile cgroup root widens nothing.

## Decisions taken by the coordinator

- F1: as adversary finding 1 in `review-result:adr-0109-u10-adversary-pass-1`.
- F2, F3: `--image` only with `--backend oci`. Otherwise exit 2 with `confinement.fact-unserved: image_digest_pinned`. A tag is checked before the adapter is chosen.
- F4: 0700.
- F5: remove the state directory on SIGINT and SIGTERM.
- F6: probe stdio to null.
- F7: no change; named in `--help`.
