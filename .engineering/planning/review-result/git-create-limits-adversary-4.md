---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary-4
kind: review-result
status: active
title: 'Adversary pass 4: the patch rule of check-packages after its first widening'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Adversary pass 4: the patch rule of `check-packages` after its first widening

2026-11-24, on the fourth commit of the branch `fix/git-create-work-and-memory-limits`
(`240dfa35e`), ordered by the operator's session before the merge question of the pull request
that carries the branch. The adversary added six cases in the test module of
`xtask/src/packages.rs` and did not edit the implementation. Each input was confirmed against
cargo 1.97.1 on offline probe workspaces (`cargo metadata --offline`,
`cargo generate-lockfile --offline`). Verdict: six cases red, from four causes; one judgement
finding. `xtask` cases executed 731 to 737, red 6. S1: 0. S2: 4. S3: 1.

Every input is a hand-built manifest or configuration. No file of the repository has one.

## Held

- A `[patch]` table in a member's manifest: cargo ignores it, and so does the check.
- The keys `registry+…` and `sparse+https://index.crates.io/`: cargo does not apply them.
- Two path entries of one name with different versions are refused.
- A path dependency beside a patched registry dependency of the same version: cargo itself
  refuses the pair.
- A Git entry whose `rev` is a prefix of another revision: the suffix must be empty or start
  with `#`.

Not established by the pass: whether the lock file writes an entry's Git URL as the manifest
writes it; configuration outside the repository (`--config`, `CARGO_*` variables, a file above
the root).

## Findings and what was done

The rule asked by all four: `[patch.crates-io]` in the root `Cargo.toml` is the one place the
workspace replaces a crates.io package, and each entry is held to `Cargo.lock`.

1. S2. Rule asked: one package name is patched by one entry, and an entry the lock file lists
   under `[[patch.unused]]` is refused. Cases
   `two_patch_entries_of_one_name_and_version_are_refused` and
   `a_patch_entry_the_lock_file_lists_as_unused_is_refused`.
2. S2. Rule asked: a `paths` key of a cargo configuration is refused. Case
   `a_path_override_of_a_patched_name_in_the_cargo_configuration_is_refused`.
3. S2. Rule asked: a patch of crates.io under another key is refused: an index URL in any
   spelling, or the name of a registry whose index is crates.io's. Two cases.
4. S2. Rule asked: a cargo configuration beneath the root, in any directory, is read. One case.
5. S3, judgement. Where a `.cargo` directory holds both `config` and `config.toml`, cargo reads
   the first alone; the check read both.

Done in the branch `fix/check-packages-closed-patch-rule`, on the decision of the operator's session
(fix before the merge): the rule is closed instead of widened a second time. The check admits a
patch in `[patch.crates-io]` of the root `Cargo.toml` alone and refuses by name: another key
under `[patch]`, `[replace]`, a `patch` table or a `paths` key in a cargo configuration beneath
the root, a `.cargo` directory holding both configuration names, an entry under
`[[patch.unused]]`, two entries of one package name, and an entry with neither a `path` nor a
`git` source. The six cases are in the change; four state the refusal in their names.

Found by the session's own read of that change, not by the pass: a `[source.<name>]` table
with `replace-with`, and a `[source.crates-io]` table with `registry`, `local-registry`,
`directory` or `git`, replace every package of crates.io beside any patch. Both are refused in
every cargo configuration beneath the root, with one named exception that the check proves on
every run: the fixture workspace `crates/substrate-host/tests/fixtures/cargo-test`, whose
configuration applies to no build of this workspace. Five cases.

Runs of the pass, release profile, in the build pool: `xtask` 737 run, 731 passed, 6 failed
(the six cases); no new case over 0.06 s. Runs of the change: the `packages` cases 43 of 43,
`xtask` 746 of 746.
