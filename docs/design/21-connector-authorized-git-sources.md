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
  entry and, beside the items, a list for each object that entries are stored behind. The
  table grows with the entries that arrive: before the first entry it is reserved for no more
  than 8,192 items, whatever number the pack states. The fetch counts each entry as two items,
  128 bytes: the one the table keeps, and one for what the indexing keeps for the entry
  beside the table, which is, one after the other, its place in the queue of the id pass and
  its place in the list the items are merged into, each no more than one item for an entry.
  The entries are sorted by id where they lie, so the sorting keeps nothing for them. It
  counts each list as four bytes
  for each entry it has reserved for and 16 more, and each id that entries name an object by
  as 128 bytes more. An entry stored as it is therefore counts 128 bytes, the first entry
  stored behind an object 160, and an entry that names an object by an id no earlier entry
  named 288. That count takes no more bytes than the byte limit. A pack that states more
  entries than the byte limit has a count for is refused before its first entry is read, and
  a pack that brings more than the count admits is refused at the entry that takes it over.
  At 2 GiB that is 16,777,216 entries stored as they are, 13,421,772 entries stored one
  behind the other in a line, or 7,456,540 entries that each name another id.
- **Objects that wait.** When the fetch computes the ids, an object that other entries are
  stored behind is held until each of them has been made, so what is held at once follows how
  the entries of the pack branch. The fetch keeps each object it holds this way at its own
  length, counts that length, and holds no more than the byte limit: the object that would
  take it over ends the fetch. The fetch computes the ids on one thread. Beside the objects
  that wait it holds what the rule of object sizes admits for the one entry it works on, and
  which objects wait at once follows the pack alone: the same pack under the same byte limit
  gets the same answer every time. Files that have nothing to do with each other never wait
  beside one another: what waits at once lies on one path, from an object stored as it is
  down to the entry the fetch works on.
- **What the fetch inflates and makes.** The fetch inflates each entry once while the pack
  arrives, to find where the entry ends. When it computes the ids it inflates each entry once
  more and makes the object of each entry stored behind another once. It counts those bytes
  while the pack arrives and when it computes the ids, each pass counted on its own against 64
  times the byte limit, and ends before the count of a pass is over. While the pack arrives it
  inflates no more of an entry than the bytes the entry states and one more; an entry that
  inflates to more than it states ends the fetch as every pack that is not well formed does,
  `workspace.git-fetch-failed`. When it computes the ids it counts the bytes before it
  allocates for them.

The first bound is held where the chains are read and the baseline is checked
(`crates/substrate-host/src/git/store`). The other three are held in `gix-pack`, the crate in
which the fetch indexes a pack, as four counts: the fourth bound is one count for each of its
two passes. The crate takes the byte limit as the most it allocates for one object
(`gitoxide.objects.allocLimit`) and refuses each of the four with the error it has for an
allocation over that limit, which the create already names `workspace.git-storage-limit`. The
create names the one thread in the same settings (`pack.threads=1`).

**What the bounds refuse of an ordinary source.** A byte limit can be no smaller
than the files of the baseline and the fetched pack together; the figures here are multiples of
that sum. Measured on 2026-11-24 on a fetch of depth 50 of five repositories and of one
generated history of 1,000 files and 50 commits (1,022 to 16,703 places, 2.2 MB to 114 MB of
baseline objects, 2,096 to 20,431 entries): whole reads 0.8 to 14.4 against the bound 64, the
table 0.006 to 0.15 against 1, the objects that wait at most 0.11 against 1, and what the
fetch makes 1.3 to 3.1 against 64. The table is the number of entries measured, at the 128
bytes an entry stored as it is counts: with its list, an entry stored behind another by its
place in the pack counts no more than 160 bytes for those 128, so the table is at most 0.19.
The objects that wait were measured on two threads, which hold
two paths at once where the one thread of a create holds one. What the entries inflate to while
the pack arrives is part of what the fetch makes when it computes the ids, so it is no more
than those 1.3 to 3.1. With a byte limit as small as that sum, the bounds refuse:

- a baseline of which a whole read of each object inflates and makes, on average by bytes,
  more than 64 times the object: one stored behind 63 entries that each make an object of its
  size;
