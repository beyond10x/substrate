# Issue 112: unrecorded terminal streaming evidence

Observed locally on 2026-10-02 for the implementation of
[Substrate #112](https://github.com/beyond10x/substrate/issues/112).
The design is [terminal capture selection](../../docs/design/23-terminal-capture-selection.md).
This records implementation evidence, not release or downstream Mantle acceptance.

Release candidate `d43654350fb3a0eb817e527a6460f162f167362b` (0.7.9) also passed the
complete delegated lane on 2026-10-02 at 18:03:21 UTC. This is candidate evidence;
the release tag still requires a run against its exact merged main commit.

## Production paths

`scripts/delegated-lane.sh` passed the existing host confinement, SDK, remote WSS,
MCP, hosted attachment authority and runtime-vector journeys. Its unrecorded journey
was subsequently expanded and rerun against the shipped daemon, real host driver and
SQLite store in a fresh delegated systemd scope.

The final `b10x-substrate-sdk --test unrecorded` run passed both Rust tests:

- Twenty unrecorded canary sessions: pipes and PTY, each covering successful stdout,
  stderr, child failure, cancellation, lease expiry, output limit, disconnected reader,
  blocked reader, injected SQLite update failure, and daemon restart/recovery.
- Two policy refusals before execution reservation and two recording-enabled positive
  controls, using the identical database/WAL/log scan.
- Sixty-six additional successful unrecorded attachments, 33 per mode, exceeding the
  published global capacity sequentially and proving permit recovery after termination.
- A portable unsupported-driver refusal with deployment policy enabled.

Synthetic canaries enter through stdin and are observed in live output. The scan checks
raw bytes and base64-aligned fragments in the live database, live WAL and captured daemon
stdout/stderr before shutdown. Operation and event payloads live in that same database.
Persistence-error and restart cases also scan before their recovery step. Recording
controls are detected. Reattachment is refused by name; output queries distinguish
intentional non-recording from an empty transcript.

The production journey checks exact pipe stdout/stderr counts, queue ceilings, named
output/backpressure bounds, bounded termination, exit codes, cancellation signal,
authoritative expired lease, and removal of the applied execution cgroup. The host drain
test independently checks both streams: ten raw bytes observed, eight queued, exactly
two queued frames at high water, cancellation at the ceiling and zero captured bytes.
The store test attempts both payload insertion and capture-mode downgrade, verifies
rejection before SQLite/WAL writes, then proves the same scanner detects recording.

## Verification scope

`task check` passed. The repository gate covers the ESS projections and generated capture type, public docs,
workspace tests, formatting, Clippy, secrets, advisories, licenses, dependencies, immutable
bundles, JSON classification and toolchain. Earlier bundles 0.1.0 through 0.16.0 are
unchanged. The new development bundle is 0.17.0; it is not a stable contract release.

Resource measurement assertions execute when the host advertises the required counters.
This workstation's delegated user scope has no `io.stat`, including with IO accounting
requested, so that live measurement lane is explicitly absent here. Confinement CPU,
memory and process bounds still run. Before acceptance, run the new journey on a host
with the complete resource-counter capability to verify measurements in unrecorded mode.
The existing seven real project-quota cases also remain absent without their filesystem
fixture; this change does not alter quota implementation.

The ESS capture projection gives selection a typed home and checks generated-code drift.
Native production tests prove the transport/storage properties; an ESS reference
interpreter is not presented as proof of privacy. Restart counters are last persisted
observations. Queue counts are host-channel acceptance, not acknowledged client delivery.
The scan does not claim control over child/client files, host swap, core dumps or administrators.

Mantle must independently pin this SDK revision and validate its launcher replay,
last-output and authentication-client diagnostic policy. No Mantle source is changed here.
