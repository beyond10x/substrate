---
format: aep.planning-md/3
id: story:repository-gate-hardening
kind: story
status: implemented
title: The repository gate proves history, dependencies, links and publication fences
summary: Make every previously false-green repository check fail closed.
relations:
- decomposes: epic:release-hardening
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T08:37:44Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T08:37:44Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:49:59Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Story: Repository gate hardening

## Outcome

The full gate fails closed on corrupt history, secrets, advisories, publishable crates, incomplete Markdown links and non-Rust bot checks.

## Acceptance

- Secret scanning proves a non-empty, fsck-clean full history and exact commit count.
- RustSec vulnerabilities fail the gate and h2 is absent.
- All crates set publish=false.
- CommonMark link forms are checked.
- The bot-file checker is a cargo xtask verb.
