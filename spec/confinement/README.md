# Confinement: one contract, declared adapters

This ESS projection is the confinement contract of atlas ADR 0109 and
[ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md): the
contract families a run may need, the adapters that serve them, the facts each adapter proves when
it opens, and the refusal that answers anything an adapter does not serve. It is written before any
adapter code. Each adapter story adds a test file that runs its lane's scenarios against the
adapter. No adapter story edits the committed suite.

`cargo xtask check-ess` validates it, regenerates `../confinement-suite.json` and compares bytes,
runs that suite against ESS's `interpreted` reference, and regenerates `ContractFamily`,
`AdapterName` and `Fact` into `crates/substrate-host/src/generated/confinement.rs`. The interpreted
run checks the model, not an adapter.

## Model (`confinement.yaml`)

- `ContractFamily`: `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
  `filesystem_view`, `network_egress`, `network_ingress`, `limits`, `audit` (ADR 0109 decision 8).
- `AdapterName`: `local`, `linux`, `macos`, `systemd`, `oci`, `pod`. `systemd` carries an `inner`
  adapter, `local` or `linux` (decision 3).
- `Fact`: what an adapter proves at open and a profile may require. The comment above the type
  defines each fact.
- `OpenAdapter` is `confinement::open`. Its input is the set the adapter proved, so each scenario's
  open step states that adapter's declared set. A runner that holds a real adapter compares the
  adapter's facts with that set. `unserved` is external: the host decides whether an adapter
  opens. The declared set of every adapter, with its source, is the comment above `OpenAdapter`.
- `AdmitProfile` refuses in this order:
  - a set `ingress_routes` or `rate_bounds` (`confinement.reserved-field-unserved`, 501);
  - an adapter that did not open (`confinement.adapter-unserved`, 501);
  - on an adapter that proves `declared_insecure`, a missing or empty reason
    (`confinement.local-reason-missing`, 422);
  - on an adapter that proves `image_digest_pinned`, an image that names no `sha256` digest
    (`confinement.image-unpinned`, 422);
  - on an adapter that proves `declared_insecure` but not `local_permitted`, any request
    (`confinement.local-refused`, 403);
  - the first required family the adapter does not serve, in decision 8's order
    (`confinement.family-unserved`, 501);
  - the first required fact it does not prove, in the `Fact` order
    (`confinement.fact-unserved`, 501).

  Otherwise it admits the profile, `declared_insecure: true` with the reason on a `local` adapter
  and `false` elsewhere.

A run is `local` exactly when its opened adapter proves `declared_insecure`: `local` itself, or
`systemd` opened around `local`. The model reads this from the opened adapter row, not from the
request's `adapter` and `inner`. Under ESS 0.70.0, synthesis evaluated a guard comparing the
`adapter` input with an enum literal differently from the interpreter, and three synthesized
scenarios failed against `interpreted`. Keying on the row's facts removed the disagreement.

## Scenarios and lanes

An ESS scenario has no tag, so the lane is the name's suffix `-on-<lane>`, and the summary begins
with `lane <lane>:`. The name begins with the adapter: `local`, `linux`, `macos-s1`, `macos-s2`,
`systemd-local`, `systemd-linux`, `oci` or `pod`. Each runner selects its lane by that suffix. An
`adapter-unserved` scenario runs in the lane where its adapter is absent: `portable` for `macos`,
`systemd`, `oci` and `pod`, and `macos` for `linux`.

| lane | scenarios |
|---|---|
| `portable` | 13: nine on `local` (admission, a missing reason, host policy, a filesystem view, three facts, both reserved fields), and the four `adapter-unserved` scenarios with their setup in the summary |
| `delegated` | 10: six on `linux`, four on `systemd` (two around `local`, two around `linux`) |
| `macos` | 8: `linux` absent, four `macos-s1-*` (S1's adapter), three `macos-s2-*` (the admissions S2 makes pass) |
| `oci` | 3: admission by digest, egress refused, a tag refused `image-unpinned` |
| `pod` | 2: admission by digest, a filesystem view refused |

Synthesis writes 22 scenarios and refuses 10 obligations. Each refused obligation has an authored
scenario among the 36 above:

- `adapter-unserved` (`ESS-SYNTH-001`): the identity is a six-value enum, so synthesis cannot name
  an adapter that no other scenario opens;
- `local-refused` (`ESS-SYNTH-003` and `ESS-SYNTH-001`), `admitted-insecure`, and
  `fact-unserved` for `ipc_signals_contained`, `sealed_paths`, `exec_subpaths`, `private_tmpdir`,
  `host_network` and `image_digest_pinned` (`ESS-SYNTH-003`): the bounded arrangement reaches no
  adapter row that holds the needed facts.

All 58 scenarios pass against ESS 0.70.0's `interpreted` target.

## Open questions (`UNMAPPED:` in `confinement.yaml`)

- Whether `systemd` without `inner`, `inner` beside another adapter, or an `inner` other than the
  open `systemd`'s is a refusal. The model decides by the opened row and records `inner` as sent.
- The audit record of a refusal (`ProfileRefused`). ESS refuses an event on an outcome that reports
  an error, so the adapters' own tests hold it.
- What an image beside `local`, `linux`, `macos` or `systemd` means. The model ignores it.
- The shape of an ingress route and a rate bound. Any value is refused.
- An `oci` or `pod` profile with no image is refused as unpinned. This fails closed, and no source
  says so.
- The facts no story declares: `exec_subpaths` and `private_tmpdir` on `linux`; whole-tree kill,
  IPC, `private_tmpdir` and `paths_are_host_paths` on `oci`; whole-tree kill on `pod`; the `oci`
  story's rootless fact, which has no `Fact` variant; and the audit snapshot fact of decision 2.
