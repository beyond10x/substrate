---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

The newest release is 0.25.0, published on 23 November 2026. The
[status page](./status.md#newest-release) summarizes it.

A release that moves the contract bundle needs its SDK clients upgraded with the daemon. See the
[deployment rules](./guides/deployment.md#rules-that-do-not-change-by-posture).

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.25.0 (2026-11-23)

### Added in 0.25.0

- The Rust SDK's command and session builders take `.workdir(..)`, the directory the program
  starts in (design 26). The SDK applies the wire's lexical rule before any request and answers
  `SdkError::Protocol`. An exec start on a host without the `exec.workdir` fact is refused
  `exec.workdir-unserved`. A session start on a host without confinement is refused
  `session.confinement-unavailable` first, before its working directory is read.

### Changed in 0.25.0

- In the Rust SDK an identifier is one path segment. Every call that names a workspace, an exec,
  a session, a host process or a reconciliation snapshot by id refuses an id that is empty, `.`
  or `..`, or holds `/`, `?`, `#`, `%` or a control character, with `SdkError::Protocol` naming
  the kind of id, before any request is sent; a handle's own id is held to the same rule.
  `Client::operation` refuses an operation id outside its published form the same way. A call
  therefore reaches the route its method names and no route beneath it. The CLI passes its id
  arguments through these calls.

- The ESS pin is 0.97.4, from 0.86.0 (the six `ess-inputs.yaml` under `spec/`, and `ESS_VERSION`
  and `ESS_SHA256` in `gate.yml`). The host-process suite is regenerated: its 92 scenarios keep
  their ids and twelve change. Eleven create their second instance with the inputs' declared
  examples, `RenewLease/not-live` also sends an ended adopted record, and synthesis records 26
  refusals where it recorded 27. The other suites and the CLI contract keep their bytes, and the
  generated types change only in their header. Measured on a workstation under ESS 0.97.4:
  synthesis of the confinement suite took 28 s (0.86.0: 92 s) and `cargo xtask check-ess` as a
  whole 54 s.

- A Git create whose root name already has a commit-id file or a store is refused
  `workspace.git-target-exists` before any request to the source. A create whose store would
  be larger than the create's byte limit (the requested storage limit, else 2 GiB), or whose
  baseline holds an object whose header states more, is refused `workspace.git-storage-limit`,
  and the workspace is proved absent.
- A Git create whose baseline nests directories deeper than 1,024 is refused
  `workspace.git-path-unrepresentable` before its store is written, and the workspace is
  proved absent.
- A Git create whose baseline names more paths than its inode limit (the requested storage
  limit, else 200,000) is refused `workspace.git-storage-limit` before anything is checked
  out, and the workspace is proved absent. Every entry of every tree counts once for each
  place the baseline names it, a directory that holds nothing included.
- A Git create whose baseline holds a name longer than 255 bytes or a path longer than 4,096
  bytes is refused `workspace.git-path-unrepresentable`. One whose paths together are longer
  than 256 bytes for each path its inode limit admits, or which holds a tree whose header
  states more than 283 bytes for each such path, is refused `workspace.git-storage-limit`.
  Each is refused before anything is checked out, and the workspace is proved absent.
- A Git create whose baseline holds a name with a `/` in it is refused
  `workspace.git-path-unrepresentable`. One that holds a tree which states more than 283
  bytes for each entry it holds, or whose files and links together state more bytes than the
  create's byte limit, each counted once for each place the baseline names it, is refused
  `workspace.git-storage-limit`. Each is refused before anything is checked out, and the
  workspace is proved absent.
- A Git workspace's baseline objects are on disk a second time, compressed, outside its
  reported usage: a Git workspace under a project quota holds at most twice its byte limit.
- The changes observation answers whatever the workspace's `.git/objects` holds. Where the
  reader of 0.24.0 refused `workspace.git-repository-failed` for an object store with a link,
  an entry that is not a regular file, an `info/alternates` file, more than eight nested
  directories or more than 262,144 entries, the observation now answers from the store.
- A Git workspace created by a release before this one has no baseline store. After the
  upgrade both Git observations of it (changes and baseline file) are refused
  `workspace.git-baseline-missing` until the workspace is created again; its files, sessions
  and execs are unaffected.

### Fixed in 0.25.0

- `b10x-substrate workspace destroy` run again with the same `--operation-id` exits 0 and prints
  the stored result. It read the workspace before the delete and answered `resource.not-found`.
  The SDK gains `Client::destroy_workspace`, which destroys by id without that read. Only
  `workspace destroy` changes: `workspace renew`, `exec run` and `session signal` still read
  their resource before the mutation, and a retry of one of them after that resource is gone
  answers the read's `resource.not-found`.
- The Rust SDK recovers a Git workspace create whose reply is lost from the operation ledger, as
  it recovers every other mutation. It returned the transport error, without the operation id.
- The Rust SDK's workspace builder takes the source selected last: `git(..).empty()` creates an
  empty workspace and sends no source authority. `empty()` left an earlier Git selection in place.

### Security in 0.25.0

Four fixes for defects of earlier releases, each found by a review. Each entry states the rule
that holds from this release on, the releases it concerns and what it leaves open.

- **Both Git observations read the baseline from a store the driver keeps, and open nothing
  beneath the workspace's `.git/objects`** (design 21, § *The baseline store*;
  `story:daemon-holds-the-baseline-objects-of-a-git-workspace`). What that directory holds, and
  what a program running in the workspace does to it during an observation, changes neither
  the answer of the change-set observation and of the baseline-file observation nor whether
  there is one. No route, schema or refusal code changes.
  - **Affected:** every earlier release that serves the two Git observation routes, `0.24.0`
    included.
  - The store is `.substrate-git-baselines/.objects/<root name>/` in the directory that holds
    all workspaces, beside the commit-id file `.substrate-git-baselines/<root name>`. It is
    one pack and its index and holds the baseline commit, every tree it reaches and every
    file and link object those trees name: no history, and no commit of another repository a
    tree records. Directories are mode 0700 and files 0400. It lies in no workspace, is not
    part of a workspace's reported usage and is under no workspace's project quota.
  - A create writes the store after the checkout is verified, then the commit-id file, then
    installs the workspace. Each object is read by its id and checked against it. The store
    is made in a staging directory inside `.substrate-git-baselines` and renamed into place.
  - A Git workspace with a commit-id file and no store has no Git observation: both are
    refused `workspace.git-baseline-missing`. The same refusal answers a store that does not
    hold the commit the commit-id file names, and a store whose pack or index the Git library
    cannot read as one. A store that cannot be opened for any other reason is refused
    `workspace.git-baseline-read-failed`. No observation writes, removes or replaces a store,
    and the driver never reads a file a workspace wrote as a Git object, a pack or the index
    of a pack.
  - A Git create whose fetched pack stores an object behind more than 4,095 entries is refused
    `workspace.git-storage-limit` before the header of any object is read. One whose baseline
    holds objects stored behind more than 64 entries together for each path its inode limit
    admits is refused the same way before anything is checked out, once the headers within
    that bound are read. One whose fetched pack holds an object twice is refused
    `workspace.git-checkout-failed`.
  - The fetch of a Git create allocates for one object no more than the create's byte limit:
    an object that states more ends the fetch `workspace.git-storage-limit`, and an allocation
    within the limit that the host refuses carries the same name.
  - The driver refuses a root name outside the root-name set, `workspace.path-escape`, before
    it writes or removes a commit-id file or a store; a Git create with such a name is refused
    before dispatch.
  - Wherever the driver removes a workspace's commit-id file, it removes the store with it, in
    a build without the `git` feature too. A failed create leaves neither entry, also when its
    staging tree cannot be removed. At start, a driver with a Git source configured removes
    every entry of `.substrate-git-baselines` that is neither the commit-id file nor the store
    of a workspace directory that exists; a staging directory an interrupted write left is
    such an entry. In a build without the `git` feature that start rule is new.
  - Design 23 holds unchanged: a driver with no Git source configured neither reads nor
    removes anything in `.substrate-git-baselines`, at start or at destroy. A Git workspace
    destroyed by such a driver leaves its commit-id file and its store until a start with a
    Git source configured removes them.
  - **Not covered:** how much work and memory a Git create spends on a source before one of
    its limits refuses it (`story:git-create-work-and-memory-follow-its-limits`, a draft).

- **A stored operation outcome is returned to a mutation request only after the recorded grant
  is compared, in every transaction in which such a request can find the row**
  ([ADR 0017](https://github.com/beyond10x/substrate/blob/main/adr/0017-delegated-context-is-verified-before-replay.md)). The comparison
  holds for requests that arrive at the same time: a request that verified another grant than the
  row records is answered `delegated-context.grant-conflict` (409) whichever of them wrote the
  row, and a refusal decided from a request's own envelope or delegated context, such as a
  missing or invalid context, is never replaced by another request's stored outcome. Reads of an
  operation (`GET /v1/ops/{op}`, the event stream) are as before and need no context. The wire
  and the bundle are unchanged.
  - **Affected:** every earlier release that verifies a delegated context, `0.24.0` included,
    in a hosted deployment.

- **A v2 file replacement, edit or patch is written only when its result fits what the
  operation ledger reserved for it.** The daemon decides that before it dispatches the change to
  any driver, and holds the driver's answer to the reserved bytes of JSON, escaping counted, and
  to the `maxLength` the result schema publishes, by cutting the diff and marking it
  `truncated`.
  - **Affected:** every earlier release that serves the v2 file replacement, edit and patch
    routes, `0.24.0` included.
  - A replacement creates the parent directories it asks for directly before the write. A
    replacement refused for its text bounds, its digest, its content, its size or its
    precondition creates no directory.
  - The server itself refuses an `old_text`, a `new_text` or a patch `text` over its published
    `maxLength`, and a replacement `content.data` outside the two lengths its schema publishes,
    as `request.schema-invalid` (422) before dispatch. No wire schema, route or refusal code
    changes, and the request the daemon sends an executor is unchanged.
  - **Not covered:** a replacement whose write fails after its parent directories were created
    (a file name the file system refuses, a quota, a failed rename) leaves those directories
    (`story:failed-file-replacement-removes-the-directories-it-created`, a draft). The success
    results of an exec start, an exec signal and a host-process start are not yet decided
    against the reserve before their side effect
    (`story:every-success-result-fits-the-ledger-before-its-side-effect`, a draft).

- **An exec on the Kubernetes namespace driver is held to what it was admitted for until the
  request that creates its Pod is sent** (design 34 § 6.3). After every wait the start can sit
  in, the driver computes its capability snapshot again and creates the Pod only when that
  snapshot is a valid proof of every fact the start requires, on the node the workspace's keeper
  runs on, and under the binding, the policy set and the Lease of the admission. A proof the
  driver renewed while the start waited serves it when it proves the same facts. Otherwise the
  start is `exec.capability-stale` (422) before dispatch, recorded as refused, and no Pod is
  created. Daemon admission of an exec start takes the facts and the digest from one capability
  snapshot and still refuses a start that names any snapshot but the current one.
  - **Affected:** every earlier release whose namespace driver starts an exec, `0.24.0`
    included.
  - A proof of the namespace driver is valid for a span measured on the driver's monotonic
    clock from the probe that made it, and the next probe is due on that clock. The wall clock
    states `probed_at` and `valid_until` in the snapshot and can only end a proof early, when
    it passes `valid_until`: it never keeps a proof valid. No wire schema, capability fact or
    refusal code changes.
