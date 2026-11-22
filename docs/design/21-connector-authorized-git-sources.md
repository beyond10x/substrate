# 21. Connector-authorized Git workspace sources

Status: accepted for implementation on 2026-09-04.

## Context

The wire contract already permits `WorkspaceSource::Git`, but the daemon and host driver reject
every non-empty source. Products therefore duplicate a repository into two uploaded file trees and
cannot provide a real `.git` worktree to terminals or editors. The advertised schema and the
served capability must become one truth.

## Decision

Substrate materializes a Git source through one deployment-configured HTTPS source named by the
request. The durable request records a non-secret locator, provider branch, exact commit, and depth;
the transient control request carries a source authority in the
`X-B10X-Workspace-Source-Authorization` header. The daemon passes that value directly to the host
driver and excludes it from JSON, state, hashes, logs, process arguments, and environment.

The host driver clones into a private temporary directory with a driver-owned Git implementation,
checks out the exact requested commit, verifies `HEAD`, fsyncs the materialization and its parent,
records the commit in owner-private host metadata, and atomically renames the materialization into
the workspace root. A normal `.git` directory remains available to the workspace
terminal, while file and tree APIs continue to hide `.git`. The provider branch is metadata; it is
never replaced by a hard-coded branch name.

The initial implementation supports depth 1 through 50, a two-GiB workspace limit, and 200,000
inodes. HTTPS is mandatory, redirects are refused, the configured source selects the only admitted
origin, and the transient authority may be attached only to that exact origin. Submodules and Git
LFS are not followed automatically.

## Lifecycle and recovery

The create operation is durable before driver dispatch and does not report `ready` until commit
verification, fsync and atomic installation all succeed. The host stages under a private temporary
directory. A failure before rename proves the workspace absent and removes that staging tree; a
failure after rename is outcome-unknown and normal reconciliation observes the installed root.
Substrate never replays a source fetch from its ledger because the authority is deliberately not
durable: a caller retries a proved-absent attempt under a new operation id and fresh authority.

`workspace.git` is advertised only when at least one segment-bounded HTTPS Git source is configured
and a startup probe proves the local Git repository/configuration mechanism. It deliberately does
not dial the configured endpoint: source readiness depends on per-request authority and must not
make daemon readiness depend on a remote service. An unconfigured deployment rejects Git sources
before allocation. Empty-source workspace behavior is unchanged.

## The baseline store

Amendment of 2026-11-22 (`story:daemon-holds-the-baseline-objects-of-a-git-workspace`). It changes
no route, no schema and no refusal code. The store is state the driver keeps for itself: a route
published as a read stays one for its caller.

The driver keeps, for each Git workspace, the objects of its baseline commit in a store of its
own: the commit, every tree it reaches and every file and link object those trees name. No
history, and no object of another repository's commit a tree records. Both observations read the
baseline from this store and open nothing beneath the workspace's `.git/objects`.

- **Where.** `.substrate-git-baselines/.objects/<root name>/`, in the directory that holds all
  workspaces, beside the files `.substrate-git-baselines/<root name>` that hold the commit ids. A
  root name has no `.`, so `.objects` is no workspace's commit-id file. The driver refuses a root
  name outside the root-name set before it writes or removes either entry. The directories are
  mode 0700 and the files 0400, owned by the daemon's user. The store lies in no workspace, so no
  view binds it, and the directory of all workspaces is one of the daemon's private paths: no
  read-only root, read-only file or writable root can name it.
- **Form.** One pack and its index, written by the driver. Each object is read by its id from the
  staging repository, checked against that id and then written. The store is written into a
  staging directory inside `.substrate-git-baselines`, of a name no root name can have, synced,
  and renamed into place.
- **When.** After the checkout is verified against the admitted commit and before the commit-id
  file is written and the workspace is installed. A workspace is `ready` only with its store. A
  create whose root name already has a commit-id file or a store is refused
  `workspace.git-target-exists` before either is written. A failure writes nothing: the
  workspace is proved absent, as for every failure before the rename, and the refusal is
  `workspace.git-baseline-write-failed`.
- **What it costs.** The compressed size of one commit's files. Measured on 2026-11-22 with
  `git pack-objects` on one commit of three repositories: 7.1 MB for 73.2 MB of files (15,691
  files), 28.9 MB for 113.5 MB (10,316), 6.4 MB for 21.7 MB (2,890).
- **Its bound.** A store larger than the byte limit the create applies (the requested storage
  limit, else 2 GiB) is not written and the create is refused `workspace.git-storage-limit`. The
  store is not part of the workspace's reported usage and is not under its project quota: it is
  written once and never grows. A Git workspace under a project quota therefore holds at most
  twice its byte limit on disk; in the three measurements above the store adds 10 % to 30 % to
  the files.
- **Removal.** Wherever the driver removes a workspace's commit-id file, it removes the store
  with it, in the build without the `git` feature too; a failed create removes both even when
  its staging directory could not be removed. At start the driver removes every entry of
  `.substrate-git-baselines` that is neither the commit-id file nor the store of a workspace
  directory that exists; a staging directory left by an interrupted write is such an entry.
  Design 23 holds unchanged: a driver with no Git source configured neither reads nor removes
  anything in `.substrate-git-baselines`, at start or at destroy. A Git workspace destroyed by
  such a driver therefore leaves its commit-id file and its store, up to the byte bound above,
  until a start with a Git source configured removes them.

