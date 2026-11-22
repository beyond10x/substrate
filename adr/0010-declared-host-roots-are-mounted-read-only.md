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

## Amendment, 2026-11-20: eight roots

A box's toolchain closure already needs `~/.cargo` and `~/.rustup`, and a third tool root left the
start one root short of `MAX_READ_ONLY_ROOTS` (four) with nothing to spare for a second language's
closure (substrate#236). The bound becomes **eight**: the `read_only_roots` start list, the applied
record and the `exec.read-only-roots` fact's `max_roots` state it from bundle `0.27.0`. The rule
above is unchanged — each root is read-only, declared, pinned without a symlink and reported — and
eight is still a list a reviewer reads. Bundles `0.23.0` to `0.26.0` keep their bytes and state
four. (Decided 2026-11-20, issue #236.)

## Amendment, 2026-11-22: a root that meets a path this host keeps private is refused

A declared root admits host state into a confined process, and the caller chooses the host path.
[ADR 0038](0038-writable-roots-are-named-host-paths-or-git-checkouts.md) decision 2 refuses a
writable root that meets a path the daemon keeps for itself. This amendment states the same kind
of rule for a read-only source: a root here, and a single file in
[ADR 0033](0033-a-single-host-file-is-admitted-read-only.md), which carries the same amendment.
Decides `story:read-only-mount-sources-refuse-the-daemons-private-paths`.

### Decision

1. **The rule.** A start is refused when a read-only root's `host_path` equals, lies under or
   contains an entry of the read set. A read-only file's `host_path` is refused when it equals or
   lies under one.
2. **The read set**, as the daemon computes it from its own configuration:
   - the state directory, which is the parent directory of `--state`: the durable ledger and what
     is keyed beside it, the aperture and host-process state and the instance lock;
   - the workspace root: every subject's workspaces, and the capsule, scratch, aperture and
     Git-baseline directories the host driver keeps under it;
   - the production TLS private key, the static-bearer file, every secret-slot source, the API
     socket and the delegated cgroup root.

   Each entry is something only the daemon, or the one subject it serves a workspace to, may
   read or reach. A workspace is given to a run by its identifier, never by host path, so the
   workspace root holds nothing a start needs to name. The API socket is in the set because a
   read-only bind does not stop a connect.
3. **Narrower than the writable set.** ADR 0038 also keeps a writable root clear of the daemon
   executable, the bubblewrap binary, the TLS certificate chain and both CA bundles. Those are
   protected against replacement, not against reading, and they are not in the read set: a
   toolchain root such as `/usr` may hold them and must still mount.
4. **The set is the daemon's, not an embedder's.** Nothing is in it for lying under an embedding
   application's directory. With `--state <E>/substrate/state.db` and
   `--workspaces <E>/workspaces`, those two directories, everything under them and every
   ancestor, `<E>` among them, are refused as a root, and `<E>/presets/<name>/<version>/tools`
   mounts. With `--state <E>/state.db` the state directory is `<E>` and all of it is refused:
   the daemon already requires that directory to be its own and owner-only. An embedder gives
   Substrate a directory of its own.
5. **Where and when.** At daemon admission, in `check_read_only_roots` and
   `check_read_only_files` (`crates/substrate-daemon/src/app/operations.rs`), for `exec.start`
   and, through its embedded exec input, `session.start`. It comes after the lexical rules and
   the capability fact and before dispatch, so the ledger records the start as refused and no
   driver sees it. The fact stays first: a host that publishes none says nothing about a path.
6. **How it is compared.** By path component, never by string prefix, against each entry as
   written (made absolute) and as resolved at that admission, because a secret-slot file may be
   replaced without a restart ([ADR 0012](0012-secret-slots-are-sealed-memfds.md)). An entry that
   cannot be resolved at that moment is compared as written only, and the start does not fail for
   that. The caller's path is compared as text. That is sound with two existing rules: the wire
   admits only an absolute canonical `host_path`, and the driver pins it with `openat2`
   `RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS` and binds that descriptor, so a path that binds
   has no symlink in any component and its text is its place in the daemon's mount namespace.
7. **Every posture, daemon admission only.** There is no exception for the local Unix socket:
   the reader is the confined program, not the caller. The set exists in the daemon
   (`PrivateHostPaths` in `crates/substrate-daemon/src/runtime.rs`), which is where it is
   enforced. The Kubernetes driver serves neither list.

### Refusal

The published code for a root this host cannot give is reused: `exec.read-only-root-absent`,
422 `refused`, at `read_only_roots`, with a message of its own:

> A declared read-only root is not admitted: its host path meets a path this host keeps private.

The wire crate already reads that code as "not present, or not reachable, on this host", and for
a caller a private path is not reachable. The message names no entry of the set, so a refusal
says nothing about where this host keeps what. No refusal code, schema or vector is added, no
released bundle byte changes, and no successor bundle is cut. Only the message tells this refusal
from a missing root. A code of its own needs a successor bundle and is left to the story named
below.

### Not decided

- **The embedded library.** A consumer that opens the host driver in process is its own caller
  and is not reached by decision 7. The driver knows its workspace root, its capsule and
  aperture roots and its secret-slot sources, and knows no ledger and no listener key.
- **A second name for a private object.** The rule compares paths. A hard link to a private
  object, or a bind mount of one, elsewhere on the host is a name the host's operator made.
  Telling it apart needs the pinned descriptor's device and inode, which is the driver's half
  and answers after dispatch.
- **Which sources may be named at all.** That is
  `story:read-only-mount-sources-are-declared-by-the-operator`.
