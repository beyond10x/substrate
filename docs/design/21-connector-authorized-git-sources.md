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

**A workspace materialized before the store existed** has a commit-id file and no store. The
driver builds none for it. Both Git observations of such a workspace are refused
`workspace.git-baseline-missing` until the workspace is created again; its files, its terminal
and every other route are unchanged. The driver never reads a file a workspace wrote as a Git
object, a pack or the index of a pack; the workspace's Git index is still read, for labels only.
Decided by the operator on 2026-11-22, in place of a one-time build of a store from the
workspace's own object files, which four reviews did not find bounded.

The same refusal answers a store that does not hold the commit the commit-id file names, and a
store whose pack or index the Git library cannot read as one. A store that cannot be opened for
any other reason is refused `workspace.git-baseline-read-failed`. No observation removes or
replaces a store.

**The shape of a baseline is checked before anything walks it.** The check reads each tree of
the baseline once, however often the baseline names it, and holds these bounds:

- **Stored chains.** A pack may store an object as its difference from another entry, and that
  entry as its difference from a third. The Git library walks such a chain each time it reads
  the object's header. Before it reads any header, the create therefore reads how many entries
  each entry of the fetched pack is stored behind: one step for each entry, from the entry's
  own header and the pack's index, with ten bytes kept for each. A pack with an entry stored
  behind more than 4,095 entries, the deepest chain `git pack-objects --depth` writes, is
  refused `workspace.git-storage-limit` before the header of any object is read. A baseline
  whose commit, trees and file objects are stored behind more than 64 entries together for
  each path the inode limit admits is refused the same way before anything is checked out:
  the check ends before it reads the header that would take it over that bound. A pack of
  Git's default depth, 50, is within the bound with one object for each such path. A pack
  with an entry stored behind what is no entry of it, or behind itself, is refused
  `workspace.git-checkout-failed`. So is a pack whose index holds an id twice or out of
  ascending order, or whose fan-out table is not the count of its ids' first bytes: with ids
  that ascend, each once, and that table, the Git library and the reading of the chains find
  the same entry by an id.
- **Nesting.** A baseline whose directories nest deeper than 1,024 is refused
  `workspace.git-path-unrepresentable`.
- **Names and path lengths.** A name longer than 255 bytes, a name that holds a `/`, or a path
  longer than 4,096 bytes is refused `workspace.git-path-unrepresentable`: no checkout can make
  it as one entry. A baseline whose paths together are longer than 256 bytes for each path the
  inode limit admits is refused `workspace.git-storage-limit`.
- **Object sizes.** While the fetch indexes the pack it received, it allocates for one object
  no more than the byte limit: an object that states more, whether the pack stores it as it is
  or behind another entry, ends the fetch `workspace.git-storage-limit` before those bytes are
  allocated, and an allocation within the limit that the host refuses carries the same name.
  The fetch inflates each entry as it arrives and again when it computes the ids; the limit
  bounds what it allocates for one object. The check then reads the header of the commit, of
  each tree and of each file object before the object: one that states more than the byte
  limit is not read and is refused `workspace.git-storage-limit`. A tree is also not read when
  its header states more than 283 bytes for each path the inode limit admits, and is refused
  when it states more than 283 bytes for each entry it holds, the most one entry with a
  255-byte name takes.
- **Paths.** A baseline that names more paths than the inode limit is refused
  `workspace.git-storage-limit`. Every entry of every tree counts once for each place the
  baseline names it, a directory that holds nothing included. The check also refuses as soon as
  the distinct trees it has read hold more entries than the inode limit, so it reads and keeps
  no more of a baseline than that. The inode limit is the one the create applies: the
  requested storage limit, else 200,000.
- **Bytes.** A baseline whose files together state more bytes than the byte limit is refused
  `workspace.git-storage-limit`. A file counts once for each place the baseline names it.

A create runs the check on the fetched commit before it checks anything out, and again before
it writes the store. Every store therefore holds a baseline within these bounds. A create asks
for the header of each object of the baseline three times: in each check, and when it writes
the store. What the Git library walks for those headers is bounded by the inode limit, and the
reading of the chains by the entries of the fetched pack: neither by the number of places a
tree is named in, nor by how deep the pack stores an object. Reading an object whole applies
each entry it is stored behind: at most 4,095 of them, none of more bytes than the byte limit,
and for the whole baseline no more bytes than the next rule admits.

