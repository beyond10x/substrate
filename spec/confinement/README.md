# Confinement: one contract, declared adapters

This ESS projection is the confinement contract of atlas ADR 0109 and
[ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md): the
contract families a run may need, the adapters that serve them, the facts each adapter proves when
it opens, and the refusal that answers anything an adapter does not serve. It is written before any
adapter code. Each adapter story adds a test file that runs its lane's scenarios against the
adapter. No adapter story edits the committed suite. A story that adds a fact and proves it on
the adapters ([design 32](../../docs/design/32-adapter-stdin-and-live-output.md) § 6) meets this by
role: a spec author writes the suite change as the story's first commit, and the adapter
implementor edits nothing under `spec/confinement/`, the generated suite or the pinned counts.

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
  adapter's facts with that set. The declared set of every adapter, with its source, is the
  comment above `OpenAdapter`. It refuses, before anything opens:
  - `local`, or `systemd` around `local`, whose set lacks `declared_insecure`
    (`confinement.fact-unserved` naming `declared_insecure`, 501): ADR 0044 decision 3 and atlas
    ADR 0109 decision 4 make `local` declared insecure always;
  - `oci` or `pod` whose set lacks `image_digest_pinned` (`confinement.fact-unserved` naming
    `image_digest_pinned`, 501): both run an image by digest only.

  `unserved` (`confinement.adapter-unserved`, 501) is external: the host decides whether an
  adapter opens. `local` always opens, so it has no `unserved` answer.
- `AdmitProfile` refuses in this order:
  - a set `ingress_routes` or `rate_bounds` (`confinement.reserved-field-unserved`, 501);
  - an adapter that did not open (`adapter-not-open`: `confinement.adapter-unserved`, 501);
  - a requested `inner` that differs from the opened row's, present or absent: `systemd` around
    `linux` on a scope opened around `local` and the reverse, an `inner` beside an adapter opened
    without one, `systemd` requested without its `inner` (`adapter-unserved`:
    `confinement.adapter-unserved`, 501);
  - on an adapter that proves `declared_insecure`, a missing or empty reason
    (`confinement.local-reason-missing`, 422);
  - on an adapter that proves `image_digest_pinned`, an image that is not pinned
    (`confinement.image-unpinned`, 422; see *Image digests* below);
  - on an adapter that proves `declared_insecure` but not `local_permitted`, any request
    (`confinement.local-refused`, 403);
  - the first required family the adapter does not serve, in decision 8's order
    (`confinement.family-unserved`, 501);
  - the first required fact it does not prove, in the `Fact` order
    (`confinement.fact-unserved`, 501).

  Otherwise it admits the profile, `declared_insecure: true` with the reason on a `local` adapter
  and `false` elsewhere.