**A workspace materialized before the store existed** has a commit-id file and no store. The first
observation that finds no store builds it. One build runs at a time in a daemon; an observation
that needs one waits for its turn, and while it waits it holds none of the places the driver
keeps for file operations. The turn ends when the store is in place or the build is refused,
before the observation compares anything. An observation that has waited 10 s for its turn is
refused `workspace.git-baseline-missing`; a later one tries again.

1. It copies the workspace's `.git/objects` into a staging directory inside
   `.substrate-git-baselines`. The copy is made beneath the admitted workspace's own directory,
   pinned as the reader pins it, with plain reads that do not block. An entry that is neither a
   directory nor a regular file, a link among them, an `info/alternates` file, directories nested
   deeper than eight, or more than 262,144 entries ends the attempt
   `workspace.git-repository-failed`. More bytes than the copy's bound end it
   `workspace.git-storage-limit`; the bound is the limit of the workspace's project quota where
   it has one, else 2 GiB.
2. It opens the copy, not the workspace, and writes the store from it as a materialization does:
   each object by its id, checked. An object of the baseline whose header states a size over the
   copy's bound is not read and ends the attempt `workspace.git-storage-limit`; a create checks
   the same against its byte limit.
3. It removes the copy, whatever the outcome.

When an object is absent or fails its check, no new store is kept, a store that was there
stays as it was, and the observation is refused
`workspace.git-baseline-missing`. A later observation tries again. A program that runs in the
workspace during the copy can make an attempt fail. It cannot make the store hold an object that
is not the baseline's, because every object is bound to the daemon-held commit id by its hash. A
store that does not hold the commit the commit-id file names is treated as absent and replaced
the same way; a store whose pack or index the Git library cannot read as one counts as not
holding the commit. A store that cannot be opened for any other reason is not removed: the
observation is refused `workspace.git-baseline-read-failed`.

**The shape of a baseline is checked before anything walks it.** The check reads each tree of
the baseline once, however often the baseline names it, and holds two bounds:

- **Nesting.** A baseline whose directories nest deeper than 1,024 is refused
  `workspace.git-path-unrepresentable`.
- **Paths.** A baseline that names more paths than the inode limit is refused
  `workspace.git-storage-limit`. Every entry of every tree counts once for each place the
  baseline names it, a directory that holds nothing included. The inode limit is the one the
  create applies (the requested storage limit, else 200,000); for a build it is the limit of
  the workspace's project quota where it has one, else 200,000.

A create runs the check on the fetched commit before it checks anything out, and a build runs it
on the copy before it writes the store. Every store therefore holds a baseline within both
bounds, and what a create, a build or an observation does with a baseline is bounded by them,
not by the number of places a tree is named in.

What a build costs and does not bound:

- While it runs, the workspace, the copy and the store exist together; the copy and the store
  are outside every quota. One build at a time holds that to one copy per daemon.
- The Git library reads each object of the copy to the size its header states, and an object
  the baseline's objects are stored against is read without the check of step 2. The reader this
  section replaces read the workspace's object store the same way in every observation; a build
  does it until one attempt succeeds (`story:git-read-limits-bound-the-work`).

