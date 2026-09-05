# Design 22: bounded Git protocol v2 materialization

**Status:** accepted · **Date:** 2026-09-05

This extends [Design 21](21-connector-authorized-git-sources.md) without changing the
source payload, durable baseline, capability identifier, or released wire bundle. Atlas ADR 0034
records the coordinated Connectors-first rollout. Connectors must serve protocol v2 before
consumers select this implementation.

## Fetch boundary

The host uses exactly pinned `gix` 0.87.1 for an initial shallow fetch over blocking HTTPS.
Its transport requires an observed protocol v2 handshake; a legacy response is a named
`workspace.git-protocol-refused` refusal before reference enumeration or pack transfer. Git v2
separates capability discovery, filtered `ls-refs`, and `fetch`, allowing Connectors to constrain
reference enumeration to the admitted branch and `HEAD`. Reference prefixes are an optimization;
the host checks the exact branch and expected commit before requesting objects and again before
checkout. Depth remains the caller's admitted 1 through 50 commits. Tags, partial-clone filters,
additional packfile URLs, submodules, and LFS are not fetched.

The host creates the local repository without templates and opens `gix` with isolated configuration.
System, user, environment, and included Git configuration must not influence the fetch. No helper,
prompt, hook, URL rewrite, ambient proxy, or redirect is admitted. The only HTTPS endpoint is the
already-validated configured source aperture. TLS verification remains enabled and uses the
source's configured CA bundle when present. Transient source authorization is injected by the
HTTP adapter on requests to that exact endpoint and is never saved in Git configuration, logs,
baseline metadata, or the operation ledger.

The pinned curl backend is statically included, so runtime images need no system libcurl or its
additional transitive libraries. The backend reuses its connection across discovery and fetch. HTTP/1.1 remains the
transport envelope; Git protocol v2 does not require HTTP/2. The adapter bounds received bytes
across every HTTP response, including advertisements, before exposing bytes to Git parsers.
It retains streaming backpressure and does not buffer a whole pack. Connections have a ten-second
connection timeout, a thirty-second low-throughput timeout, and a five-minute overall fetch
deadline. Dropping the asynchronous operation signals cancellation to the blocking fetch and
prevents continuing later materialization stages. A blocked network read is additionally bounded
by the transport timeout. Transfer exhaustion, cancellation, and deadline failures are reported
without library error text or authority bytes.

## Installation and observations

Network work stays inside the existing sixteen-permit blocking executor. `git2` retains the
existing detached exact-commit checkout with filters disabled, baseline reads, and diff behavior.
After checkout, one traversal both measures installed bytes/inodes and synchronizes regular files;
it never follows symlinks. Every entry, including `.git`, contributes to the same existing accounting
semantics. Crossing either ceiling refuses installation. Directories synchronize child-before-parent,
followed by private baseline publication, atomic no-replace rename, and parent-directory fsync.
The pre/post-rename recovery boundary from Design 21 is unchanged.

Stage timings cover discovery/reference mapping, pack receipt, checkout, and synchronization with
accounting. They contain durations and byte counts only. They do not expose locator paths,
credentials, branch names, or tenant/actor identifiers.

## Verification

A local TLS Git HTTP fixture runs the actual Git upload-pack implementation. Tests observe v2 on
discovery and command requests, targeted branch selection, depth 50, absence of tags, exact HEAD,
and usable shallow history. The suite also exercises legacy negotiation refusal, moved commits,
byte exhaustion, failed trust, redirects, interruption, and redacted errors. Accounting cases cover
bytes, inodes, nested directories, `.git`, and symlinks without following their targets. Existing
baseline, diff, crash-recovery, and full repository gates continue to apply.

The authoritative transport semantics are the [Git protocol v2 specification](https://git-scm.com/docs/protocol-v2)
and the [pinned gix transport API](https://docs.rs/gix-transport/0.59.2/gix_transport/).
