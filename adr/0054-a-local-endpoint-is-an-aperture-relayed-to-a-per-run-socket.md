---
status: accepted
date: 2026-11-28
---

# ADR 0054: a local endpoint is an aperture relayed to a per-run socket

Decider: the stakeholder of the work, the session that maintains the `weft` repository,
2026-11-28 (`approval-record:weft-approves-long-lived-service-stories-2026-11-28`): "I also
approve the design direction for gap 3 (a listener in the run's netns, relayed to the agent's
per-run socket)". On the same day the stakeholder decided that the socket's name is a key the
caller chooses, with the run id as the fallback. A design change within
[ADR 0013](0013-egress-apertures-are-declared-by-the-operator.md), decided by the repository's
session. Decides `story:local-endpoint-attributed-to-its-run`, consumer gap 3 of the `weft`
repository. The design is [design 38](../docs/design/38-long-lived-service-on-the-host-driver.md)
§ 6; the design comes before code (invariant 8).

## Context

An egress aperture (ADR 0013) is the one way a confined run reaches anything outside its network
namespace: a listener on `127.0.0.n` inside the namespace, relayed by a forwarder in the host
namespace to one operator-declared TCP destination. Every run that names an aperture reaches the
same destination, so the destination cannot tell two runs apart.

The consumer's node agent must know which run a connection came from: "a socket the node agent
serves inside each exec's network namespace, from which the agent learns which instance called".
The forwarder already holds that knowledge. It is forked per run, and it never leaves the host
namespaces, so it can reach a host Unix socket that the run cannot see.

An instance's first call often comes while it starts. When the socket is named by an id
substrate mints, the agent learns the name only from the start's answer, and a connection the
run makes before the agent created the socket is closed. The caller knows its instance's id
before the start, so a name the caller chooses lets its agent create the socket first.

## Decision

1. **A local endpoint is an operator-declared aperture whose destination is a directory of Unix
   sockets.** It is declared with `--local-endpoint <name>=<port>:<directory>` and selected by
   name through `sandbox.aperture` or `sandbox.apertures`, never with a destination. It shares
   the name space of `--egress-aperture` (one name declared by both is refused at startup), the
   run bound `MAX_EGRESS_APERTURES` (8), the deployment bound `MAX_DEPLOYMENT_EGRESS_APERTURES`
   (12, counting both kinds) and the listen addresses: the n-th selected aperture listens on
   `127.0.0.n:<port>`, whatever its kind. A run that names none has no listener for it.