- a fetch with fewer bytes of byte limit than 128 for each entry of its pack and 32 more for
  each object that entries are stored behind: many files of a few bytes with a long history;
- a pack in which more than the byte limit waits at once. Versions of a file stored
  one behind the other in a line hold two of them at once. Stored as a tree, with more than
  one entry behind each level, they hold one for each level: for a file of a tenth of the byte
  limit that is a tree more than 10 levels deep;
- a fetched history whose entries inflate to and make more than 64 times the byte limit: about
  sixty versions of every object of the baseline, each stored behind another.

A larger requested storage limit admits each of them.

**The copy of `gix-pack`.** The repository carries the crate as `vendor/gix-pack-0.74.2`: the
published source of the version `gix` 0.87.1 resolves, with the four counts, selected by
`[patch.crates-io]` as `vendor/axum-0.8.9` is. The change is about 250 added lines in five
files (`src/data/input/bytes_to_entries.rs`, `src/bundle/write/mod.rs`,
`src/cache/delta/tree.rs`, `src/index/write/mod.rs` and
`src/cache/delta/traverse/resolve.rs`). It adds two counters a traversal shares between its
threads, one count to the iterator of the entries that arrive and one to the loop that fills
the table, no thread and no lock. It fills the queue of the id pass from the items, with no
list of that work beside the queue, and sorts the entries by id where they lie, with their
place before the sorting as the second key: entries of one id keep the order they had, and
the index of a pack is the same bytes. It changes no public item of the crate. It is kept
this way:

- `vendor/gix-pack-0.74.2/README.substrate.md` states the change file by file, the four
  multiples, what an update must preserve, and the checksum of the published crate the copy
  was made from. The crate's licence files stay in the copy.
- The workspace names `gix` by its exact version. A `[patch.crates-io]` entry replaces only the
  version it has, and cargo builds the published crate with no more than a warning when the two
  differ. `cargo xtask check-packages` therefore refuses a `[patch.crates-io]` entry that
  `Cargo.lock` does not resolve to, and a lock that holds a second package of an entry's name
  from any source, whatever its version: the registry, a `sparse+` registry, a Git source or
  another path. A dependent of that second package would not build the entry's source. A Git
  entry is held to the one package of its reference: the same repository at another revision
  is a second package. The rule is closed: only `[patch.crates-io]` in the root `Cargo.toml`
  carries a patch, and the check refuses by name, instead of modelling them:
  - another key under `[patch]` (an index URL in any spelling, a registry name, a Git URL)
    and `[replace]` in `Cargo.toml`;
  - in a cargo configuration beneath the root (`.cargo/config.toml` or `.cargo/config`): a
    `patch` table, a `paths` key, a `[source.<name>]` table with `replace-with`, and a
    `[source.crates-io]` table with `registry`, `local-registry`, `directory` or `git`;
  - a `.cargo` directory holding both names, of which cargo reads `config` alone;
  - an entry `Cargo.lock` lists under `[[patch.unused]]`;
  - two entries that patch one package name, a renamed entry counted by its `package`;
  - an entry with neither a `path` nor a `git` source.

  One named exception admits the source replacement of
  `crates/substrate-host/tests/fixtures/cargo-test/.cargo/config.toml` while its directory
  is a workspace of its own (a `[workspace]` table in its `Cargo.toml`), no member of the
  root's and not beneath a member's `src`, so it applies to no build of this workspace; a
  `patch` table or `paths` key there is still refused. Each refusal names the file and the
  key or entry, and what to do. Outside the check are configuration outside the repository
  (a file above the root, `CARGO_HOME`, `--config`, `CARGO_*` variables) and files beneath
  a directory named `target` or `.git`.
- The cases below run the fetch of a create, so they fail with a `gix-pack` that does not hold
  the four counts, whichever version it is.
- An update of `gix` takes the published source of the `gix-pack` it resolves as
  `vendor/gix-pack-<version>`, makes the change in it again and runs the cases. The copy is
  removed with the update whose `gix-pack` passes them unchanged.

