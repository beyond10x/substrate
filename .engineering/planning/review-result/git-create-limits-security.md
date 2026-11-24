---
format: aep.planning-md/3
id: review-result:git-create-limits-security
kind: review-result
status: active
title: 'Security review: the work and the memory of a Git create follow its byte limit'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Security review: the work and the memory of a Git create follow its byte limit

2026-11-24, on the first commit of the branch `fix/git-create-work-and-memory-limits`
(`3a2504977`, base `16ddbec11`). The reviewer added seven cases in
`crates/substrate-host/src/git/security_create_bounds_tests.rs` and did not edit the
implementation. Host Git cases executed 144 to 151, one red.

## Held

- The bound of the objects that wait and the bound of what the id pass makes each turn at the
  byte: a pack at the bound is indexed and one byte more is refused, on one thread and on two,
  and in a build without the crate's `parallel` feature.
- A refused pack leaves no `.pack` and no `.idx`; through the driver the workspace is absent.
- The limit the fetch is given is the create's own, never absent.
- A pack that states as many entries as the default limit holds reserves no table for them.
- The vendored copy is the published archive (its checksum is the lock's) plus the files
  `README.substrate.md` lists. No new dependency, no public item changed.
- Whole reads are counted before each read.

## Findings and what was done

1. Rule asked: every byte a fetch inflates is counted by one of its bounds, the pass that runs
   while the pack arrives included. Done in the second commit: that pass has a count of its own
   of 64 times the byte limit, and an entry is inflated to the bytes it states and one more.
   The reviewer's case
   `what_a_fetch_inflates_while_its_pack_arrives_is_within_64_times_its_byte_limit` holds it,
   unchanged.
2. Rule asked: the table bound counts what is kept per entry. Done: an entry counts two items,
   the list it joins and the id it names (128, 160 or 288 bytes, as of the third commit), and design 21 states the three
   figures at 2 GiB.
3. Rule asked: `cargo xtask check-packages` refuses a lock that holds a package with
   the name of a patched one. Done, with a refusal case for each source kind.
4. Rule asked: the same pack under the same limit gets the same answer in every run. Done: the
   fetch of a create computes ids on one thread.

## Not established

The bounds of the id pass through the driver (they need project quotas; they return through
the same error as the table bound), the per-entry table check with a non-empty object store,
and macOS.
