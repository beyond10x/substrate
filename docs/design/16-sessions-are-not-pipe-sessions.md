# Design 16: a session is not a pipe session

**Status:** accepted as [ADR 0028](../../adr/0028-session-is-the-canonical-route-resource-name.md) · **Date:** 2026-09-01

This design closes the resource name, the breaking migration shape, and the evidence required to
remove the old route. `story:canonical-session-routes` owns the implementation.

## Context

Substrate has one leased session resource with two modes: `pipes` for machine protocols and `pty`
for terminal interaction. Its durable ids are `ses_…`, its operation ids are `session.*`, and its
wire resource kind is `session`. Development bundle `0.14.0` nevertheless puts all eight session
operations beneath `/v1/pipe-sessions`, including the hosted attachment-authority mint route.

The path names one mode rather than the resource and is actively misleading for PTY sessions.
Keeping an alias would carry the wrong public vocabulary into every daemon, SDK, MCP adapter and
example. The operator explicitly chose a clean pre-1.0 break: there is no compatibility route and
no public SDK alias.

This is a coordinated migration because another party may verify the route bytes. Atlas ADR 0022,
at `atlas/architecture/adr/0022-substrate-session-route-is-a-breaking-development-migration.md`,
authorises the exact move and its ordering. Earlier released bundle directories remain immutable.

## Decision

### One resource and one route family

The daemon serves the eight existing `session.*` operations only at these addresses:

| operation | method | successor path |
|---|---|---|
| `session.capabilities` | `GET` | `/v1/sessions` |
| `session.start` | `POST` | `/v1/sessions` |
| `session.get` | `GET` | `/v1/sessions/{session_id}` |
| `session.attach` | `GET` | `/v1/sessions/{session_id}/attach` |
| `session.authority.mint` | `POST` | `/v1/sessions/{session_id}/attachment-authorities` |
| `session.signal` | `POST` | `/v1/sessions/{session_id}/signal` |
| `session.retire` | `DELETE` | `/v1/sessions/{session_id}` |
| `session.lease.renew` | `POST` | `/v1/sessions/{session_id}/lease/renew` |

The corresponding `/v1/pipe-sessions` addresses are not registered. They receive the daemon's
ordinary `route.not-found` response, not a redirect, alias, deprecation response or silent
translation. Durable operation ids and request semantics do not change.

`pipes` remains the correct name for the channel mode and for pipe-channel frames. It is not a
route prefix. Rust resource types, schema authority filenames and internal identifiers do not move
in this route-only change.

### Closed breaking successor

Bundle `0.15.0` directly names `0.14.0` as its predecessor and declares
`breaking-development-v1`. Compatibility is counted by `(method, path)`, not only by operation id:
26 non-session route addresses are preserved, eight old session addresses are removed, and eight
canonical addresses are added.

The compatibility declaration enumerates all eight replacement pairs. The checker is special-cased
to this exact predecessor and successor and proves:

1. the removed and added address sets equal the declaration;
2. every non-session route is byte-equivalent after the normal version substitution;
3. each replacement keeps its operation id, method, scope, idempotency, effects, exposure and
   request/result schemas; and
4. no other route is removed, added, moved or changed.

The default additive rule remains unchanged for every other successor. This one migration does not
turn arbitrary removals into an accepted compatibility class.

### Atomic producer and consumer move

The daemon, SDK requests, contract-derived runtime inventory, MCP adapter, examples and public site
move in one Substrate change. The daemon and SDK advance their advertised bundle name and exact
inner `bundle.json` digest to `substrate-wire/0.15.0` together. A consumer pinned to an earlier
bundle therefore fails contract verification before attempting an old route.

Portable and delegated clean-room lanes each exercise the canonical paths appropriate to their
capabilities. Explicit negative cases prove the legacy collection, member and attachment paths are
not served. Development TCP retains its existing read-only session posture; hosted TLS retains its
scoped authority and exporter-bound proof requirements.

## Consequences

The public vocabulary now matches the actual resource before remote SDK adoption expands it. The
cost is deliberate: existing development clients must update their contract pin and paths for
Substrate `0.5.0`. Earlier bundles remain reproducible reference artifacts, but their session
routes are not served by the new daemon.

This remains a development contract. A verified breaking migration and a signed daemon image do
not make the bundle stable.
