# Repository status

**Observed:** 2026-08-13

## Current state

The phase-2 **minimum host slice** is complete. The canonical bundle is implemented by a standalone
Rust daemon with exactly twelve routes, kernel-derived Unix peer identity, a durable subject-scoped
operation/resource store, guarded descriptor-relative workspace I/O, and a probed bubblewrap plus
cgroup-v2 exec driver. See [Plan 02](docs/plan/02-minimum-host-slice.md).

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | enforce ADRs 0001–0006 in dependency and conformance tests |
| Wire contract | `substrate-wire` 0.1.0 has 12 closed operations, exact hash/state fixtures, and executable route/threat coverage | keep generated types subordinate to the bundle and prove clean-room producer conformance |
| Drivers | Linux host driver implemented; absent delegation keeps exec facts absent; real delegated lane passes | retain the delegated lane and add no optimistic facts |
| Security | `openat2` beneath/no-link/no-mount I/O, atomic replacement, cleared/shaped environment, namespace no-egress, pids/memory+swap/CPU cgroup bounds, output draining, timeout, and whole-tree kill are enforced | expand adversarial coverage without weakening admission |
| Stack integration | trust, session, event, federation, and contract-release seams accepted in umbrella ADRs 0015–0019 | keep later features behind their named phases |
| Implementation | phase 2 complete: wire/store/host/daemon crates, portable black-box lane, and delegated confinement lane are green | begin phase 3 event journal and deeper recovery design/implementation |

## Repository facts

- The Rust workspace has four crates: `substrate-wire`, `substrate-store`, `substrate-host`, and
  `substrate-daemon`; the contract bundle remains authoritative.
- No Flux package, type, protocol, or checkout is required.
- The portable clean-room lane passes 14 HTTP/startup cases without confinement and therefore proves
  typed `unserved`; the delegated Linux lane passes 21 cases including actual bounded exec.
- Phase-2 tests also prove canonical hash fixtures, restart-to-unknown recovery, subject isolation,
  replay/conflict, symlink escape refusal, strict request/limit handling, and all twelve routes.
- Events, leases, Git, sessions, workloads, images, volumes, endpoints, Docker, and Kubernetes are
  still absent rather than stubbed.

## External dependencies

Substrate has no source dependency on another Daemonloom repository. Cross-repository compatibility
will use stable wire contracts and conformance fixtures, never sibling path dependencies.
