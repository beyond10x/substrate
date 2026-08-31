# Design 13: a PTY is a second session mode, not a second session resource

**Status:** accepted as [ADR 0019](../../adr/0019-pty-is-a-second-session-mode.md) · **Date:** 2026-08-30

This document precedes the ADR that `story:pty-sessions` names as its first evidence item. Accepted
by the operator on 2026-08-31 as ADR 0019. It fixes
which enum grows, whether a terminal adds a route family, the frame set, the resize bounds and what
the child observes, where the capability fact lives, and the named refusal when the host cannot give
a terminal. It claims **no ADR number**: `adr/` admits `accepted` and `superseded` only
(`xtask/src/adrs.rs:12`), so the number is assigned by the operator at acceptance, exactly as
[design 12](12-aperture-byte-ceiling.md) waited for one.

## Context

[Design 05](05-streams-sessions-and-endpoints.md) § 2 already decided the hard part: an interactive
exec has an explicit mode; `pty` carries input, output, resize, signal, exit and protocol-error
frames; **a PTY is never substituted for pipes**; and both share finite frame, byte, queue, idle,
lease and attachment bounds. [ADR 0007](../../adr/0007-protocol-processes-use-raw-pipe-sessions.md)
served `pipes` first and named PTY as absent, and
[ADR 0008](../../adr/0008-pipe-sessions-have-distinct-durable-identity.md) built the durable session
around a `mode` field. `README.md:47` still lists PTY as **absent** and
`docs/plan/04-direct-byte-plane.md:86` still lists it as remaining.

What is unresolved is smaller than it looks, and it is two questions the tree answers ambiguously.
The wire has *both* axes: `SessionKind` holds one variant `Session`
(`crates/substrate-wire/src/lib.rs:1220-1223`) and `SessionMode` holds one variant `Pipes`
(`:1227-1229`). And the routes are literally
`/v1/pipe-sessions/{session_id}/…` (`crates/substrate-daemon/src/app/routes.rs:68-85`), so a
terminal either arrives under a name that says `pipe` or under a new one.

## Decision

**`SessionMode` grows; `SessionKind` does not.** `SessionKind` is the *resource* axis — one variant
serialised `"session"`, the same shape as `ExecKind::Exec`
(`crates/substrate-wire/src/lib.rs:1196-1199`), and the field that tells a client which kind of
resource an absence describes (`SessionAbsence`, `:1282-1287`). `SessionMode` is the *channel*
axis, already carried as `mode` on the durable resource (`:1268`), already named "mode" by design 05
§ 2 and by ADR 0007's *"two explicit session modes"* (`adr/0007-protocol-processes-use-raw-pipe-sessions.md:22`),
and already recorded per session by ADR 0008. Growing `SessionKind` instead would make a terminal a
different *kind of resource* from a pipe session, which is precisely the split ADR 0008 refused: one
`ses_…`, bound one-to-one to one `ex_…`, with one lifecycle and one lease clock. The story's word
"kind" is English; the enum is `SessionMode`.

**One route family, unrenamed. `adds_routes: 0`.** The bundle's operation ids are already
mode-neutral — `session.capabilities`, `session.start`, `session.get`, `session.attach`,
`session.signal`, `session.retire`, `session.lease.renew`, all seven on `/v1/pipe-sessions*`
(`contracts/substrate-wire/0.8.0/operations.json`) — so a terminal needs no new operation, only a
mode. `PipeSessionStartInput` (`crates/substrate-wire/src/lib.rs:1212-1217`) gains
`mode`, defaulting to `pipes`, and a `window`. The default is what enforces design 05's rule
mechanically: an omitted field can only ever mean pipes, so no existing client can be handed a
terminal, and the type is `deny_unknown_fields` (`:1211`), so a client asking a `0.4.0` daemon
for `mode: "pty"` is refused schema-invalid rather than quietly given pipes.

