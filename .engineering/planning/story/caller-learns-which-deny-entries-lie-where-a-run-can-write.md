---
format: aep.planning-md/3
id: story:caller-learns-which-deny-entries-lie-where-a-run-can-write
kind: story
status: draft
title: A caller can ask which deny entries of a profile lie where a run can write
summary: The stakeholder's request with the stated deny-list limit; needs an adapter method that pins the view without a program
tags:
- sdc
revision: 1
---
# Story: a caller can ask which deny entries of a profile lie where a run can write

## Outcome

Before a start, an embedder can ask the `linux` adapter, and `systemd` around it, which entries
of a profile's executable deny-list lie in a location the run can write, and refuse such a
configuration itself. Today only the documentation says that a deny entry there holds for the
files that exist at the start and no more.

## Where the requirement comes from

Session `runbook`, the stakeholder of `story:executable-deny-list`, on 2026-11-22, when it
decided that the limit is stated and no mechanism changes: "the helper's result names each deny
entry that lies in a location the run can write, so a consumer can refuse such a configuration
itself; the wording alone must not be the only place a caller learns it." It asked for it in the
deny-list's round only if cheap, otherwise as a story. The implementor judged it not cheap.

## Why it is not a field

- Only a start knows the exact answer. The driver's plan classes each deny entry against the
  mounts of the view (`beneath` means a location a run can write), and the view is pinned at
  admission of a start.
- Asking without a start needs a new adapter method that pins the view without a program, on
  `linux` and `systemd`, with lane cases on real mounts. The implementor's estimate: 150 to 250
  lines plus tests. Not measured.
- A lexical answer from the profile alone is small but wrong in one shape: a read-only root
  whose host directory a run can write through another mount. `macos` resolves entries
  differently and was not run.
- `render` shows the class in the helper's options today (`--deny beneath …`), but refuses a
  profile with writable roots.

## What changes

1. Design first (invariant 8): a section of design 35 that names the method, what it pins, what
   it returns for an entry that does not exist, and what `macos`, `local`, `oci` and `pod`
   answer.
2. A failing test first: a profile with one deny entry in the workspace and one in a read-only
   root; the method names the first and not the second; the same through a read-only root whose
   host directory lies in a writable location.
3. `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.
4. No new fact, no wire change, no new refusal. A consumer that wants a refusal makes it.

## Not decided

Whether the answer also rides on the start's own result, so a consumer that did not ask learns
it after the fact.

## Acceptance

The method names every deny entry of a profile that lies where the run can write, on `linux` and
on `systemd`, in the three shapes above, and a start of the same profile behaves as before.