**The work and the memory of a create follow its byte limit.** Amendment of 2026-11-24
(`story:git-create-work-and-memory-follow-its-limits`). It changes no route, no schema and no
refusal code. Each bound below is a multiple of the byte limit the create applies, the requested
storage limit, else 2 GiB. A create over one of them is refused `workspace.git-storage-limit`,
and the workspace is proved absent, as for every failure before the rename.

- **Whole reads.** Reading an object whole inflates every entry the object is stored behind and
  makes the object of each, one after the other, and keeps none of them for the next read. The
  reading of the chains therefore also reads two numbers of each entry: the bytes the entry
  inflates to, from its header, and, for an entry stored behind another, the bytes of the
  object it makes, from the first bytes of the entry. It inflates no more of an entry than
  those. It keeps eight more bytes for each entry, eighteen in all: the bytes a whole read of
  the entry inflates and makes, its own and those of every entry it is stored behind. The
  check adds that number for the commit, for each tree and for each file object, once for each
  place the baseline names it. A baseline for which the sum is more than 64 times the byte
  limit is refused. The check adds a tree's number before it reads the tree, so it ends before
  the read that would take it over the bound, and before any file object is read. After that
  check, how often a create reads an object whole follows the places the baseline names it in
  and nothing the pack states: a file object is read once when it is checked out and once when
  the store is written.
- **The table of entries.** While a pack arrives the fetch keeps one item of 64 bytes for each
  entry. The table grows with the entries that arrive: before the first entry it is reserved
  for no more than 8,192 items, whatever number the pack states. Its items take no more bytes
  than the byte limit. A pack that states more entries than that is refused before its first
  entry is read, and a pack that brings more is refused at the entry that would be over. At
  2 GiB that is 33,554,432 entries.
- **Objects that wait.** When the fetch computes the ids, an object that other entries are
  stored behind is held until each of them has been made, so what is held at once follows how
  the entries of the pack branch. The fetch counts the bytes reserved for each object it holds
  this way and holds no more than the byte limit: the object that would take it over
  ends the fetch. Beside them each of its two threads holds what the rule of object sizes
  admits for the one entry it works on.
- **What the fetch makes.** When it computes the ids the fetch inflates each entry once and
  makes the object of each entry stored behind another once. It counts those bytes before it
  allocates for them and ends before they come to more than 64 times the byte limit.

The first bound is held where the chains are read and the baseline is checked
(`crates/substrate-host/src/git/store`). The other three are held in `gix-pack`, the crate in
which the fetch indexes a pack. It takes the byte limit as the most it allocates for one object
(`gitoxide.objects.allocLimit`) and refuses each of the three with the error it has for an
allocation over that limit, which the create already names `workspace.git-storage-limit`.

**What the bounds refuse of an ordinary source.** A byte limit can be no smaller
than the files of the baseline and the fetched pack together; the figures here are multiples of
that sum. Measured on 2026-11-24 on a fetch of depth 50 of five repositories and of one
generated history of 1,000 files and 50 commits (1,022 to 16,703 places, 2.2 MB to 114 MB of
baseline objects, 2,096 to 20,431 entries): whole reads 0.8 to 14.4 against the bound 64, the
table 0.003 to 0.07 against 1, the objects that wait at most 0.11 against 1, and what the
fetch makes 1.3 to 3.1 against 64. With a byte limit as small as that sum, the bounds refuse:

- a baseline of which a whole read of each object inflates and makes, on average by bytes,
  more than 64 times the object: one stored behind 63 entries that each make an object of its
  size;
- a fetch with fewer than 64 bytes of byte limit for each entry of its pack: many files of a
  few bytes with a long history;
- a pack in which more than the byte limit waits at once. Versions of a file stored
  one behind the other in a line hold two of them at once. Stored as a tree, with more than
  one entry behind each level, they hold one for each level: for a file of a tenth of the byte
  limit that is a tree more than 10 levels deep;
- a fetched history whose entries inflate to and make more than 64 times the byte limit: about
  sixty versions of every object of the baseline, each stored behind another.