*What a `/v1/pty-sessions/*` family would have cost.* Seven duplicated operation ids for a lifecycle
that is byte-identical after start and attach — two names in the durable operation ledger for
"retire a session", two capability documents, and a `GET /v1/pty-sessions/{id}` that must answer for
a `ses_…` of the other mode without answering `absent: true` (`SessionAbsence`, `:1282-1287`), which
would be a false observation about a session that exists. *What renaming to `/v1/sessions/*` would
have cost.* A wire-visible path another party verifies, so a coordinated migration with an ADR in
atlas (`AGENTS.md:3-6`, `:92-98`) — and not expressible as a successor anyway: the compatibility
kind is `additive-v1` (`contracts/substrate-wire/0.8.0/bundle.json:5-10`) and a rename removes seven
routes rather than adding any. *The price paid instead* is a path literal that names the family's
first mode rather than its only one. It is a stable identifier, not a description.

**Six frames, and the two that are missing are the point.** Client to server: `input`, `resize`,
`signal`. Server to client: `output`, `exit`, `protocol-error` — design 05 § 2's list exactly. There
is no `close-input` because a pty has no half-close: a client ends input by sending the terminal's
own EOF character as ordinary input bytes, which is line-discipline behaviour and not a frame. There
is no per-frame `stream` discriminator (`PipeServerFrame::Output` carries one, `:1323-1327`) because
stdout and stderr **are the same descriptor** on a pty; that, plus the line discipline rewriting the
bytes it carries, is the mechanical reason ADR 0007 forbids running a machine protocol on one. The
durable capture records the merged stream as `stdout`, so
`GET /v1/execs/{exec_id}/output?stream=stderr` returns an empty slice — reported, not inferred:
stderr genuinely was the same file.

**Reaching the output bound ends a pty session; it does not truncate it.** For `pipes`, truncation
is a statement about the durable record delivered at terminal time (`stdout_truncated`, read at
`crates/substrate-daemon/src/app/sessions.rs:1123`, `:1135`), and design 05 gave the pty no
`truncated` frame to deliver it on. A terminal stream also has no per-stream offset to resume from,
so a client whose transcript silently stops has no way to rejoin it. The session therefore ends at
the bound the client itself declared (`ExecLimits::output_bytes`,
`crates/substrate-wire/src/lib.rs:865`, capped by host configuration at
`crates/substrate-host/src/process.rs:547-549`), reported through the field ADR 0014 added to the
exec observation — class `exhausted`, code `session.output-limit` — beside a state that is already
`Cancelled`. No new frame, no new event kind, and the `exit` frame the client already reads carries
the state.

**Resize is bounded at 1–1000 columns and 1–1000 rows, in cells only.** The kernel field is an
`unsigned short`, so 65535 is deliverable, but a 65535×65535 window is not a display — it is an
amplification knob, because programs allocate per-cell buffers when the size changes and that
allocation is spent from the run's memory bound. 1000×1000 is above any real terminal (a 4K display
at a small font is roughly 400×100 cells). Zero is refused rather than mapped to a default: a zero
dimension is how a terminal says *I do not know*, which is not what a client that sent a resize
meant. Pixel dimensions are not on the wire and are set to zero. An out-of-bounds resize is a
`protocol-error` frame, code `session.resize-invalid`, joining the `session.frame-invalid` /
`session.sequence-invalid` / `session.signal-invalid` vocabulary already at
`crates/substrate-daemon/src/app/sessions.rs:890`, `:930`, `:988`. Resize frames are rated against
the control window that exists (`max_controls_per_window` / `control_window`, `:51-52`), so a resize
storm cannot become a free ioctl loop.

