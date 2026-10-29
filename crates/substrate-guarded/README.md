# b10x-substrate-guarded

The guarded workspace file access every Substrate driver serves its file routes with: each path is
opened beneath a pinned root descriptor with `openat2` and no symlink, magic link or mount
crossing. The package also holds the driver error type, the listing result that counts omitted
entries, and the closed non-secret shape every run's environment is held to.

It depends on `b10x-substrate-wire` and nothing else in this workspace, so a binary that must not
link the host driver can still use it. `b10x-substrate-host` re-exports its types.

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/docs/concepts/adapters) for
its current status.
