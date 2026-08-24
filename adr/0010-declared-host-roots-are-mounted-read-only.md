---
status: accepted
date: 2026-08-24
---

# ADR 0010: declared host roots are mounted read-only

## Context

An exec runs with `/usr`, `/bin`, `/lib` and `/lib64` bound read-only, the workspace bound
read-write at `/workspace`, and no network. That is enough to run an interpreter whose whole
implementation lives under `/usr`, and it was verified from inside a confined process: `/etc` is
unreadable, the operator's home is not listable, and a credential file under it cannot be opened.
The isolation is real and this decision does not weaken it.

It is not enough to run a *toolchain*. A build tool's compilers, its package registry and its
pinned toolchain live in per-user directories under the operator's home — `~/.cargo`, `~/.rustup`,
`~/.npm`, `~/.m2` — and none of them is under `/usr`. A confined `cargo --version` therefore
starts and stops at `could not find cargo home dir`. Because the network is unshared, there is also
no route by which the process could fetch what it is missing, which is the intended behaviour and
also a dead end.

Execution capsules (ADR 0009) are the wrong instrument. A capsule carries its files **inline in the
request**, digested and bounded, and that ADR says in terms what it is for: small development
fixtures, and "not the host kernel, interpreter, libraries, or read-only base system". A toolchain
and a package registry are gigabytes. It also named the gap this decision closes — "a separately
defined complete runtime closure".

So the substrate can confine a process that *interprets* and cannot confine one that *builds*. That
is a capability boundary drawn by an accident of where a distribution puts files, rather than by
anything a caller declared.

## Decision

Add an optional, bounded list of **declared read-only roots** to exec and raw-pipe start. A root is
a host directory and the absolute path it appears at inside the sandbox. Both are the caller's, and
both are validated before dispatch.

Substrate stays generic. It knows nothing about Rust, cargo, npm or any other toolchain: a root is a
directory and a mount point, and which directories a *client* needs is the client's semantics, in
the client's repository. Nothing named after a vendor enters this contract.

Validation refuses rather than adjusts. A root is refused when the host path is not absolute,
not canonical, not an existing directory, or reaches the sandbox through a symlink; when the mount
point is not absolute, not canonical, or collides with a mount substrate owns — `/usr`, `/bin`,
`/lib`, `/lib64`, `/proc`, `/dev`, `/tmp`, `/workspace`, and the execution capsule's `/runtime`;
when two roots name the same mount point; or when the list is longer than the bound the capability
snapshot publishes. A refused root refuses the dispatch. Nothing is silently dropped, re-pointed or
made writable.

The mount is `--ro-bind`. A declared root is readable and never writable, so a process cannot alter
the toolchain it was given any more than it can alter `/usr`, and the workspace remains the only
writable path. The network stays unshared; a declared root is a way to bring a closure *in*, not a
way to reach out.

The applied confinement observation lists every root that was mounted, with its host path and its
mount point, so a reader of a finished run can see exactly what was admitted rather than inferring
it from the fact that a build succeeded. The capability snapshot publishes the served bound.

**This admits host state into a confined process, and that is the cost.** A declared root is
trusted by the caller and unverified by substrate: unlike a capsule there is no manifest and no
digest, because hashing a package registry on every exec is not a thing anybody would run twice.
What substrate guarantees is narrower and is stated rather than implied — the root is mounted
read-only, at the point declared, and reported in the observation. Whether the directory's contents
are trustworthy is the caller's claim, and a governed profile that cannot accept that claim should
refuse to declare any root rather than declare one and hope.

## Consequences

- A confined process can run a toolchain whose closure lives outside `/usr`, which is what the
  evaluation programme's own corpus needs: its verification step is `cargo test`.
- The isolation properties verified from inside a sandbox are unchanged for a run that declares no
  root, which is every existing consumer.
- Substrate gains no vendor semantics. The mapping from "a Rust build" to two directories lives in
  the client that wants it.
- A declared root is the first input substrate mounts without verifying its bytes. That asymmetry
  with ADR 0009 is deliberate and is the reason the observation reports what was mounted: what
  cannot be verified must at least be visible.
- The wire, the capability snapshot, the contract bundle and the runtime vectors all grow a bounded
  field, and the refusals above need vectors for path escape, mount collision, a non-directory, and
  the bound.
