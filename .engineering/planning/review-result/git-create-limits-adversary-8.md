---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary-8
kind: review-result
status: active
title: 'Confirming adversary pass 8: a cargo configuration is found in every letter case'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Confirming adversary pass 8: a cargo configuration is found in every letter case

2026-11-24, on the commit `f1c371980` of the branch `fix/check-packages-closed-patch-rule`. The
adversary added one case in the test module of `xtask/src/packages.rs` and did not edit the
implementation. Verdict: one case red, in how a found file is read, which the commit under
review did not change. `packages` cases executed 52 to 53, red 1. S1: 0. S2: 0. S3: 1.

The red input is two hand-built directories on a Linux file system, one with a name that is
not UTF-8. No file of the repository has one, and a file system that refuses such names
cannot hold it.

## Held

- Every spelling is read and refused: `config` beside `config.toml` in the form it had;
  `.cargo/config.toml` beside `.cargo/Config.toml`; `.cargo` beside `.Cargo`, grouped by the
  directory that holds them; three files, all named; a link named `CONFIG` to a file; a
  `.Cargo` that is a link to a directory.
- A directory named `config.toml` is no configuration file, and an entry whose name is not
  UTF-8 beside a real file is no configuration name; cargo reads neither.
- The summary counts the files read.
- No input was found that the parent commit refused and this commit passes.
- The case removed for pass 7's finding 3 is held by
  `a_source_replacement_and_net_in_the_fixture_directory_are_refused_like_any_other` and
  `a_source_replacement_in_a_member_cargo_configuration_is_refused`.
- Design 21, the `check-packages` row of `AGENTS.md` and the changelog match the code; 27
  cases, 26 on throwaway workspaces and one on this tree.

Not established by the pass: the letter-case rule on APFS or NTFS themselves; whether cargo
builds from inside a directory whose name is not UTF-8 (`cargo metadata --offline` ran there
and applied that directory's configuration).

## Findings and what was done

1. S3. Rule asked: every cargo configuration file is read from the path the walk found it at,
   also beneath a directory whose name is not UTF-8. Case
   `a_configuration_beneath_a_directory_name_that_is_not_utf8_is_read_from_its_own_path`.
2. Note, no case. `.Cargo/config` beside `.Cargo/config.toml` is refused; its message is to say
   that cargo reads `config` alone, as for `.cargo`.

Done in the branch's next commit: both.

Runs of the pass, release profile, in the build pool: the `packages` cases 53 run, 52 passed,
1 failed (the case).
