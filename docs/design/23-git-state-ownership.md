# Design 23: Git inactivity without configured sources

## Scope

A host with an empty `HostConfig.git_sources` list serves no Git source. Its Git-specific
initialization, observations and cleanup must therefore leave existing Git metadata untouched.
This includes hosts that adopt workspaces beneath a parent borrowed from a caller.

## Existing behavior and correction

`HostDriver::open` previously created and chmodded `.substrate-git-baselines` and invoked
Git reconciliation even with no sources configured. Reconciliation could remove siblings based
on their names. Git-only observations read same-name baseline files, and workspace destruction
could remove them, despite Git being disabled.

Guard initialization and reconciliation with the configured-source condition already used by
the capability probe. Refuse both Git-only observations with the existing
`workspace.git-workspace-required` result before resolving or reading a baseline path.
Skip baseline removal during workspace destruction when sources are disabled. Normal workspace
creation, filesystem operations, quotas, capsule initialization and execution retain their
existing paths.

## Compatibility and limits

No public type, error vocabulary, wire bundle, dependency or persisted format changes.
Configured-Git startup, materialization, cancellation, quota handling and recovery retain the
published implementation. This correction does not establish ownership for configured-Git state
or resolve its broad-prefix cleanup concern. It must not be described as a general safe-recovery
implementation.

An earlier unpublished ownership/recovery design was withdrawn from this change. Its independent
source and test evidence is preserved outside the active source tree; none of its new storage
formats, kernel-handle requirements or quota mechanisms is part of this correction.

## Verification

Eight public-API regressions use exclusively disposable temporary parents. They cover absent
baseline state, unrelated prefixed entries, existing baseline directory reads and metadata,
ordinary baseline files, dangling and existing-target symlinks, Git-only observations, and empty
workspace destruction preserving foreign baseline metadata.

The directory-read test calibrates an inotify watch before observing startup. Other checks compare
complete sentinel bytes and relevant filesystem metadata. Existing configured-Git and quota tests
remain unchanged. Run the eight regressions and the repository's complete `bash scripts/gate.sh`
before publication; no quota fixture or configured-Git recovery redesign is needed for this scope.
