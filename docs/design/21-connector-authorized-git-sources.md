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

## Observations

Substrate publishes a bounded baseline-file read and a path-sorted bounded Git change-set
observation computed against the exact materialized commit kept in host-private metadata. Tree and
current-file reads remain lazy and paged. The observation
does not disclose `.git`, Connector authority, remote credentials, or remote URL userinfo.
Every byte the change-set observation returns comes from a file beneath the admitted workspace's
root or from the baseline the daemon holds: the workspace's own repository metadata decides
neither which directory is read nor how an entry is read.

### What the change-set observation reads

The answer compares the worktree beneath the admitted workspace's root with the tree of the
baseline commit the daemon holds.

- **Baseline.** The tracked set and every expected object id come from the tree of the baseline
  commit. Each object is read by its id and checked against that id. A size in the answer is the
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
- **Object store.** Before the Git library opens `.git/objects`, the reader walks it beneath the
  root and refuses the observation with `workspace.git-repository-failed` when it holds a link,
  an entry that is neither a directory nor a regular file, an `info/alternates` file,
  directories nested deeper than eight, or more than 262,144 entries. The check holds while no
  program runs in the workspace. While one runs, the observation gives no guarantee of an
  answer, and it still returns no byte and no size that was not checked against the daemon-held
  commit.

What bounds the reads of one observation:

- `max_file_bytes` bounds the text of one worktree file or link, and of one baseline object, that
  a patch is rendered from. A side over it has no text and `binary` set.
- Rules beyond the first 1 MiB of an ignore file do not apply.
- One observation hashes at most 1 GiB to compare: the tracked files it compares and the
  baseline objects it reads for their sizes, taken in the byte order of their paths.
  - A tracked file larger than what is left of it is not hashed. It is reported `modified` with
    `binary` set, no text and no `current.oid`, the answer is marked `truncated`, nothing is
    taken from the 1 GiB for it, and the comparison goes on with the next path.
  - A baseline object whose stated size is over what is left ends the answer there, marked
    `truncated`. That is no more than the writer of a workspace can already do to its own
    object store.
- Beside that 1 GiB, each listed item reads and hashes its worktree side when that is within
  `max_file_bytes`: at most `max_files` × `max_file_bytes` more, 1,000 MiB at the largest
  bounds a query may name.
- Directories nested deeper than 100 are not entered and the answer is marked `truncated`. A
  changed or untracked path of more than 64 components refuses the observation
  `workspace.path-escape`, as before: the answer cannot name it.
- Not bounded by the observation: the Git library reads the index, the commit and tree objects
  of the baseline, and the stored file of each object it reads, whole, each to its apparent
  size. A storage limit that counts blocks does not bound an apparent size
  (`story:git-read-limits-bound-the-work`).

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
- An object store with `info/alternates`, a link, an entry that is not a regular file,
  directories nested deeper than eight or more than 262,144 entries is refused.

## Refusals

The served seam is closed and names the failed guarantee:

| Condition | Refusal |
|---|---|
| deployment has no matching configured source | `workspace.git-source-unserved` (`unserved`) |
| locator is outside the exact scheme/host/port/path-segment aperture, carries userinfo, or needs encoded-path normalization | `workspace.git-locator-refused` (`refused`) |
| source authority is absent or malformed | `workspace.git-authority-absent` / `workspace.git-authority-invalid` (`refused`) |
| branch no longer resolves to the admitted commit | `workspace.git-commit-moved` (`refused`) |
| transfer, installed bytes or installed inode count crosses its admitted ceiling | `workspace.git-transfer-limit` / `workspace.git-storage-limit` (`exhausted`) |
| a Git-only observation is requested from a non-Git workspace | `workspace.git-workspace-required` (`refused`) |
| baseline file or aggregate change query exceeds its declared bound | `workspace.git-baseline-limit` (`exhausted`) or `request.schema-invalid` (`refused`) |
| local Git, TLS trust, checkout, fsync, installation, private baseline or reconciliation machinery fails | a stage-specific `workspace.git-*-failed` (`failed`), with no library text or authority bytes |

## Consequences

Workspace can bind one canonical materialization instead of maintaining base and working copies.
The filesystem and Git index become the source of editor, terminal, tree, and diff truth. The Git
source schema is no longer aspirational: unsupported deployments omit the capability and configured
deployments serve it end to end.
