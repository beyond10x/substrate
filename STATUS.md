# Repository status

**Observed:** 2026-08-13

## Current state

The active repository goal is **phase-2 contract execution readiness**. Runtime scaffolding begins
only after the operation registry, closed route schemas, exact hashing fixtures, complete threat
inventory, and machine-executable fixture format pass the bundle gate in a clean checkout. The
ordered checklist is [Plan 02](docs/plan/02-minimum-host-slice.md#active-goal-contract-execution-readiness).

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | `substrate-wire` 0.1.0 draft common schemas and first non-executable vector cases are hash-covered | add closed route schemas, machine-checkable fixtures, and the remaining threat inventory before generating code |
| Drivers | exactly one active driver per v1 daemon; first driver is Linux host | implement the closed capability document and host conformance port |
| Security | Linux enforcement floor, local/hosted trust, token lifecycle, Git refusal rules, secret handoff, and subject isolation accepted | complete the executable Design 04 threat fixtures; the present cases are not proof |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | phase 2 contract work in progress; no runtime code yet | finish the closed route schemas and executable vectors, then scaffold the minimum vertical slice |

## Repository facts

- No `Cargo.toml`, source tree, implementation crate, generated API, or container artifact exists
  yet; the development contract bundle is the first implementation artifact.
- No Flux package, type, protocol, or checkout is required.
- The existing contract is preserved and curated rather than replaced.
- The first planned implementation is a minimum host slice; Docker and Kubernetes do not block it.

## External dependencies

Substrate has no source dependency on another Daemonloom repository. Cross-repository compatibility
will use stable wire contracts and conformance fixtures, never sibling path dependencies.