A run is `local` exactly when its opened adapter proves `declared_insecure`: `local` itself, or
`systemd` opened around `local`. Admission reads this from the opened adapter row, not from the
request's `adapter`, and `OpenAdapter` makes the row's facts agree with its name: no `local` row
lacks `declared_insecure`, so no `local` profile is admitted without a reason or reported
`declared_insecure: false`. Admission never compares the requested `adapter` with a literal:
ESS 0.75.0 synthesis ignores a `when:` guard on an identity input bound to a captured instance
([beyond10x/ess#693](https://github.com/beyond10x/ess/issues/693)). `OpenAdapter`'s guards do
name adapters: it creates the row, so the identity is bound to no captured instance, and its
synthesized scenarios agree with `interpreted`.

### Image digests

A pinned reference ends with `@sha256:` and exactly 64 lowercase hex characters. ESS 0.75.0 has no
pattern predicate and no substring operator (its predicate reference, *Text shapes without
patterns*), so the model cannot state "the 64 characters after `@sha256:`, each lowercase hex".
It states every part it can, and refuses `confinement.image-unpinned` unless the reference:

- names `@sha256:`;
- ends with a lowercase hex character (`0` to `9`, `a` to `f`);
- is at least 73 characters long: a one-character name, `@sha256:` and 64 characters.

`registry.invalid/run@sha256:`, `…@sha256:latest` and `…@sha256:abc` are refused. A reference
such as `<long name>@sha256:abc` passes the model. **Obligation for the `oci` and `pod` adapter
code:** check that `@sha256:` is followed by exactly 64 lowercase hex characters and nothing else,
and refuse anything else `confinement.image-unpinned`; each adapter's tests assert it.

### The refusal audit record

ADR 0109 decision 2 makes every refusal an audit record (`ProfileRefused`, carrying the adapter and
the family or fact named). ESS refuses an event on an outcome that reports an error
(`refusal_mutated_state`), so the model cannot carry it; `UNMAPPED:` marks it in
`confinement.yaml`. **Obligation for the adapter code:** S0's tests
(`story:confinement-contract-on-linux-and-local`) assert that each refusal writes its audit record,
and each later adapter's tests do the same for its own refusals.

## Scenarios and lanes

An ESS scenario has no tag, so a scenario is named by adapter and lane: it begins with the adapter
and ends with `-on-<lane>`, and its summary begins with `lane <lane>:`. The rule each runner
follows:

- **An adapter's own lane selects the `<adapter>-*` scenarios that end `-on-<lane>`**: `local-*`
  on `portable`, `linux-*` and `systemd-*` on `delegated`, `macos-s1-*` (S1's adapter),
  `macos-s2-*` (the admissions S2 makes pass) and `macos-io-*` (design 32's standard input and
  live output) on `macos`, `oci-*` on `oci`, `pod-*` on `pod`.
- **`absent-<adapter>-*` scenarios run in the lane where that adapter is absent**:
  `absent-macos-*`, `absent-systemd-*`, `absent-oci-*` and `absent-pod-*` on `portable`,
  `absent-linux-*` on `macos`. No adapter's own selection matches them, so S1's `macos-s1-*`
  never picks up the scenario that needs `macos` missing.
- The open refusals of `systemd` around `local`, `oci` and `pod` (`*-open-without-*`) end
  `-on-portable`: S0's open checks the proved set before it returns and needs no such adapter on
  the host, so S0's tests feed it a set without the fact.
- **`model-*` scenarios end `-on-interpreted`** and no adapter lane selects them: they reach
  branches no declared adapter set reaches, and run only against the interpreted reference.

| lane | scenarios |
|---|---|
| `portable` | 20: thirteen `local-*` (admission, a missing reason, host policy, a filesystem view, four facts, both reserved fields, the open without `declared_insecure`, `stdin_pipe` admitted, `output_live` admitted), three `*-open-without-*` for `systemd` around `local`, `oci` and `pod`, and four `absent-*` (`macos`, `systemd`, `oci`, `pod`) with their setup in the summary |
| `delegated` | 20: eleven `linux-*`, nine `systemd-*` (four around `local`, five around `linux`, one of them requested on a scope opened around `local`); `stdin_pipe` and `output_live` each admitted on `linux` and on `systemd` around each |
| `macos` | 10: `absent-linux-*`, four `macos-s1-*`, three `macos-s2-*`, two `macos-io-*` (`stdin_pipe` admitted, `output_live` admitted) |
| `oci` | 7: admission by digest, `stdin_pipe` admitted on the lane's pinned runtime, `output_live` admitted, egress refused, three images refused `image-unpinned` (a tag, `@sha256:` with no digest, a short digest) |
| `pod` | 5: admission by digest, a filesystem view refused, `stdin_pipe` and `output_live` each refused `fact-unserved`, `@sha256:latest` refused `image-unpinned` |
| `interpreted` | 3: `model-*`, the six families every declared adapter serves, each refused by name, and `stdin_pipe` and `output_live` each refused by name on a row that holds `linux`'s other facts |

**Synthesized scenarios check the model against the interpreted reference only.** ESS names them
by command and outcome (`substrate.confinement.OpenAdapter/outcome/opened`) and cannot rename
them, so they carry no adapter and no lane, and no adapter lane selects them. Adapter lanes run the
authored scenarios above.

Synthesis writes 5 scenarios, all on `OpenAdapter`, and refuses 32 obligations, every one on
`AdmitProfile`. The open refusals that keep a `local` row declared insecure and an `oci` or `pod`
row digest-pinned are guards on the adapter identity, and with them ESS 0.75.0 synthesis arranges
no adapter row for an admission (`ESS-SYNTH-003`, "no candidate of the 15 tried satisfies
`substrate.confinement.Adapter` stored related row selecting this branch", and `ESS-SYNTH-001` for
the reserved fields and `adapter-not-open`). The model
keeps the refusals and the authored scenarios carry admission: each of the 37 outcomes of the two
commands has at least one scenario among the 65 authored. All 70 scenarios pass against ESS
0.75.0's `interpreted` target.

## Open questions (`UNMAPPED:` in `confinement.yaml`)

- `OpenAdapter` accepts `inner` beside an adapter other than `systemd`, and `systemd` without
  `inner`. Refusing either at open made ESS 0.75.0 synthesis refuse every admission
  (`ESS-SYNTH-002`). Admission refuses a requested `inner` that differs from the opened row's,
  which the model decides without a source.
- The audit record of a refusal (`ProfileRefused`): an adapter-test obligation, above.
- What an image beside `local`, `linux`, `macos` or `systemd` means. The model ignores it.
- The shape of an ingress route and a rate bound. Any value is refused.
- An `oci` or `pod` profile with no image is refused as unpinned. This fails closed, and no source
  says so.
- The facts no story declares: `exec_subpaths` and `private_tmpdir` on `linux`; whole-tree kill,
  IPC, `private_tmpdir` and `paths_are_host_paths` on `oci`; whole-tree kill on `pod`; the `oci`
  story's rootless fact, which has no `Fact` variant; and the audit snapshot fact of decision 2.
