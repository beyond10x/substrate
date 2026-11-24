---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary-5
kind: review-result
status: active
title: 'Confirming adversary pass 5: the configuration rule of check-packages'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Confirming adversary pass 5: the configuration rule of `check-packages`

2026-11-24, on the commit `b003a0457` of the branch `fix/check-packages-closed-patch-rule`. The
adversary added four cases in the test module of `xtask/src/packages.rs` and did not edit the
implementation. Each input was confirmed against cargo 1.97.1 on offline probe workspaces
(`cargo metadata --offline`). Verdict: four cases red. `xtask` cases executed 746 to 750,
red 4. S1: 0. S2: 3. S3: 2.

Every input is a hand-built configuration or manifest. No file of the repository has one.

## Held

- `[registries.crates-io]`, `[registry] index` and `[registry] default`: cargo ignores or
  refuses them; resolution does not change.
- `replace_with`, `Replace-With` and `[source.CRATES-IO]`: no replacement.
- A `[source]` table keyed by an index URL: no effect.
- `[env] CARGO_SOURCE_CRATES_IO_REPLACE_WITH`: no effect on cargo itself.
- A patch entry with both `path` and `git`: a cargo error.
- A member spelled through `..` into a directory with its own `[workspace]`: a cargo error.
- A `.cargo/config` linked to `/dev/null` beside `config.toml`: the check refuses.

Not established by the pass: whether the `toml` crate the check uses reads a byte-order mark;
the walk's handling of a `.cargo` that is a link and the TOML spellings (dotted, inline, quoted)
were read, not run; a `config` that is a FIFO or a device.

## Findings and what was done

1. S2. Rule asked: a cargo configuration beneath the root that includes another file is
   refused. Case `a_cargo_configuration_that_includes_another_file_is_refused`.
2. S2. Rule asked: an alias that passes configuration, from a file or inline, is refused. Cases
   `an_alias_that_passes_a_configuration_file_is_refused` and
   `an_alias_that_passes_a_source_replacement_inline_is_refused`.
3. S2. Rule asked: the fixture workspace's exception does not hold while a root member lies
   beneath the fixture directory. Case
   `a_root_member_beneath_the_fixture_workspace_voids_the_exception`.
4. S3. The printed line and the four documents listed what is not read without included files
   and alias flags, and said the exception is proved on every run.
5. S3, kept. A `.cargo/config` that is a link to `config.toml` is refused as two names, where
   cargo reads the one file. The refusal stays; the text says "a link included".

Done in the branch's next commit, on a decision of the session that works this repository: the
list of refused keys is replaced by a list of admitted ones, because a list of the keys cargo
reads was incomplete in three passes. A cargo configuration beneath the root holds only `alias`
and `profile`; the fixture's file, while its exception holds, only `net` and `source`; every
other top-level key is refused by its name, `include` in both spellings among them. An alias is
a string or an array of strings without a `--config` token; a probe showed that cargo takes no
abbreviation of the flag. The exception also fails for a root member beneath the fixture
directory and for a workspace path the comparison cannot place. The text says what the check
tests. Ten cases, the pass's four among them.

Runs of the pass, release profile, in the build pool: `xtask` 750 run, 746 passed, 4 failed
(the four cases). Runs of the change: the `packages` cases 53 of 53, `xtask` 756 of 756.
