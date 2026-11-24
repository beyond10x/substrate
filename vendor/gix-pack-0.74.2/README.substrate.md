# Substrate gix-pack bounds patch

This directory is the published `gix-pack` 0.74.2 crate, the version `gix` 0.87.1 resolves. It was
made from the crate whose crates.io checksum is
`3d199d515cbdcf05f531f3859be9771fc12acac7b7fe0f5b581b2c1ab1c3a61e`. Substrate adds three bounds
to the indexing of a pack that arrives. Each is a multiple of the allocation limit the caller
gives (`gitoxide.objects.allocLimit`), the most the crate allocates for one object; a caller that
gives none gets none of the three. Indexing over a bound ends with the error the crate has for
an allocation over that limit, `traverse::Error::OutOfMemory`. No public item of the crate
changes. The change adds two counters the threads of a traversal share, no thread and no lock.

| bound | multiple of the limit | held in |
|---|---|---|
| the bytes of the table of entries | 1 | `src/index/write/mod.rs` |
| the bytes reserved for the objects that wait for the entries stored behind them | 1 | `src/cache/delta/traverse/resolve.rs` |
| the bytes the traversal inflates and makes | 64 | `src/cache/delta/traverse/resolve.rs` |

## What differs from the published crate

- `src/cache/delta/tree.rs`. `Tree::with_capacity` reserves for no more than `RESERVED_ITEMS`
  (4,096) items in each of its two lists, 8,192 in all, whatever count it is given; the lists
  grow with the items that are added. `Tree::bytes_of_items` states the bytes a count of items
  takes.
- `src/index/write/mod.rs`. `items_over_limit` holds the first bound: the items of the tree take
  no more bytes than the limit. It is asked for the count the pack states, before the first
  entry is read, and for each entry that arrives.
- `src/cache/delta/traverse/resolve.rs`. A `Budget` holds the two counters and the limit, and
  the functions of the traversal take it where they took the limit.
  - `Budget::hold` holds the second bound. An object that entries are stored behind is kept
    until each of them has been made; its reserved bytes are counted when it is kept and given
    back when it is dropped (`Charge`). The object that would take the count over the limit
    ends the traversal.
  - `Budget::resolve` holds the third bound. The bytes an entry inflates to and the bytes of the
    object a delta makes are counted before they are allocated, each once; the traversal ends
    before the count is over `RESOLVED_BYTES_PER_LIMIT` (64) times the limit.
  - `resize_with_limit` reserves exactly the bytes it is asked for, so the bytes counted for a
    kept object are its own.
- `rustfmt.toml` is Substrate's: `max_width = 120`, the width the crate's source is written in.
  The workspace's `cargo fmt --all --check` reads a `[patch]` source, and with this file the
  published source is what it expects.
- This file.

Every other file is the published one, `.cargo_vcs_info.json` and `Cargo.toml.orig` included.

## Why

The work and the memory of a Git create follow its byte limit (design 21, § *The baseline
store*). A create gives its byte limit as the allocation limit, and the three bounds make the
table of entries, the objects that wait and the bytes the indexing makes follow it. The host
crate's cases in `crates/substrate-host/src/git/create_bounds_tests.rs` run a fetch against a
pack within every other bound and over one of them, and state the refusal and the memory of the
test process.

## What an update of gix must preserve

`cargo xtask check-packages` refuses the `[patch.crates-io]` entry when `Cargo.lock` does not
resolve `gix-pack` to this directory, so an update of `gix` that resolves another `gix-pack`
fails the gate until the entry is moved. Such an update takes the published source of the
`gix-pack` it resolves as `vendor/gix-pack-<version>`, makes the change above in it again and
runs those cases. The copy is removed with the update whose published `gix-pack` passes them
unchanged.

The upstream crate's licences remain in `LICENSE-APACHE` and `LICENSE-MIT`.
