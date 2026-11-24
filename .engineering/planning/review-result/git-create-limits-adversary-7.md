---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary-7
kind: review-result
status: active
title: 'Confirming adversary pass 7: no cargo configuration beneath the root replaces a source'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Confirming adversary pass 7: no cargo configuration beneath the root replaces a source

2026-11-24, on the commit `e5db9df59` of the branch `fix/check-packages-closed-patch-rule`. The
adversary added two cases in the test module of `xtask/src/packages.rs` and did not edit the
implementation. Each input was confirmed against cargo 1.97.1 with `cargo metadata --offline`
on a throwaway workspace. Verdict: two cases red, both in the walk that finds the files, which
the commit under review did not change. `packages` cases executed 50 to 52, red 2. S1: 0.
S2: 2. S3: 1.

The two red inputs are hand-built directories. No file of the repository has one.

## Held

- Nothing of the former exception is left in `xtask/src/packages.rs`: `check_configuration` is
  one path with one list of admitted keys.
- The two macOS test files, by reading: both import what they use, the configuration is written
  after the copy and before the profile is built, the constant is byte for byte the same in both
  files and equal to the removed file (113 bytes), and `.cargo` and `vendor` exist in the copy
  where they are sealed.
- No other test, script, workflow or document reads the removed file or copies the fixture.
- Design 21, the `check-packages` row of `AGENTS.md` and the changelog match the code; the
  changelog's count of 25 cases holds.

Not established by the pass: the two macOS test files were not compiled; the letter-case rule
was shown on a Linux tmpfs that folds case, not on APFS or NTFS.

## Findings and what was done

1. S2. Rule asked: a `.cargo` directory in another letter case is held like `.cargo`. Case
   `a_cargo_directory_in_another_letter_case_is_held_like_cargo`.
2. S2. Rule asked: a file named `config` or `config.toml` in another letter case is held like
   the file of that name. Case
   `a_configuration_file_name_in_another_letter_case_is_held_like_the_name`.
3. S3, judgement. With no directory excepted, the case
   `a_source_replacement_beneath_the_fixture_is_refused_whatever_the_root_members_are` holds
   nothing another case does not.

Done in the branch's next commit: the walk takes the directory and both file names without
regard to ASCII letter case, and the commit's message states what became of the case of
finding 3.

Runs of the pass, release profile, in the build pool: the `packages` cases 52 run, 50 passed,
2 failed (the two cases).
