---
format: aep.planning-md/3
id: decision-blocker:execution-profile-noatime-remount
kind: decision-blocker
status: open
title: The execution profile refuses bubblewrap's read-only remount of a noatime bind
relations:
- blocks: story:chart-lane-local-execution-record
withholds: test_result
revision: 1
---
# The execution profile refuses bubblewrap's read-only remount of a noatime bind

Decider: operator

## Finding

A held chart-lane run at `d21204a4` (kind, node profiles enforced) publishes no `exec.*` fact: the
confinement probe's bubblewrap sandbox fails, so every exec is refused `exec.sandbox-unavailable`
(501, `exec.namespaces`). The kernel's denial for the lane's own profile:

```
apparmor="DENIED" operation="mount" info="failed flags match" name="/newroot/runtime/sentinel/"
comm="bwrap" flags="ro, nosuid, nodev, remount, noatime, bind, silent"
```

bubblewrap keeps the source mount's `noatime` when it remounts a bind read-only. Inside the pod,
`/tmp` (emptyDir) and `/var/lib/b10x-substrate` (the PVC) are `rw,noatime` because the node's root
filesystem is. The profile's rule (`deploy/apparmor/substrate-host-execution:33`) does not list it:

```
remount options in (rw,ro,bind,nosuid,nodev,noexec,relatime,silent) /**,
```

One-variable probe, same container, daemon credentials (uid 65532, no capabilities,
`no_new_privs`, profile enforced, seccomp mode 2): binding from the two `relatime` mounts
(`/run/substrate/tls`, `/run/substrate/identity`) starts the sandbox; binding from the two
`noatime` mounts is denied as above. Ruled out in the same run: the cgroup check, user namespace
creation and the seccomp profile.

Not verified: whether production nodes mount `noatime`; whether each exec's workspace bind from
the PVC is denied the same way (expected, not run).

## Options

| | what it does | cost |
|---|---|---|
| A | A successor profile `substrate-host-exec-v2` adds `noatime,nodiratime` to the remount set; `v1` bytes stay as shipped since `0.10.0`; installer and bootstrap accept `v2`; the chart defaults to it. | A profile version, an installer and bootstrap change, nodes load `v2` on upgrade. |
| B | Edit `v1` in place. | Nodes holding `v1` from `0.10.0`–`0.14.0` disagree with the installer's exact-bytes check on upgrade. |
| C | No profile change; the chart requires `relatime` or `strictatime` mounts for `/tmp` and the state volume. | Depends on the node and storage class; a kind node inherits the host's mount options, so the lane still cannot run here. |

Recommendation: A. `noatime` only stops access-time writes; it grants no path, capability or
device, and A keeps every shipped profile's bytes.

## What clears it

A chosen option, and for A the profile change merged. Until then PR 190 stays a draft.
