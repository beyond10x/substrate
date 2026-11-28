---
format: aep.planning-md/3
id: story:root-filesystem-from-a-digest-named-archive
kind: story
status: draft
title: A root filesystem is unpacked from a digest-named archive without a container daemon
summary: 'Consumer gap 5: a gzip tar named by sha256 becomes a read-only root filesystem for an exec or workload; a digest mismatch is a named refusal.'
owner: substrate
relations:
- decomposes: epic:long-lived-service-on-the-host-driver
revision: 2
---
# Story: a root filesystem is unpacked from a digest-named archive without a container daemon

## Where the requirement comes from

Consumer gap 5 of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit
`2ed72c2`): "Images from an artifact without a Docker daemon (N6): a root filesystem from a
content-addressed archive, digest observed." First input: a gzip tar of a root filesystem named by
its sha256 digest; an OCI image comes second. The stakeholder is the session that maintains `weft`.

## Why

- The image family is designed in `docs/design/01-contract.md` § 3.4 (pull, build, push) and is
  absent from the wire; images there assume a registry and a Docker daemon.
- A confined exec today sees the host's `/usr` read-only as its base view; nothing lets a run see a
  root filesystem of its own.

## What changes

1. An ADR first (invariant 8), after the operator's answer to decision D2 (who unpacks). It fixes
   the unpack rules: every entry written with `openat2` beneath the store root; no device node,
   FIFO or socket entry; setuid and setgid bits dropped; owners not kept; a hard link only to an
   entry already in the archive; a symbolic link kept as a link and never followed while
   unpacking; bounds on entry count, total bytes and path length.
2. A successor bundle: an image (root filesystem) resource the caller creates by naming a sha256
   digest and supplying the bytes over the daemon socket, with the observed digest in its record,
   a capability fact, and named refusals (digest mismatch, bound exceeded, unsupported entry).
3. Unpacked trees live in an owner-private content-addressed store, read-only after unpacking,
   removed by a delete that is refused while a run uses the tree.
4. The host driver mounts the tree read-only as `/` of an exec or workload in place of the host
   base view; `/workspace`, `/runtime` and volumes are mounted as today.

## Acceptance

A weft test (`weft` story `ports-node-images-local`) can check:

- Create an image from a gzip tar with its correct sha256: the record states that digest as
  observed; an exec started on it reads `/etc/os-release` from the archive, not the host's.
- The same bytes under another digest are refused with a named refusal that states the observed
  digest; nothing is left in the store, and a workload naming that image never starts.
- An archive holding a `../` path, an absolute path, a device node, or a link pointing outside its
  root is refused by name (or, for a link, stored and never followed); no file outside the store
  root is written (checked on the host).
- A setuid binary in the archive is stored without the setuid bit.
- Delete of an image used by a running workload is refused `conflict`; after the run ends it
  succeeds and the tree is gone.
- Portable lane: without the mechanism the fact is absent and a create is refused by name.

## Blocked on / Depends on

- Decision D2 (operator), then the ADR.