**What an ordinary create pays.** The reading of the chains keeps eighteen bytes for each
entry of the fetched pack, eight of them for this bound, and inflates the first bytes of each
entry stored behind another: for the six fetched packs above, no more than 0.02 s of processor
time. The fetch adds to its counts for each entry, and its table grows with the entries.
Indexing those six packs on two threads, with the bounds of the table and of the id pass, took
0.05 s to 2.6 s of processor time and 4 MB to 48 MB of peak resident memory. On one thread the
ids of a pack are computed one entry after the other. Measured on 2026-11-24 with the indexer
of the fetch: the pack of the generated history above (440,816 bytes) was indexed in 35 ms to
58 ms on one thread and in 27 ms to 38 ms on two, 6 ms to 15 ms of which is the arrival of the
pack; the largest pack a case builds, 123 entries that make 492 MiB, in 1.5 s to 2.3 s on one
thread and in 0.8 s to 1.6 s on two.

**The memory the indexing keeps.** On a pack within every count the fetch holds, beside its
table, four things, and each is no more than the byte limit: the pack itself, which the fetch
reads through a mapping of the pack file when it computes the ids; the objects that wait; the
instructions of the one entry it works on; and the object they make. A pack is no larger than
the byte limit, and how much of it is resident follows how far it deflates. What is measured
is the growth of the peak resident memory of the process over one indexing, the pages of the
mapped pack among it, on 2026-11-24 with the indexer of the fetch on one thread under byte
limits of 8 MiB and of 16 MiB:

- A pack that deflates: one file object of the byte limit and six entries stored behind it,
  each with instructions of the byte limit that make an object of nearly the byte limit, alone
  and beside a store of earlier objects. The pack is a small part of the byte limit, and the
  growth was 3.03 to 3.23 times the byte limit. Four cases hold it under 4 times:
  `an_indexer_within_every_bound_keeps_its_memory_under_four_times_its_byte_limit` and the
  three beside it in `crates/substrate-host/src/git/adversary_create_bounds_tests.rs`.
- A pack that does not deflate: one file object of the byte limit and one entry stored behind
  it, whose instructions the pack holds as they are and which fill what the byte limit leaves
  for the pack. The pack, the object that waits, the instructions and the object they make
  are then each nearly the byte limit at once: 3.97 times the byte limit by their lengths.
  Over the second indexing of a process the growth was 3.75 to 3.95 times the byte limit. Two
  cases hold it under 4 times:
  `an_indexer_keeps_its_memory_under_four_times_its_byte_limit_on_a_pack_that_does_not_deflate`
  and the one at 8 MiB beside it in
  `crates/substrate-host/src/git/review2_create_bounds_tests.rs`.
- The first indexing of a process also pays, once, for the pages of the program it runs: 1.9
  to 2.4 MiB, which do not follow the byte limit. The figures of the pack that deflates are
  of a first indexing and hold that cost. For the pack that does not deflate the growth over
  the first indexing was 3.98 to 4.18 times the byte limit; its two cases print that figure
  and hold the second indexing.

So the cases hold 4 times the byte limit for what the indexing keeps beside its table: on a
pack that deflates over the first indexing of a process, and on a pack that does not deflate
over the second, where the fixed cost is paid already.

**The memory of a full table.** A full table is resident beside that. Its count is two items
for an entry and the lists. What is resident was measured on 2026-11-24 with the indexer of
the fetch on one thread, in a process whose allocator gives every freed block of 128 KiB or
more back to the system:

- Between a full table under 16 MiB and one under 32 MiB the peak resident memory grew, less
  the bytes of the pack, by 98 to 106 bytes for each entry stored as it is against the 128
  counted, by 107 to 116 for each entry of a pair, an object and one entry stored behind it,
  against 144, and by 134 to 136 for each entry that names another id against 288. Three
  cases hold that growth to the count:
  `an_indexer_keeps_no_more_for_an_entry_stored_as_it_is_than_its_table_counts` and the two
  beside it in `crates/substrate-host/src/git/review2_create_bounds_tests.rs`.
- Over one indexing of a full table under 8 MiB, 16 MiB and 32 MiB the peak grew by 0.74 to
  0.94 times the byte limit for entries stored as they are and by 0.65 to 0.85 times for
  pairs, the pages of the mapped pack among it, 0.15 to 0.16 times the byte limit. Those
  figures are printed by the same cases and not held.
