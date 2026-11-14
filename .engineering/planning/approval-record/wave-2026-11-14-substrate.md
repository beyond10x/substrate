---
format: aep.planning-md/3
id: approval-record:wave-2026-11-14-substrate
kind: approval-record
status: approved
title: 'Substrate wave 2026-11-14: RustSec advisories, in-process test FIFOs, SDK aperture list'
relations:
- decides: story:deps-advisories-rmcp-libsqlite3-sys
- decides: story:test-fifos-made-in-process
- decides: story:sdk-aperture-list
revision: 3
transitions:
- {from: "draft", to: "approved", at: "2026-11-14T20:13:49Z", actor: "human:agent", revision: 3, executor: "human:timo"}
---
# Substrate wave 2026-11-14: RustSec advisories, in-process test FIFOs, SDK aperture list

## Approval

Timo approved this wave on 2026-11-14 in the agent-box session, choosing it without a release
("B": the wave without the 0.21.0 release).

## Units

| unit | artifact | note |
|---|---|---|
| U1 | `story:deps-advisories-rmcp-libsqlite3-sys` | `rmcp` =3.2.0 to =3.9.0, `rusqlite` 0.40 to 0.41 (`libsqlite3-sys` 0.38.2 to 0.39.1) for RUSTSEC-2026-0457, -0459, -0461, -0462; `check-advisories` failed every pull request without it |
| U2 | `story:test-fifos-made-in-process` | test helpers call `nix::unistd::mkfifo`; the Ubuntu 26 `ubuntu-latest` runner's `mkfifo` refuses non-UTF-8 arguments (runs 43614631820, 43615670301) |
| U3 | `story:sdk-aperture-list` | the SDK's exec, `pipe_session` and `pty_session` builders send a list of egress apertures, which the daemon already accepts; conductor's box runtime needs the model aperture and a dial-in aperture to conductord because `socket(AF_UNIX)` is refused (ADR 0039) |

## Terms

- **Order:** U1 and U2 land together, because each alone fails the other's gate step; U3 follows.
- **Gate:** the full gate on the wave's pull request; nothing merges red and no check is overridden.
- **Release:** none. Consumers pin the merge commit on `main` by exact revision.
- **Superseded pull requests:** #219 (U3), #220 (U1 and U2) and #221 (U2) close in favour of the
  wave's pull request.