**The initial window is required, and the child is told nothing else.** `mode: "pty"` without a
window is refused rather than defaulted to 80×24: substrate has nothing to observe here, the client
does, and inventing the number is manufacturing a fact. Thereafter the child observes a resize the
way any process does — the daemon sets the size on the master, the kernel signals the foreground
process group, and the child reads it back with `TIOCGWINSZ`, the check the story's own acceptance
names. No `COLUMNS`, `LINES` or `TERM` is injected: the environment is `--clearenv` plus exactly the
declared names (`crates/substrate-host/src/process.rs:1115`, `:1167-1172`), a size in the
environment goes stale at the first resize, and a `TERM` substrate chose would be a claim about a
terminal substrate does not render. A client that wants one declares it through
`ExecStartInput.env.set` (`crates/substrate-wire/src/lib.rs:921`) like any other name.

**`--new-session` stays, and the controlling terminal is acquired after it.** Inheriting the slave
as descriptors 0, 1 and 2 is not enough for a terminal that works. The shared confinement path
passes `--new-session` (`crates/substrate-host/src/process.rs:1113`), and `bwrap(1)` states that the
flag "calls `setsid()`" and "disconnects the sandbox from the controlling terminal", which is the
mitigation it names for CVE-2017-5226. Without a controlling terminal there is no foreground process
group, so no job control and no hangup on the master closing — and hangup is what the story's
acceptance requires. The terminal is therefore made controlling **inside** the sandbox, after
bubblewrap's `setsid`, at the interposition point the command line already has
(`crates/substrate-host/src/process.rs:1190` already interposes `/usr/bin/env`). What must **not**
happen is dropping `--new-session` to get the same effect: that would weaken the confinement floor
of every non-pty exec to serve one feature, which is the silent degradation invariant 3
(`AGENTS.md:36-37`) forbids. Which interposition is used is the implementing story's failing test,
not this decision; that it happens after `setsid` and that no fact is published until a probe proved
it end to end, is.

That is safe here only because **the master never crosses the boundary**: the child inherits the
slave and nothing else, the pty is allocated per session, and no descriptor of anybody else's
terminal is ever passed in — so the input queue a child could push characters into is its own. A pty
also gives that child a second, in-band signal path, because the line discipline turns the interrupt
character into a signal for the foreground process group. That is inside the run's cgroup, changes
no bound and leaves whole-tree kill unaffected; the `signal` frame stays the only path substrate
observes and records.

**`sessions.pty` is a driver fact, and the mode gate is in the capability document.**
`CapabilityFacts` has no `sessions.*` member (`crates/substrate-wire/src/lib.rs:1387-1496`); it gains
one, `sessions.pty`, published by the host probe (`crates/substrate-host/src/probe.rs:44`) only after
that probe allocated a pair, made it controlling in a throwaway sandbox and round-tripped a size —
the pattern `secrets.slots` set (`crates/substrate-wire/src/lib.rs:1489-1490`,
`crates/substrate-host/src/probe.rs:62-65`). It belongs in the facts because the capability snapshot
is what a client pins and what is bound to backend identity, so a host that loses the ability moves
the digest. The **cost of one route family** is that the registry's own gate cannot express it:
`capability_predicates` are per operation (`contracts/substrate-wire/0.8.0/operations.json`), and
hanging `sessions.pty` on `POST /v1/pipe-sessions` would take the whole route away from a daemon
that serves pipes perfectly well. So the per-mode gate is served where session capability already is
— `GET /v1/pipe-sessions`, which already refuses `unserved` / `session.confinement-unavailable` when
the confinement floor is missing (`crates/substrate-daemon/src/app/sessions.rs:159-170`).
`PipeSessionCapabilities` (`crates/substrate-wire/src/lib.rs:1291-1301`) gains `modes` and the two
window ceilings, derived from the fact and never a second source of truth.

**Two refusals, and neither degrades to pipes.**

