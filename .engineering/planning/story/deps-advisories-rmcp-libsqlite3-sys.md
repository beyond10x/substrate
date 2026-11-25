---
format: aep.planning-md/3
id: story:deps-advisories-rmcp-libsqlite3-sys
kind: story
status: implemented
title: 'check-advisories passes again: rmcp 3.9.0 and rusqlite 0.41 for four November RustSec advisories'
summary: rmcp moves from 3.2.0 to 3.9.0 and rusqlite from 0.40 to 0.41 (libsqlite3-sys 0.38.2 to 0.39.1), the smallest versions that clear RUSTSEC-2026-0457, -0459, -0461 and -0462.
owner: substrate
tags:
- dependencies
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-14T20:14:06Z", actor: "human:agent", revision: 3, executor: "human:timo"}
- {from: "proposed", to: "active", at: "2026-11-14T20:14:06Z", actor: "human:agent", revision: 4, executor: "human:timo"}
- {from: "active", to: "implemented", at: "2026-11-25T19:09:42Z", actor: "agent:substrate", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# Story: check-advisories passes again: rmcp 3.9.0 and rusqlite 0.41

## Outcome

A pull request against `main` passes `cargo xtask check-advisories` in *Gate: static checks*
again: the workspace no longer locks a crate version that a RustSec advisory published after the
last green gate on `main` (8b194eaf, 2026-11-08) names.

## Context

Pull request #219 failed *Gate: static checks* only at `cargo xtask check-advisories`, on four
advisories against versions `main` also locks:

| Advisory | Crate, locked | Patched | Moves to |
|---|---|---|---|
| [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html) (SQLite WAL uninitialised memory, CVE-2026-119375) | `libsqlite3-sys` 0.38.2 via `rusqlite` 0.40.2 | `>= 0.39.0` | `libsqlite3-sys` 0.39.1, `rusqlite` 0.41.1 |
| [RUSTSEC-2026-0461](https://rustsec.org/advisories/RUSTSEC-2026-0461.html) (Streamable HTTP server tool schema cache) | `rmcp` 3.2.0 | `>= 3.6.0` | `rmcp` 3.9.0 |
| [RUSTSEC-2026-0457](https://rustsec.org/advisories/RUSTSEC-2026-0457.html) (OAuth client SSRF filter) | `rmcp` 3.2.0 | `>= 3.7.0` | `rmcp` 3.9.0 |
| [RUSTSEC-2026-0462](https://rustsec.org/advisories/RUSTSEC-2026-0462.html) (Streamable HTTP client body buffering) | `rmcp` 3.2.0 | `>= 3.9.0` | `rmcp` 3.9.0 |

Each move is the smallest that clears its advisories: `rusqlite` 0.41 is the first line on
`libsqlite3-sys` `^0.39`, and `rmcp` 3.9.0 is the highest lower bound of the three. `rmcp` stays
exact-pinned with no default features, as `cargo xtask check-mcp-boundary` requires; substrate-mcp
uses only its model types and enables none of the affected transports or the OAuth client, so the
three `rmcp` advisories are reachable by no shipped binary. `rusqlite` is bundled, so the SQLite
in the daemon's ledger and the SDK's store is the one `libsqlite3-sys` 0.39.1 carries.

## Acceptance

- `cargo xtask check-advisories` passes on the branch.
- `Cargo.lock` moves only `rmcp`, `rusqlite` and `libsqlite3-sys`; `THIRD_PARTY_LICENSES.html` is
  the cargo-about 0.9.1 fixed point of it (`cargo xtask check-licenses`).
- `cargo clippy --all-targets -- -D warnings` and the tests of `substrate-mcp`, `substrate-store`,
  `substrate-daemon` and `b10x-substrate-sdk` pass.
- `CHANGELOG.md` records the move under *Unreleased*, *Security*.

## Out of Scope

- The latest releases (`rmcp` 3.10.0, `rusqlite` 0.42.0): no advisory asks for them.
- Releasing: the move ships with the next release.

## Open Questions

None.
