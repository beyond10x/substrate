# Repository status

**Observed:** 2026-08-13

## Current state

The phase-2 **contract execution-readiness** goal is complete. The exact operation registry, closed
route schemas, canonical hashing fixtures, threat inventory, executable fixture format, and
clean-room runner contract pass the dependency-free bundle gate. Runtime scaffolding is now open;
the bundle does not claim host-driver conformance until a producer executes the vectors. See
[Plan 02](docs/plan/02-minimum-host-slice.md#contract-execution-readiness-gate).

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | `substrate-wire` 0.1.0 has 12 closed operations, exact hash/state fixtures, and executable route/threat coverage | keep generated types subordinate to the bundle and prove clean-room producer conformance |
| Drivers | exactly one active driver per v1 daemon; first driver is Linux host | implement the closed capability document and host conformance port |
| Security | Linux enforcement floor plus complete executable Design 04 threat inventory is accepted | implement the host enforcement and make every vector pass without weakening a request |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | phase 2 contract gate passed; no runtime code yet | scaffold the Rust workspace and implement the minimum vertical slice against the bundle |

## Repository facts

- No `Cargo.toml`, source tree, implementation crate, generated API, or container artifact exists
  yet; the development contract bundle is the first implementation artifact.
- No Flux package, type, protocol, or checkout is required.
- The existing contract is preserved and curated rather than replaced.
- The first planned implementation is a minimum host slice; Docker and Kubernetes do not block it.

## External dependencies

Substrate has no source dependency on another Daemonloom repository. Cross-repository compatibility
will use stable wire contracts and conformance fixtures, never sibling path dependencies.