| when | class | code | address | status |
|---|---|---|---|---|
| `sessions.pty` absent and `mode: "pty"` asked for | `unserved` | `session.pty-unserved` | `mode` | 501 |
| the host cannot allocate a pty at start | `exhausted` | `session.pty-exhausted` | `session` | 429 |
| `window` sent with `mode: "pipes"` | `refused` | `session.window-invalid` | `window` | 422 |
| `mode: "pty"` sent with no `window` | `refused` | `session.window-invalid` | `window` | 422 |

The first is refused before dispatch and the second is a `NotDispatched` driver error, the shape
`start_pipe` already uses for `session.limit-unserved`
(`crates/substrate-host/src/process.rs:280-284`); `exhausted` maps to 429 and `unserved` to 501
(`crates/substrate-daemon/src/app/operations.rs:1254-1255`). Allocation failure is `exhausted` and
retriable because the host's pty count is a global resource other tenants can fill and free. **In no
case is a pipe session started instead**, and in no case is a `pipes` request served on a terminal.

**Successor bundle `0.9.0`, provisionally**: predecessor `0.8.0`, `adds_routes: 0`,
`preserves_routes: 26`. [Design 14](14-network-session-authority.md) also names `0.9.0`; the
number belongs to whichever is accepted first, and the other moves to its successor. Two
designs landing together share a bundle and say so
(`contracts/substrate-wire/0.8.0/bundle.json:5-10`, whose 26 operations are the routes), authored
under `xtask/bundle-source/0.9.0/`, with `cargo xtask check-bundle 0.9.0` added to `scripts/gate.sh`
beside the four that are there (`scripts/gate.sh:27-30`) — a bundle whose check is not in the gate is
unverified from the next commit onward. It adds `schemas/pty-channel-frame.json` beside
`schemas/pipe-channel-frame.json` (`contracts/substrate-wire/0.8.0/schemas/`). Earlier directories
keep their bytes (invariant 6, `AGENTS.md:43-48`). The resize ceilings are bound from
`xtask/src/bundle.rs` and **never** from `xtask/src/render.rs`, whose sha256 every rendered
`bundle.json` carries (`AGENTS.md:203-209`).

**The dependency change is two lines.** `crates/substrate-host/Cargo.toml` depends on no `nix` at
all today (`:10-25`) and gains `nix.workspace = true`; the workspace entry
`nix = { version = "0.30", features = ["fs", "user"] }` (`Cargo.toml:37`) gains `"term"`, because
`nix::pty` is behind that feature (`nix-0.30.1/src/lib.rs:168-171`) and `openpty` lives in it
(`nix-0.30.1/src/pty.rs:256`). Cargo features are additive, so `"term"` also compiles for
`substrate-daemon`, the only other `nix` consumer today
(`crates/substrate-daemon/Cargo.toml:30`).

## Consequences

A human gets a terminal on a confined process with the lease, single attachment, bounded channel and
whole-tree cleanup a pipe session already has, and gets it without a second resource, a second
lifecycle or a second operation vocabulary. `README.md` § *Status*, `STATUS.md` and
`docs/plan/04-direct-byte-plane.md` § *Later phase-4 slices* stop listing PTY as absent in the same
change.

The costs are stated rather than hidden. The route family keeps a name that describes its first mode
— renaming it later is a coordinated migration with an ADR in atlas, not a successor bundle. A pty
session ends when its declared output bound fills, where a pipe session survives and reports
truncation, so an interactive client declares a bound it can live with. And a terminal that works
needs a controlling terminal acquired inside the sandbox after bubblewrap's `setsid`; if that cannot
be proved on a host, `sessions.pty` is absent and every terminal request is refused by name rather
than served as something quieter.

The positive half is unprovable on a hosted runner, exactly as for ADR 0013 and ADR 0014: echoing
bytes through a real terminal, applying a resize the child reads back, and exiting on hangup need
the delegated lane on a self-hosted runner (`bash scripts/delegated-lane.sh`). CI proves the typed
`unserved`, the request-side refusals, the frame schema and that every frozen bundle byte is
unchanged, and reports the rest **absent rather than passed** (invariant 3).
