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

## Observations

Substrate publishes a bounded baseline-file read and a path-sorted bounded Git change-set
observation computed against the exact materialized commit kept in host-private metadata. Tree and
current-file reads remain lazy and paged. The observation
does not disclose `.git`, Connector authority, remote credentials, or remote URL userinfo.

## Refusals

The served seam is closed and names the failed guarantee:

| Condition | Refusal |
|---|---|
| deployment has no matching configured source | `workspace.git-source-unserved` (`unserved`) |
| locator is outside the exact scheme/host/port/path-segment aperture, carries userinfo, or needs encoded-path normalization | `workspace.git-locator-refused` (`refused`) |
| source authority is absent or malformed | `workspace.git-authority-absent` / `workspace.git-authority-invalid` (`refused`) |
| branch no longer resolves to the admitted commit | `workspace.git-commit-moved` (`refused`) |
| transfer, installed bytes or installed inode count crosses its admitted ceiling | `workspace.git-transfer-limit` / `workspace.git-storage-limit` (`exhausted`) |
| a Git-only observation is requested from a non-Git workspace | `workspace.git-workspace-required` (`refused`) |
| baseline file or aggregate change query exceeds its declared bound | `workspace.git-baseline-limit` (`exhausted`) or `request.schema-invalid` (`refused`) |
| local Git, TLS trust, checkout, fsync, installation, private baseline or reconciliation machinery fails | a stage-specific `workspace.git-*-failed` (`failed`), with no library text or authority bytes |

## Consequences

Workspace can bind one canonical materialization instead of maintaining base and working copies.
The filesystem and Git index become the source of editor, terminal, tree, and diff truth. The Git
source schema is no longer aspirational: unsupported deployments omit the capability and configured
deployments serve it end to end.
