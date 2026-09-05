---
format: aep.planning-md/1
id: review-result:git-workspace-quota-image-pass-1
kind: review-result
status: active
title: Quota image startup independent review
relations:
- reviews: story:git-workspace-quota-lifecycle
revision: 1
---
unit: Substrate final-image startup fix in dirty tree based on 7ef5321832dad05a523c3cae0ac2463532df81e9; scoped diff sha256 b714f4033f60d1b3e4d391b783c8db0a64a5665fcb7983c772fbee9ec816bc94
verdict: nothing found
cases: executed 0→0, red 0 (read-only review; retained implementor runs are identified below)
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: none
needs-coordinator: full gate, released-image provenance and hosted containerd/quota proof remain separately assigned

1. `git --no-pager diff --stat`

```console
 .engineering/planning/journal.jsonl                |  14 ++
 .github/workflows/release.yml                      |   4 +
 CHANGELOG.md                                       |  10 ++
 Cargo.lock                                         |  16 +-
 Cargo.toml                                         |   2 +-
 Dockerfile                                         |  11 ++
 README.md                                          |  17 ++
 THIRD_PARTY_LICENSES.html                          |  16 +-
 crates/b10x-substrate-sdk/Cargo.toml               |   4 +-
 crates/substrate-daemon/Cargo.toml                 |   6 +-
 crates/substrate-host/Cargo.toml                   |   2 +-
 .../src/git/materialization_tests.rs               |  25 ++-
 crates/substrate-host/src/lib.rs                   | 190 +++++++++++++++------
 crates/substrate-host/src/quota.rs                 |  24 +++
 crates/substrate-mcp/Cargo.toml                    |   2 +-
 crates/substrate-store/Cargo.toml                  |   2 +-
 xtask/Cargo.toml                                   |   2 +-
 xtask/src/main.rs                                  |   5 +
 18 files changed, 274 insertions(+), 78 deletions(-)
```

These are handed-off implementor/coordinator changes, including the earlier quota unit and excluded planning/version/license work. My source/test delta is empty. The bounded review covers exactly Dockerfile, README.md, .github/workflows/release.yml, xtask/src/main.rs and the new untracked xtask/src/image_startup.rs. Their complete diff is 37 tracked inserted lines plus the new 453-line verifier. The header hash is produced by this read-only command:

```console
{ git --no-pager diff -- Dockerfile README.md .github/workflows/release.yml xtask/src/main.rs; git --no-pager diff --no-index -- /dev/null xtask/src/image_startup.rs; } | sha256sum
b714f4033f60d1b3e4d391b783c8db0a64a5665fcb7983c772fbee9ec816bc94  -
```

The only file written by this pass is this assigned scratch report. No source, tests, planning, versions, build artifacts, images or deployment settings were changed, and no suite or image build was repeated.

2. Cases added

None. The coordinator requested read-only review while the final full gate runs. No concrete defect requiring a new bounded reproduction was identified.

3. Inspected runner records and read-only observations

I read image-startup-implementation-report.md and the retained baseline failure, initial worker-name failure, final image proof, final package, formatting, clippy, link and toolchain logs. These remain implementor runner records; this pass did not execute them.

The baseline check against published 0.7.3 fails at the first file case because the explicit quota executable is absent. Retained output from image-startup-red.log:

```console
    Finished `release` profile [optimized] target(s) in 0.09s
     Running `target/release/xtask check-image-startup --image 'ghcr.io/beyond10x/b10x-substrate-daemon:0.7.3'`
image startup: 0 passed; 1 failed
xtask: docker ["cp", "substrate-image-startup-01m1sjmhf34t8zhgt3dmp5rkjw:/usr/local/bin/substrate-daemon-quota", "-"]: Error response from daemon: Could not find the file /usr/local/bin/substrate-daemon-quota in container substrate-image-startup-01m1sjmhf34t8zhgt3dmp5rkjw
```

The first repaired-image run passed the file case but reported four threads and zero workers because the initial name match was wrong. Its retained output remains in image-startup-green.log. The final source matches the actual tokio-rt-worker name and retains the minimum thread/worker counts and all exact capability assertions. This was a verifier correction, not a product failure hidden by changing runtime code.

Final retained image command and output, implementor-reported exit 0:

```console
cargo run --release --locked -p xtask -- check-image-startup --image substrate-image-startup:20260905
   Compiling xtask v0.7.4 (/home/timo/.local/state/worktree/trees/b10x/substrate/projects-recovery-substrate-20260905/xtask)
    Finished `release` profile [optimized] target(s) in 5.87s
     Running `target/release/xtask check-image-startup --image 'substrate-image-startup:20260905'`
PASS final-image files: root:root 0755, byte-identical sha256=8ba55168865d17d5696dce84b76db1576b12405e9dd9d44194e30d1bbb16f7d8, default has no file capabilities, quota has only cap_sys_admin=ep
PASS default without capabilities: 4 threads (3 Tokio workers), UID/GID 65532, CapPrm/Eff=0x0, CapBnd=0x0, CapInh/Amb=0
PASS default with SYS_ADMIN bounding only: 4 threads (3 Tokio workers), UID/GID 65532, CapPrm/Eff=0x0, CapBnd=0x200000, CapInh/Amb=0
PASS explicit quota startup: 4 threads (3 Tokio workers), UID/GID 65532, CapPrm/Eff=0x200000, CapBnd=0x200000, CapInh/Amb=0
image startup: 4 passed; 0 failed
```