Decided: comparing the worktree by kept stat facts instead of hashing each tracked file (ADR
0051's manifest) is not part of this step. The observation hashes as before.

## Observations

Substrate publishes a bounded baseline-file read and a path-sorted bounded Git change-set
observation computed against the exact materialized commit kept in host-private metadata. Tree and
current-file reads remain lazy and paged. The observation
does not disclose `.git`, Connector authority, remote credentials, or remote URL userinfo.
Every byte the change-set observation returns comes from a file beneath the admitted workspace's
root or from the baseline the daemon holds: the workspace's own repository metadata decides
neither which directory is read nor how an entry is read. The baseline-file read returns an
object of the baseline store and, once the workspace's store exists (§ *The baseline store*),
opens nothing in the workspace.

### What the change-set observation reads

The answer compares the worktree beneath the admitted workspace's root with the tree of the
baseline commit the daemon holds.

- **Baseline.** The tracked set and every expected object id come from the tree of the baseline
  commit, read from the baseline store. Each object is read by its id and checked against that id. A size in the answer is the
  size of an object that was read whole and checked, never a size an unchecked header states.
- **Changed or not.** A tracked file or link is unchanged only when the hash of its content, read
  beneath the root, equals the tree's object id, and its kind and executable bit agree. No
  modification time, no size and nothing the workspace's repository records decides it. A file
  over the per-file bound is still hashed; only its text is withheld. What could not be compared
  is reported as changed, never as unchanged, and the answer is marked.
- **The index labels a path and decides nothing else.** It decides neither whether a path
  changed nor whether a path is listed: a path absent from the baseline is `added` when the
  index records it and `untracked` when it does not, and a path the index records in conflict
  stages is `conflicted`. A path the tree of the baseline commit records as another
  repository's commit is neither compared nor entered. An index entry of that kind is honoured
  only at such a path; at any other path it labels like any other entry, and the path and
  everything beneath it are compared and listed.
- **Worktree.** Every entry is opened beneath the root without following a link. A link is
  reported as its own target text. The workspace's repository configuration is not read: its
  attributes, filters and `core.fileMode` do not apply.
- **Ignore rules** come from regular files beneath the root, the `.gitignore` files and
  `.git/info/exclude`. A leading byte-order mark is skipped.
- **Object store.** The workspace's `.git/objects` is not opened. What it holds, and what a
  program running in the workspace does to it during an observation, changes neither the answer
  nor whether there is one. The one exception is the build of a store for a workspace that
  predates it (§ *The baseline store*).

What bounds the reads of one observation:

- `max_file_bytes` bounds the text of one worktree file or link, and of one baseline object, that
  a patch is rendered from. A side over it has no text and `binary` set.
- Rules beyond the first 1 MiB of an ignore file do not apply.
- One observation hashes at most 1 GiB to compare: the tracked files it compares and the
  baseline objects it reads for their sizes, taken in the byte order of their paths.
  - A tracked file larger than what is left of it is not hashed. It is reported `modified` with
    `binary` set, no text and no `current.oid`, the answer is marked `truncated`, nothing is
    taken from the 1 GiB for it, and the comparison goes on with the next path.
  - A baseline object whose size is over what is left ends the answer there, marked
    `truncated`. The size is the baseline store's, which no writer of the workspace can change.
- Beside that 1 GiB, each listed item reads and hashes its worktree side when that is within
  `max_file_bytes`: at most `max_files` × `max_file_bytes` more, 1,000 MiB at the largest
  bounds a query may name.
- Directories nested deeper than 100 are not entered and the answer is marked `truncated`. A
  changed or untracked path of more than 64 components refuses the observation
  `workspace.path-escape`, as before: the answer cannot name it.
- Not bounded by the observation: the Git library reads the workspace's index whole, to its
  apparent size. A storage limit that counts blocks does not bound an apparent size
  (`story:git-read-limits-bound-the-work`). The baseline's objects come from the baseline store,
  whose files the driver wrote within the workspace's byte limit.

Where the answer differs from the reader that asked the Git library to compare the worktree:

- An index entry flagged assume-unchanged or skip-worktree does not hide a changed file.
- A change that is staged while the worktree holds the baseline's bytes is not listed; a path
  the index no longer records is compared like any other tracked path.
- The workspace's attributes and filters do not apply: a path marked `-diff` gets a textual
  patch, and line-ending conversion is not applied before the comparison.
- `core.fileMode` does not apply: a changed executable bit is always reported.
- The daemon user's own ignore file and a `core.excludesFile` do not apply.
- An ignore file over 1 MiB applies only the rules in its first 1 MiB.
- An untracked directory that holds a `.git` entry of its own is not entered and not listed;
  the earlier reader failed the observation.
- A side over `max_file_bytes` has `binary` set and no text; `binary` is set for binary content.
- `current.oid` is present whenever the worktree side was hashed.
- A tracked path that is not UTF-8 fails the observation whether or not it changed.
- Every tracked file is hashed in each observation, within the 1 GiB above; a tracked file
  larger than what is left of it is reported `modified` without being read.
- An index entry that records another repository's commit, at a path where the baseline
  records none, hides neither that path nor what lies beneath it.
- Once the workspace's store exists, the answer does not depend on what the workspace's
  `.git/objects` holds.

## Refusals

The served seam is closed and names the failed guarantee:

| Condition | Refusal |
|---|---|
| deployment has no matching configured source | `workspace.git-source-unserved` (`unserved`) |
| locator is outside the exact scheme/host/port/path-segment aperture, carries userinfo, or needs encoded-path normalization | `workspace.git-locator-refused` (`refused`) |
| source authority is absent or malformed | `workspace.git-authority-absent` / `workspace.git-authority-invalid` (`refused`) |
| branch no longer resolves to the admitted commit | `workspace.git-commit-moved` (`refused`) |
| transfer, installed bytes or installed inode count crosses its admitted ceiling; the baseline store at create, or the copy a store is built from, crosses its byte bound | `workspace.git-transfer-limit` / `workspace.git-storage-limit` (`exhausted`) |
| a Git-only observation is requested from a non-Git workspace | `workspace.git-workspace-required` (`refused`) |
| baseline file or aggregate change query exceeds its declared bound | `workspace.git-baseline-limit` (`exhausted`) or `request.schema-invalid` (`refused`) |
| local Git, TLS trust, checkout, fsync, installation, private baseline or reconciliation machinery fails | a stage-specific `workspace.git-*-failed` (`failed`), with no library text or authority bytes |

## Consequences

Workspace can bind one canonical materialization instead of maintaining base and working copies.
The filesystem and Git index become the source of editor, terminal, tree, and diff truth. The Git
source schema is no longer aspirational: unsupported deployments omit the capability and configured
deployments serve it end to end.