- The most is resident while the items are merged into one list: an entry stored behind
  another then lies twice, and the queue of the id pass, 40 bytes for each object stored as
  it is and freed before the merge, may still be resident. The anonymous memory of the
  process was sampled every half millisecond over one indexing of 60,000, of 120,000 and of
  1,000,000 entries, fifteen times each. For each entry it grew by at most 104.0, 104.1 and
  104.4 bytes where every entry is stored as it is, against the 128 counted; by 130.9, 131.5
  and 132.1 for pairs, against 144; and by 128.8, 129.7 and 130.0 where three entries are
  stored behind each object, against 136.
- Where every entry but one is stored behind another, the merge holds the items the table
  counts but one, and the sample also reads what an indexing takes that does not follow its
  table: over an indexing of 512 entries it grew by up to 270 KiB, of which the table counts
  64 to 80. Entries stored one behind the other in a line read at most 160.2, 160.1 and 160.0
  bytes for each against the 160.0 counted, 10 to 35 KiB over the count of the table; entries
  all stored behind one object read 133.9 and 132.8 against 132.4 at 60,000 and at 120,000
  entries, 88 and 48 KiB over, and 132.1 against 132.2 at 1,000,000.
- No case holds a table of up to 125,000 entries to these figures. They are read by a sample
  on a timer. The peak the system keeps for a process, which the cases above read, is not
  exact enough there: between full tables of 58,254 and of 116,508 entries in pairs it read
  95 to 100 bytes for each entry where the sample read 131 to 132.
- The daemon does not set that threshold of the allocator. Under the defaults of glibc a
  freed block may stay resident in the process, and what is then resident beside a full
  table was not measured. Computed, not measured: where the merge copies the roots while the
  queue of the id pass is still resident, about 158 bytes for each entry of a pair against
  the 144 counted. The count bounds what the indexing holds, not what the allocator keeps
  of what the indexing freed.

Over a fetch that fills a table under a byte limit of 8 MiB, the peak resident memory of the
test process, which also serves the pack, grew by 2.0 to 2.3 times the byte limit for
entries that each name another id, held under 3 times, and by 2.4 to 2.5 times for entries
stored as they are, held under 4 times. No case measures a pack that fills the table and
makes the byte limit wait at once.

**How it is proved.** One case for each of the four bounds, against a source served on
loopback: a pack within every other bound and over that one. The create is refused
`workspace.git-storage-limit`, the workspace is proved absent, and the memory of the test
process stays under a bound the case states: its peak resident memory, and for the table of
entries its peak address space, because a reservation that is never written is no resident
memory. Beside them, in `crates/substrate-host/src/git`:

- cases at the byte and at the entry where a count turns
  (`security_create_bounds_tests.rs`): a pack that makes the byte limit wait, one that makes
  64 times the byte limit and one that brings as many entries as the byte limit has a count for
  are indexed, and one byte or one entry more is refused; what the entries inflate to while
  the pack arrives is within 64 times the byte limit;
- cases of the table's count (`create_bounds_tests.rs`): the list of an entry stored behind
  another and the id an entry names are counted, each entry that arrives is counted whatever
  number the pack states, an object that waits is counted at its own length whichever entry
  was made before it, and the settings of a create's fetch name one thread;
- two packs within their byte limit that are indexed eight times each with one answer, and a
  fetch of one of them that is served (`adversary_create_bounds_tests.rs`), beside the four
  cases of the memory above;
- cases that give a pack to the indexer on each of the four ways it reads one, with the
  options the fetch gives it (`review2_create_bounds_tests.rs`): what arrives is counted to the
  byte, a pack that inflates to 64 times the byte limit arrives whole and one byte more is
  refused;
  an entry is inflated no further than the bytes it states and one more; a well-formed pack
  is indexed to the same index and pack file with a byte limit and without one, and the index
  is the one `git index-pack` writes; beside the two cases of a pack that does not deflate
  and the three of a full table above.

Each case ran under 5 s. The cases of a create that the host crate already has run against the
copy unchanged.

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