The final tooling package log reports 149 unit, 3 mutant and 13 release-workflow cases passed, zero failed/ignored/filtered: 165 total. The implementor's supplied pre-addition count is 161. Formatting output is empty; strict xtask clippy ends successfully in 2.20s, links report repository portability, and the toolchain checker confirms the pinned 1.97 inputs. I did not re-run those commands or the root's full gate.

This pass did execute read-only Docker image inspection:

```console
docker image inspect substrate-image-startup:20260905 --format '{{.Id}} user={{.Config.User}} entrypoint={{json .Config.Entrypoint}} revision={{index .Config.Labels "org.opencontainers.image.revision"}}'
sha256:b791b19c009ad82e6736d19f66b3fe216c27f643cb85caad992ab709033a5417 user=65532 entrypoint=["/usr/local/bin/substrate-daemon"] revision=unknown
```

Exit 0. This agrees with the retained local build's image ID and ordinary default. The image explicitly has unknown revision provenance and is not treated here as a published, signed release image.

I also ran `sha256sum --check .scratch/projects-recovery/image-startup-runtime-sources.sha256`, exit 0: lib.rs, quota.rs, git/materialization_tests.rs and git/quota_tests.rs all match the implementor's pre-startup-fix snapshot. A read-only `docker ps -a --filter name=substrate-image-startup --format '{{.ID}} {{.Names}} {{.Status}}'` returned no records. No container was created or removed by this pass.

4. Findings

Nothing found in the assigned final-image startup fix.

5. Inspected behavior and practical limits

- Capability scope: Dockerfile creates a separate copy in a dedicated preparation stage, sets root ownership/mode before cap_sys_admin=ep, and copies only that executable into the daemon runtime. The ordinary executable, default entrypoint and MCP runtime do not acquire this file capability. libcap2-bin remains in build ancestry and is not copied into either final runtime. Both binaries still originate from the one existing Cargo build.
- Final-file assertions: image_startup.rs checks the actual image entrypoint, regular-file archive metadata, root:root 0755 ownership/mode, nonempty identical executable bytes, no default file capability and an exact SYS_ADMIN-only effective/permitted xattr with empty inheritable bits. Only the expected version-2 value or a root-ID-zero version-3 value is accepted. The parser rejects truncated records, unexpected member types, multiple files and duplicate capability records within a PAX header; it preserves binary xattr bytes rather than relying on extracted-file ownership.
- Runtime assertions: the tool starts the final daemon image, waits for its ready log, then checks every enumerated task status for all four UID/GID values and all five capability masks. It requires at least three threads and at least two named Tokio workers. The default path is exercised with both zero and SYS_ADMIN-only bounding sets; the explicit quota path must have SYS_ADMIN effective/permitted with no inheritable/ambient authority. A changed mask on an observed worker fails the check as it does on the main thread.
- Resource scope and cleanup: test containers have no network, read-only root, a disposable owner-private state tmpfs, memory/pid bounds and only the selected capability. Successful cases require explicit container/volume removal; error paths retain a Drop cleanup fallback. The verifier is an operator tool for local rootful Linux Docker, where the engine's reported host PID and the runner's procfs refer to the same host. Its procfs observations are not portable evidence for a remote daemon, Docker Desktop VM or unrelated PID namespace.
- Release integration: the new command is registered in xtask and invoked after building/loading the final daemon image but before its push. The recovery branch invokes it again after pulling and checking the existing image's revision. Existing ordinary --help and extracted-binary runtime vectors remain. Failure propagates through the existing shell's set -euo pipefail. The new check neither publishes an artifact itself nor relaxes immutable-tag, gate, signature or source-label checks.
- Claims remain bounded: README explicitly distinguishes image startup masks from filesystem quota facts and a served execution/terminal backend. It preserves the trusted launcher's no_new_privs boundary and correctly states that setting it does not itself clear capabilities at fork. No daemon/application startup, Tokio initialization or child-launch source was changed by this fix.

The four Docker cases do not pass project-quota IDs or mount an enforced quota filesystem; they therefore establish startup authority, not quota service or EDQUOT. They also neither set nor inspect NoNewPrivs, so their result is not a complete verification of the Devcenter chart's security context. The separately assigned hosted proof must check the actual containerd process/thread masks, selected executable/image, NoNewPrivs policy and real quota facts/operations. These are explicit limits of this review, not an assertion that the local checks exercised those scenarios.

6. Paths written outside the assigned worktree

None. Only .scratch/projects-recovery/substrate-image-startup-adversary-report.md was written. No test source, image, build/cache output, temporary resource, container or background process was created by this pass. The existing image/build resources remain coordinator-owned. No costs were exposed by the tools.

7. Findings block

```findings
[]
```
