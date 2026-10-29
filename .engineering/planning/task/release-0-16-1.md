---
format: aep.planning-md/3
id: task:release-0-16-1
kind: task
status: implemented
title: Release 0.16.1
relations:
- informed_by: task:release-0-16-0
- delivers: epic:kubernetes-deployment-and-driver
- supersedes: task:release-0-16-0
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T21:07:11Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T21:07:11Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-29T21:07:11Z", actor: "human:timo", revision: 4}
---
## Outcome
Publish Substrate 0.16.1: everything listed under CHANGELOG.md `[0.16.0]` (the release of `wave/k8s-namespace-1`, with the Kubernetes namespace driver of design 34 and ADR 0047, the executor image, the development bundle `substrate-wire/0.25.0` and the host execution AppArmor profile `substrate-host-exec-v2`), plus `libz-sys` 1.1.31 for RUSTSEC-2026-0418.

## Why this replaces task:release-0-16-0
Tag `0.16.0` is annotated and pushed at `541cf55eb3efa7977aad6721f697668d1f029c28` (2026-10-29: `git ls-remote origin 'refs/tags/0.16.0*'`). It will never publish: `release.yml` requires a `success` conclusion of `gate.yml` for the tagged commit, and `main`'s gate on that commit, run 40911403183 (https://github.com/beyond10x/substrate/actions/runs/40911403183), concluded `failure` because `cargo xtask check-advisories` refused RUSTSEC-2026-0418 (`libz-sys` 1.1.29; patched `>= 1.1.31`; advisory dated 2026-10-28). The advisory was published after the pull request's gate passed and before `main`'s gate ran. Release tags are write-once, so `0.16.0` cannot be moved to a fixed commit. 0.16.1 is therefore the first published release of the namespace driver. This task is informed by `task:release-0-16-0`, whose scope and stated limits it carries; it does not move that task's status.

## Advisory exposure
The leak the advisory describes prints the build environment only when `libz-sys` <= 1.1.30 is compiled with `cc` >= 1.6.0 on a Unix host. `Cargo.lock` at `541cf55` holds `cc` 1.4.4, as does every revision of `Cargo.lock` on `main` since 2026-09-25 (40 commits, read from `git log origin/main -- Cargo.lock` and `git show <sha>:Cargo.lock`), so this workspace's locked builds did not print it. `libz-sys` reaches the workspace through `curl-sys` and `libgit2-sys` under `substrate-host`.

## Stated limits
Unchanged from 0.16.0, and repeated in the 0.16.1 release notes: every exec start on the Kubernetes driver is refused `exec.sandbox-unavailable`; exec on a cluster is unverified (design 34 § 10.3 O3); shared clean-room conformance follows in the next release (`story:remote-clean-room-conformance`).

## Scope and verification
`Cargo.lock` (`libz-sys` 1.1.29 -> 1.1.31 and the workspace packages), `Cargo.toml`, the exact internal version edges in `crates/*/Cargo.toml` and `xtask/Cargo.toml`, `THIRD_PARTY_LICENSES.html` (regenerated with cargo-about 0.9.1), `CHANGELOG.md` `[0.16.1]`, the generated release notes page, and the documents that named 0.16.0 as the coming release (`AGENTS.md`, `README.md`, `STATUS.md`, `website/docs/status.md`, `website/docs/guides/deployment.md`). Patch, not minor: one dependency fix over the unpublished 0.16.0. Contract bundle versions are unchanged. Verified by the pull request's full CI gate, then, at the exact tagged commit, the delegated lane, the project-quota lane, the chart lane and the namespace lane, an annotated bare version tag `0.16.1` with its `Confinement-lane:`, `Quota-lane:`, `Chart-lane:` and `Namespace-lane:` records, the release workflow, signatures and artifact read-back.

## Published

Tag `0.16.1` (object `0218c70b`) names `f2fd606b88a0b778970e8babe9a96eb600cd57b2`, the merge of pull request 196. Its message carries `Confinement-lane:` 182, `Quota-lane:` 7, `Chart-lane:` 4 and `Namespace-lane:` 8, each at that commit.

Release run 40949928365 (https://github.com/beyond10x/substrate/actions/runs/40949928365) failed on attempt 1 in Preflight with exit 141. The likely cause, inferred and not reproduced: SIGPIPE from the manifest-version check (`release.yml:214-222`). That check pipes `git show` into `awk`, but also passed `Cargo.toml` as an extra argument, so awk read the file and could exit before `git show` finished writing. Attempt 2 succeeded at 2026-10-29T21:03Z. The workflow keyless-signed and verified the daemon, MCP, executor and chart images and the `substrate-wire/0.25.0` bundle against `release.yml@refs/tags/0.16.1`. It published the two binary archives, `SHA256SUMS` and `SHA256SUMS.sigstore.json`. The digests are in `CHANGELOG.md` `[0.16.1]`. Each digest's manifest and signature tag resolved anonymously from GHCR.

The bot edited the GitHub release notes so they begin with the `CHANGELOG.md` `[0.16.1]` section, which says that exec on a cluster is unverified and that shared clean-room conformance follows. The workflow's generated artifact text follows it unchanged.

The pull request that records the digests also reconciles the documentation (README, STATUS, ROADMAP, AGENTS, architecture overview and website pages) to 0.16.1 and the namespace driver. It also drops the extra `Cargo.toml` argument at `release.yml:222`.

This task supersedes `task:release-0-16-0`, which is archived because its tag can never publish.
