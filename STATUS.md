# Repository status

**Observed:** 2026-08-13

## Current state

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | minimum endpoint/envelope, bundle layout, compatibility, and connectors projection mapping accepted | implement the `0.1.0` development bundle before generating server/client code |
| Drivers | exactly one active driver per v1 daemon; first driver is Linux host | implement the closed capability document and host conformance port |
| Security | Linux enforcement floor, local/hosted trust, token lifecycle, Git refusal rules, secret handoff, and subject isolation accepted | turn Design 04 threat rows into negative fixtures |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | absent but unblocked | begin phase 2 with bundle/vectors, then the minimum vertical slice |

## Repository facts

- No `Cargo.toml`, source tree, implementation crate, generated API, or container artifact exists
  yet; this is observed delivery state, not a remaining design blocker.
- No Flux package, type, protocol, or checkout is required.
- The existing contract is preserved and curated rather than replaced.
- The first planned implementation is a minimum host slice; Docker and Kubernetes do not block it.

## External dependencies

Substrate has no source dependency on another Daemonloom repository. Cross-repository compatibility
will use stable wire contracts and conformance fixtures, never sibling path dependencies.
