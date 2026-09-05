---
format: aep.planning-md/1
id: story:git-workspace-quota-lifecycle
kind: story
status: active
title: Enforce hard quotas throughout Git workspace materialization
relations:
- informed_by: story:materialize-connector-git-sources
- informed_by: story:workspace-and-scratch-quotas
scope:
- confidence: cited
  path: .github/workflows/release.yml
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: README.md
- confidence: cited
  path: THIRD_PARTY_LICENSES.html
- confidence: cited
  path: crates/b10x-substrate-sdk/Cargo.toml
- confidence: cited
  path: crates/substrate-daemon/Cargo.toml
- confidence: cited
  path: crates/substrate-host/Cargo.toml
- confidence: cited
  path: crates/substrate-host/src/git/materialization_tests.rs
- confidence: cited
  path: crates/substrate-host/src/git/quota_tests.rs
- confidence: cited
  path: crates/substrate-host/src/lib.rs
- confidence: cited
  path: crates/substrate-host/src/quota.rs
- confidence: cited
  path: crates/substrate-mcp/Cargo.toml
- confidence: cited
  path: crates/substrate-store/Cargo.toml
- confidence: cited
  path: xtask/Cargo.toml
- confidence: inferred
  path: xtask/src/image_startup.rs
- confidence: inferred
  path: xtask/src/main.rs
revision: 11
---
## Outcome

Git workspace materialization honors the existing hard byte and inode quota contract from staging through atomic installation, observation, restart and destruction. O1 confinement remains a hard floor and O4 consumers can open coding workspaces on a correctly configured host.

## Confirmed defect

The published Git creation path does not call ProjectQuotas::apply. It fetches into an ordinary temporary directory, reports a bounded directory scan as storage usage, and then observation asks the quota manager about project ID zero. A host with quota capabilities therefore cannot provide the promised Git workspace lifetime correctly. This was discovered during authenticated hosted validation; deployment-specific identifiers remain outside this public store.

## Scope

- cited: crates/substrate-host/src/lib.rs, crates/substrate-host/src/quota.rs and crates/substrate-host/src/git/materialization_tests.rs.
- inferred: focused sibling Rust tests if that improves isolation, and quota recovery integration tests.
- coordinator-owned: Cargo.toml, Cargo.lock, CHANGELOG.md and existing package version declarations for an immutable repair release.

## Acceptance

Reproduce the missing quota attachment before implementation. Attach an enforced quota before the first Git write, preserve its identity through atomic rename and update allocator path tracking, and release quota allocation only after failed or cancelled staging cleanup proves absence and zero accounted usage. Successful create and observe must return kernel accounting; restart must recover the same allocation, and destroy must release it. Test byte and inode enforcement plus failure cleanup on a real quota-capable filesystem; a skipped delegated test is not a passing live proof. Preserve v2, exact admitted commit/reference/depth, TLS and credential isolation, all confinement and existing refusal distinctions. In particular ENOSPC is not relabeled EDQUOT merely to admit XFS.

## Delivery

This repair continues the operator-authorized coding workspace implementation, publication, deployment and personal browser validation. One implementor, independent adversarial review, full repository gate and exact immutable release precede deployment. No new wire contract or capability is introduced; this corrects implementation of ADR0020 and the existing Git workspace contract.

## Validation and release evidence

The implementation first reproduced the missing quota attachment: one portable guard failed before Git discovery and all five original enforced ext4 cases failed against the original code. Independent adversarial tests extend the enforced lane to seven cases covering attach-before-write, byte/inode refusal, independent live workspace accounting, cancellation/fetch/install-conflict cleanup, restart, destruction and released identity reuse. An intentional stale allocator-path mutation was killed by the immediate destruction assertion before restoration. Review found nothing, recorded verbatim in review-result:git-workspace-quota-lifecycle-pass-1.

The complete repository gate passes on the 0.7.4 source: 582 Cargo cases passed, with eight explicitly ignored delegated cases in the portable run. The seven new real-filesystem cases were then explicitly executed against the final release test binary and passed 7/7 with zero ignored; the existing external-proxy ignored case was not executed. Formatting, strict all-target Clippy, links, ADRs, full-history secret scan, advisory and license checks, package boundaries, all sixteen immutable bundles and JSON/toolchain checks pass. Local version changes affect only eight workspace Cargo.lock packages. The development wire bundle remains 0.16.0.

The published predecessor image fails the new final-image check because the explicit quota executable is absent. The corrected local final image passes all four checks: root-owned 0755 byte-identical executables with only the quota copy carrying cap_sys_admin=ep, unprivileged default startup, inactive ordinary startup with only the SYS_ADMIN bounding bit, and explicit quota startup with the exact bit in every daemon/worker permitted, effective and bounding set. Inheritable and ambient sets remain empty. The tooling package passes 165 cases. Independent read-only image review found nothing and is recorded in review-result:git-workspace-quota-image-pass-1. These local Docker checks do not prove the hosted chart's complete security context, live filesystem quotas or terminal child confinement.

The exact-main hosted CI result, immutable release and live containerd/quota/application validation remain pending. Deployment-specific disk, node bootstrap, data migration and browser evidence belong to the downstream deployment and coordination stores.

## Quota image startup profile

The hosted filesystem now mounts with enforced ext4 project quotas after the private operator supplied its missing vendor kernel modules. The final daemon process still has zero effective/permitted capabilities despite a SYS_ADMIN bounding set and non-root Kubernetes security context: ordinary non-root exec drops those sets without a file capability. The proof was obtained from the published image under containerd and its machine document correctly withholds quota facts.

Provide a byte-identical, root-owned daemon-quota executable carrying only cap_sys_admin=ep in the existing daemon image. The ordinary executable and default entrypoint remain without file capabilities. The generic chart selects the quota path only for its existing explicit project-quota opt-in, with SYS_ADMIN in the bounding set and no_new_privs disabled as required by Kubernetes. This avoids a privileged root daemon, ambient/inheritable capabilities, SYS_RESOURCE, or a new Rust capability-activation/thread-startup path. Keep the existing child no_new_privs boundary. Prove final-image executable bytes/ownership/xattrs, default unprivileged startup, the quota process and worker-thread masks, and actual filesystem quotas under the hosted runtime before migration. Image and chart command are deployed and rolled back together. No new wire contract is introduced.