2. **The directory is opened once, at startup, and held by its descriptor.** The daemon opens it
   with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` and refuses to start,
   `config.local-endpoint-invalid`, unless it is a directory owned by the daemon's effective uid
   with no group or other write permission. The daemon creates, replaces and removes nothing in
   it: the agent, which the operator declared beside it, owns the sockets. The directory is one of
   the daemon's private host paths, so a read-only root, a read-only file or a writable root that
   meets it is refused as one that meets the state directory is. Startup also refuses, with the
   same code, a directory that meets a path the daemon owns, compared as written and as resolved
   in both directions: the workspace root, because every directory at or beneath it is one a run
   may hold read-write (a workspace, or a directory `workspace.adopt` serves); the state
   directory; and every other path the daemon trusts, the API socket among them, so no key names
   a socket of the daemon's own. Two endpoints over one directory, told apart by the device and
   inode their opened descriptors report, are refused, because decision 6 holds a key per
   endpoint and one directory would give two runs one socket.

3. **The socket's name is the start's `sandbox.local_endpoint_key`, else the run id.** The key is
   optional on the sandbox of an exec start and of a session start's exec, beside `aperture` and
   `apertures`, because it qualifies what they select: 1 to `MAX_LOCAL_ENDPOINT_KEY_BYTES` (64)
   bytes matching `^[a-z0-9][a-z0-9-]*$`, so it holds no `.` and no `/` and names an entry of the
   directory and nothing beneath or above it. Anything else is `request.schema-invalid` (422)
   at `sandbox.local_endpoint_key`. Without a key the name is the id substrate minted for the run:
   the exec id; for a session, the id of the exec it runs, which the session start's answer
   names, since the host driver knows a session by that id; and a workload's id when workloads
   are served. Every such id holds `_`, which no key holds, so a key
   never names the socket of another run's id. The key is part of the request hash like every
   start member; a start without it serializes and hashes as on `0.29.0`.

4. **The relay connects through the held descriptor, from the host side only.** For each
   connection the forwarder accepts, a relay process forked from it opens the entry `<name>`
   beneath the directory's descriptor `<n>`, which the fork inherited, with
   `openat(<n>, <name>, O_PATH | O_NOFOLLOW | O_CLOEXEC)`; requires with `fstat` that the opened
   file is a socket; and connects through `/proc/self/fd/<m>`, where `<m>` is that descriptor, so
   the connect reaches the opened socket and looks up no name again. A symbolic link is opened as
   the link and is not a socket, so an entry that is a link reaches nothing, wherever it points;
   so does a regular file, a directory or an absent entry. The name is fixed before the fork, and
   the relay allocates nothing. The relay runs in the daemon's mount, network and user
   namespaces, so nothing a run controls takes part in the lookup, and no path given to
   bubblewrap names the directory. The design's open question I10 (whether a `connect` from the
   daemon reaches the agent's socket through the held descriptor, and whether such a `sun_path`
   fits) is answered: the startup probe below connects that way on every host that publishes the
   fact, and unit cases of the host driver hold it. `/proc/self/fd/` with a descriptor number
   takes at most 24 of the 108 bytes of `sun_path`, whatever the name.

5. **An absent socket closes the connection.** When the connect fails, for an absent socket or any
   other reason, the relay closes the accepted connection and tries nothing else: no other socket,
   no retry, no fallback. A connection made before the agent created the socket is closed. The
   agent learns a run id from the start's answer; with a key it creates the socket before the
   start.

6. **One live run holds a key on a local endpoint.** A start that names a key another live run of
   this daemon holds on the same local endpoint is refused `exec.local-endpoint-key-in-use` (409,
   class `conflict`), so a socket is never shared by two runs. The host driver answers it, because
   it alone holds the live set: from the admission of the start, before anything is spawned, to the
   end of the run, when its apertures are torn down and the key is free again. A refusal of the
   driver is recorded as every driver refusal is: the accepted operation's row is `terminal` with
   the refusal as its error (design 37 D23). The keys of an endpoint are one name space across
   subjects, because the socket is one host object: a key any live run holds is refused whatever
   the subject of the start, so a deployment with several `--allow-uid` subjects shares the key
   space of each endpoint. The refusal's fixed message names neither the other subject nor its
   run.

7. **The fact needs the mechanism.** A local endpoint is listed in `exec.egress-apertures` with
   `destination: "local:<port>"`, and only after the mechanism ran at startup in a throwaway
   sandbox: a forwarder relayed to a socket in a private directory through that directory's
   descriptor and reached from inside the sandbox. An unproved mechanism leaves the element out.
   A client reads the port from the destination, as it reads a TCP aperture's. No host path is
   published.

8. **The applied aperture states the relay and no host path.** Its `mechanism` is
   `local-endpoint-forwarder` (the schema's single value `loopback-forwarder` becomes an
   enumeration of both), its `destination` is `local:<port>`, `listen` is present on the single
   form too, and `key` names the socket the relay connects to: the start's key, or the run id.
   `bytes` is counted in the relay as for a TCP aperture. `max_bytes` is not declarable for a local
   endpoint.

9. **Three refusals, each published with a vector.**

   | code | status | class | when |
   |---|---|---|---|
   | `exec.local-endpoints-unserved` | 501 | `unserved` | a start names a declared local endpoint whose mechanism is not proved; before dispatch, at `sandbox.network.aperture` or `sandbox.apertures` |
   | `exec.local-endpoint-key-without-endpoint` | 422 | `refused` | a start names a key and selects no local endpoint; before dispatch, at `sandbox.local_endpoint_key` |
   | `exec.local-endpoint-key-in-use` | 409 | `conflict` | decision 6, at `sandbox.local_endpoint_key` |

   The daemon answers the first two before dispatch; it reads the declared names from its own
   configuration, because an unproved endpoint is absent from the fact. The host driver answers
   all three, so a caller that reaches it without the daemon hears the same.

10. **Successor bundle `0.30.0`.** This is the first slice of design 38 to land, so it publishes
    bundle `0.30.0`, not the `0.33.0` design 38 § 2 proposed. It adds no route. Its compatibility
    block names `0.29.0` as its predecessor. A client pinned to `0.29.0` cannot select a local
    endpoint and so never reads the new values.

## Consequences

The forwarder gains a second kind of destination, a Unix socket in an operator-declared
directory, and the relay connects there through a descriptor rather than to a pinned address.
Everything else of ADR 0013 holds: the default is no network, a name is the only thing a request
may say, configuration owns reach, the mechanism is probed, and what was applied is observed.

The agent attributes a connection by the socket it arrived on and trusts nothing the run says.
That holds while the directory is the agent's alone: a second writer of the directory could
create a socket under another run's name. The daemon refuses a directory others can write and
one a run may hold, the relay reaches a socket entry and never what a link names, and the
operator owns the agent.

A key held by a live run is refused rather than shared, so two runs of one caller with one key
cannot be told apart by mistake; it costs the caller a retry when it restarts an instance before
the first run ended.

A hosted runner proves the refusals and the schema; the relay itself needs bubblewrap and a
delegated cgroup, so the delegated lane proves two runs reaching only their own sockets, the
closed connection of an absent socket, and the applied record.
