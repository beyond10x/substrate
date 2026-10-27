---
title: Getting started
description: Install the b10x-substrate command line, run one command under confinement and see a named refusal.
---

# Run one command under confinement

This page takes a Linux host from nothing to one confined command, with no daemon. The
`b10x-substrate` command line opens a [confinement adapter](./concepts/adapters.md) in its own
process and runs the command inside it.

## Prerequisites

- Linux on x86_64.
- For the `oci` adapter: Docker.
- For the `linux` adapter: bubblewrap and a systemd user session on cgroup v2.

## Install the command line

From release 0.14.0, each release carries the command line and `SHA256SUMS`:

```bash
version=0.14.0
asset="b10x-substrate-$version-x86_64-unknown-linux-gnu.tar.gz"
base="https://github.com/beyond10x/substrate/releases/download/$version"
curl -fLO "$base/$asset" -fLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "$asset"
```

Put the `b10x-substrate` binary from the archive on your `PATH`.

For an earlier release, build it from its tag with Rust 1.97 or newer:

```bash
cargo install --git https://github.com/beyond10x/substrate --tag 0.13.0 b10x-substrate-cli --locked
```

## Preview a run

With Docker, pull an image and name it by digest:

```bash
docker pull alpine:3.22
image=$(docker inspect --format '{{index .RepoDigests 0}}' alpine:3.22)
mkdir job
b10x-substrate confine run --backend oci --image "$image" --dir job --dry-run -- /bin/echo hello
```

The adapter never pulls an image itself.

`--dry-run` admits the run as a real run would, prints the command the adapter would start and
runs nothing. Here that is a `docker create` with no network, no capabilities, a read-only root
and `job` at `/workspace`.

## Run a confined command

Drop `--dry-run`. The command writes to `job`, mounted at `/workspace`:

```bash
b10x-substrate confine run --backend oci --image "$image" --dir job \
  -- /bin/sh -c 'echo hello > /workspace/out.txt'
cat job/out.txt
```

### Without Docker

The `linux` adapter runs the command under bubblewrap, in a private cgroup beneath a delegated
cgroup v2 root. Ask your systemd user manager for a delegated scope with a shell in it:

```bash
systemd-run --user -p Delegate=yes --scope bash
```

In that shell, move the shell out of the scope's root, which must hold no process. Then enable
the controllers and name the root:

```bash
root="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
mkdir "$root/shell"
echo $$ > "$root/shell/cgroup.procs"
echo "+cpu +memory +pids" > "$root/cgroup.subtree_control"
export SUBSTRATE_CONFINE_CGROUP_ROOT="$root"
b10x-substrate confine run --dir job -- /bin/sh -c 'echo hello > /workspace/out.txt'
```

## See a named refusal

Ask for something the adapter cannot prove, such as the host network:

```bash
b10x-substrate confine run --backend oci --image "$image" --dir job --net -- /bin/true
```

```text
b10x-substrate: confinement.fact-unserved: host_network
```

It exits 2 and runs nothing. Substrate refuses by name; it never runs with weaker isolation. An
image named by tag is refused the same way, as `confinement.image-unpinned`.

## Next steps

- [Command line](./guides/cli.md#run-under-a-confinement-adapter-without-a-daemon): every
  `confine run` flag, and the fixed bounds.
- [Confinement adapters](./concepts/adapters.md): what each adapter enforces and declares.
- [Run a bounded command](./guides/run-a-command.md#start-with-an-observable-machine): run the
  daemon and drive it through its API.
