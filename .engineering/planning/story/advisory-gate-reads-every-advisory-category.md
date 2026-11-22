---
format: aep.planning-md/3
id: story:advisory-gate-reads-every-advisory-category
kind: story
status: draft
title: The advisory gate reads every RustSec category and records each exception with its reason
summary: Unsound and unmaintained advisories fail the gate unless listed with a reason; three lockfile bumps clear three of today's
tags:
- security
revision: 1
---
# Story: the advisory gate reads every RustSec category and records each exception with its reason

## Outcome

`cargo xtask check-advisories` fails on an advisory of any category, vulnerability, unsound,
unmaintained or notice, unless the repository lists its id with a reason. Today it asks for the
vulnerability category only (`xtask/src/advisories.rs`) and has no list.

## What was found

At RustSec revision `55e1c3dfbd834237a05bbbf7b50ba3b5be27b662`, `Cargo.lock` of `1af046902`:
0 vulnerabilities, 14 unsound advisories in five packages (faster-hex, git2, heapless, rustix,
tokio), 2 unmaintained (chrono, rustls-pemfile). Read from source, nothing built: 13 of the 14
flagged items are not called. One is: `AsyncFd::new` (RUSTSEC-2026-0386) in
`crates/substrate-host/src/pty.rs` and `crates/substrate-mcp/src/protocol.rs`; both sites own
their descriptor, so the advisory's condition is not met.

## What changes

1. The verb reads every category and an exception list in the repository; an entry without a
   reason, or one whose advisory no longer matches the lockfile, fails.
2. Lockfile bumps: faster-hex to 0.10.1 (RUSTSEC-2026-0306), rustix to 1.1.6
   (RUSTSEC-2026-0486), tokio to a patched release (RUSTSEC-2026-0386). Patched tokio deprecates
   `AsyncFd::new`; both call sites change, and `substrate-mcp` forbids unsafe code, which the
   change has to respect.
3. git2 0.20 to 0.21 (RUSTSEC-2026-0183 and -0184): tried; when its API change is larger than
   this story, the two ids are listed with the reason and the bump gets its own story.
4. Listed with a reason: the nine heapless advisories (RUSTSEC-2026-0574 to -0582; no release
   of gix-hash admits a patched faster-hex chain), chrono (RUSTSEC-2026-0543) and rustls-pemfile
   (RUSTSEC-2025-0134), each with what would remove it.
5. The verb prints the database revision it read.

## Design

No contract changes. AGENTS.md's gate table gets the verb's new sentence.

## Acceptance

The verb's cases: an unsound advisory without an entry fails; an entry without a reason fails; a
stale entry fails. The gate is green at the story's head with the list as committed.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22:
the dependency triage. The operator ordered the review handled on 2026-11-22. Checked by reading;
nothing was built. Stakeholder: the operator.
