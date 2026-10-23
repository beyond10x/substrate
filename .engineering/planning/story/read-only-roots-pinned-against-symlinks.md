---
format: aep.planning-md/3
id: story:read-only-roots-pinned-against-symlinks
kind: story
status: draft
title: Read-only roots are pinned against symlinked components
relations:
- decomposes: epic:host-run-gates
scope:
- confidence: inferred
  path: adr/0010-declared-host-roots-are-mounted-read-only.md
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/src/read_only_roots.rs
- confidence: cited
  path: crates/substrate-host/tests/read_only_root_bind.rs
- confidence: cited
  path: crates/substrate-host/tests/read_only_root_bind_conformance.rs
- confidence: cited
  path: docs/design/26-working-directory-and-several-apertures.md
revision: 4
---
# Story: read-only roots are pinned against symlinked components

**Weakens confinement.** Rank this ahead of contract-only work.

## Problem

Declared read-only roots (ADR 0010) are checked by path: the host confirms the path names an
existing directory, then bubblewrap binds it by the same path. No check refuses a symlink in a
parent component, and nothing pins the directory between admission and bind, so a host path
changed in that window, or reached through a symlinked parent, can bind a directory other than the
one admitted. `read_only_files` (ADR 0033) already does this right: it opens each host path with
`openat2(O_PATH|O_NOFOLLOW, RESOLVE_NO_SYMLINKS|RESOLVE_NO_MAGICLINKS)`, checks the type with
`fstat`, and binds `/proc/self/fd/<n>` (`crates/substrate-host/src/read_only_files.rs`).

## Contract

Roots get the same treatment: resolved without symlinks in any component, pinned by descriptor at
admission, bound through the descriptor, with a named refusal for a symlinked component. Refusing
a path that is admitted today is a behaviour change; it needs a note in ADR 0010 or a successor ADR
and, if the refusal code is new, a successor bundle.

## Acceptance

- A root whose host path has a symlink in any component is refused before dispatch with a named
  code.
- Replacing the root's directory between admission and bind does not change what the exec sees: a
  delegated test swaps it and observes the admitted directory, or a refusal.
- Existing root tests and the delegated lane stay green.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Finding:** the acceptance is already met on `main` by `d8e09385` (`story:read-only-roots-bound-by-descriptor`, implemented): `openat2` with `RESOLVE_NO_SYMLINKS` refuses a symlinked component as `exec.read-only-root-not-a-directory` (`crates/substrate-host/src/read_only_roots.rs:75-172`), the bind is `--ro-bind-fd` (`:52`), and design 26 § 3 records the behaviour change — cited
- **Files:** `crates/substrate-host/src/read_only_roots.rs`, `crates/substrate-host/src/process.rs:1703`, `:2349` — cited
- **Tests:** `crates/substrate-host/tests/read_only_root_bind.rs:119`, `:224` (delegated swap case), `read_only_root_bind_conformance.rs:160` — cited
- **Documents:** `docs/design/26-working-directory-and-several-apertures.md` § 3 — cited; `adr/0010-declared-host-roots-are-mounted-read-only.md` — inferred, only if the note must also be in the ADR
- **Confidence:** high
- **Not established:** the tests were not run by the scoper; a separate refusal code for a symlinked component would need a successor bundle and a decision.
