# b10x-substrate-host

The Linux host driver used by the Substrate daemon to implement confined workspaces and bounded
processes. Direct driver calls do not provide the daemon's durable operation ledger, authenticated
Unix-socket boundary, events, or recovery; applications should use `b10x-substrate-sdk`.

## Features

Two features, both on by default, so every consumer that names none keeps the whole driver:

| feature | enables | without it |
|---|---|---|
| `git` | Git workspaces: `gix`, `git2` and a statically linked `curl` | no `workspace.git` fact is published, and a Git source is refused `workspace.git-source-unserved` before dispatch |
| `kubernetes` | the `pod` confinement adapter: `b10x-substrate-kubernetes-client` | every open of `pod` is refused `confinement.adapter-unserved`, naming the feature |

A consumer that needs only `substrate_host::confinement` (the `local`, `linux`, `macos`,
`systemd` and `oci` adapters) sets `default-features = false` and takes neither; its lockfile then
holds no Git stack, no `openssl-sys` and no Kubernetes client. The public items are the same in
every build, so the features are additive. `b10x-substrate-cli` depends on the driver this way.

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/) for its current status.