A larger requested storage limit admits each of them.

**The copy of `gix-pack`.** The repository carries the crate as `vendor/gix-pack-0.74.2`: the
published source of the version `gix` 0.87.1 resolves, with the three bounds, selected by
`[patch.crates-io]` as `vendor/axum-0.8.9` is. The change is about 100 added lines in three
files (`src/cache/delta/tree.rs`, `src/index/write/mod.rs` and
`src/cache/delta/traverse/resolve.rs`). It adds two counters the threads of the fetch share,
no thread and no lock, and changes no public item of the crate. It is kept this way:

- `vendor/gix-pack-0.74.2/README.substrate.md` states the change file by file, the three
  multiples, and the checksum of the published crate the copy was made from. The crate's
  licence files stay in the copy.
- The workspace names `gix` by its exact version. A `[patch.crates-io]` entry replaces only the
  version it has, and cargo builds the published crate with no more than a warning when the two
  differ. `cargo xtask check-packages` therefore refuses a `[patch.crates-io]` entry that
  `Cargo.lock` does not resolve to.
- The cases below run the fetch of a create, so they fail with a `gix-pack` that does not hold
  the three bounds, whichever version it is.
- An update of `gix` takes the published source of the `gix-pack` it resolves as
  `vendor/gix-pack-<version>`, makes the change in it again and runs the cases. The copy is
  removed with the update whose `gix-pack` passes them unchanged.

**What an ordinary create pays.** The reading of the chains keeps eighteen bytes for each
entry of the fetched pack, eight of them for this bound, and inflates the first bytes of each
entry stored behind another: for the six fetched packs above, no more than 0.02 s of processor
time. The fetch adds to two counters for each entry, and its table grows with the entries.
Indexing those six packs with the three bounds took 0.05 s to 2.6 s of processor time and
4 MB to 48 MB of peak resident memory.

**How it is proved.** One case for each of the four bounds, against a source served on
loopback: a pack within every other bound and over that one. The create is refused
`workspace.git-storage-limit`, the workspace is proved absent, and the memory of the test
process stays under a bound the case states: its peak resident memory, and for the table of
entries its peak address space, because a reservation that is never written is no resident
memory. Each case runs under 5 s. The cases of a create that the host crate already has run
against the copy unchanged.

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
object of the baseline store and opens nothing in the workspace. A workspace without a store has
no Git observation (§ *The baseline store*).

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
  nor whether there is one.

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
- The answer does not depend on what the workspace's `.git/objects` holds.
- A Git workspace materialized before the baseline store existed has no Git observation: both
  are refused `workspace.git-baseline-missing` until it is created again.

## Refusals

The served seam is closed and names the failed guarantee:

| Condition | Refusal |
|---|---|
| deployment has no matching configured source | `workspace.git-source-unserved` (`unserved`) |
| locator is outside the exact scheme/host/port/path-segment aperture, carries userinfo, or needs encoded-path normalization | `workspace.git-locator-refused` (`refused`) |
| source authority is absent or malformed | `workspace.git-authority-absent` / `workspace.git-authority-invalid` (`refused`) |
| branch no longer resolves to the admitted commit | `workspace.git-commit-moved` (`refused`) |
| transfer, installed bytes or installed inode count crosses its admitted ceiling; the baseline store at create crosses its byte bound, or the baseline is outside the shape bounds of § *The baseline store* | `workspace.git-transfer-limit` / `workspace.git-storage-limit` (`exhausted`) |
| a Git-only observation is requested from a non-Git workspace | `workspace.git-workspace-required` (`refused`) |
| baseline file or aggregate change query exceeds its declared bound | `workspace.git-baseline-limit` (`exhausted`) or `request.schema-invalid` (`refused`) |
| local Git, TLS trust, checkout, fsync, installation, private baseline or reconciliation machinery fails | a stage-specific `workspace.git-*-failed` (`failed`), with no library text or authority bytes |

## Consequences

Workspace can bind one canonical materialization instead of maintaining base and working copies.
The filesystem and Git index become the source of editor, terminal, tree, and diff truth. The Git
source schema is no longer aspirational: unsupported deployments omit the capability and configured
deployments serve it end to end.
