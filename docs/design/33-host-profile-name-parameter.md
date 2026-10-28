# Design 33: the host execution profile name is a deployment parameter

Status: proposed, 2026-10-28. Owned by `story:host-profile-name-is-a-deployment-parameter`
(`epic:kubernetes-deployment-and-driver`), which carries out decision A of
`decision-blocker:chart-lane-host-profile`. It refines the AppArmor part of
[design 25](25-container-private-host-execution.md) and the chart of
[ADR 0046](../../adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md). It
changes no wire byte, no contract bundle and no rule a deployed profile enforces (§ 8).

The local chart lane runs its daemon on a kind node, which shares the host kernel. That kernel may
already hold `substrate-host-exec-v1` for another workload. The installer refuses a loaded profile
whose source it does not hold, and the lane must never replace or remove a profile it did not load,
so the lane needs a profile of its own. Today the name cannot change: five places fix it.

## 1. Where the name is fixed today

| what | where |
|---|---|
| the policy declares `profile substrate-host-exec-v1` and names itself as the peer of its `signal` and `ptrace` rules | `deploy/apparmor/substrate-host-execution:5`, `:39`, `:40` |
| the installer's name, its source file `substrate-host-exec-v1.apparmor`, and the policy it compiles in for the exact-source check | `crates/substrate-daemon/src/container_profiles.rs:12-15` |
| the bootstrap requires the label `substrate-host-exec-v1 (enforce)` | `crates/substrate-daemon/src/container_bootstrap.rs:90` |
| the daemon container selects `localhostProfile: substrate-host-exec-v1` | `deploy/helm/substrate/templates/statefulset.yaml:253` |
| the static chart lane requires that selection | `xtask/src/chart.rs:35`, `:976` |

The policy file's SHA-256 is `e3ff9b10eb633585354cedbcbe1dafb4ec01c4296ea1085677221adca3b6843a`,
the hash every container-runtime review result records.

## 2. The name

A profile name is either the default `substrate-host-exec-v1`, or the default followed by `-` and
a suffix of 1 to 40 characters from `[a-z0-9-]` that starts and ends with a letter or digit. The
longest name is 63 characters.

- **Only names of this family.** The name reaches `localhostProfile` and the installer's parser.
  A free name could select a profile the installer never rendered, such as `unconfined` or another
  workload's profile. A family name is always one this installer renders from the policy below.
- **`v1` stays in the name.** It names the policy text. A suffix tells two loaded copies of that
  text apart. It never names a different policy.
- **No quoting or escaping is needed.** The grammar admits no character that AppArmor's parser,
  YAML or a shell treats specially. The name is substituted into policy text, so this also rules
  out rule injection: no `{`, `}`, `,`, space or line break can get in.

The installer, the bootstrap, the chart schema and the chart templates each enforce this grammar.
A name outside it is refused before anything is written, loaded or started.

## 3. One template, rendered with the name

The committed `deploy/apparmor/substrate-host-execution` stays the one source and stays
byte-for-byte what it is. It is the template. Its token is the default name, which occurs exactly
three times: in the `profile` declaration and as the `peer=` of the `signal (send,receive)` and
`ptrace` rules. Rendering with a name replaces those three occurrences and nothing else. A test
holds the token to exactly those three places, so an edit that adds a fourth occurrence, such as
a comment naming the profile, fails until this design is revisited.

- **Rendering with the default is the identity**: the rendered bytes are the committed bytes.
- **The peers are renamed with the declaration** because they name the profile itself. A process
  under the profile may signal and trace other processes under the same profile, and nothing
  else. Keeping `peer=substrate-host-exec-v1` in a profile with another name would let the lane's
  processes signal and trace processes of the default profile, a workload the lane does not own.
  That would change what the profile enforces. Renaming the peers keeps the rule's meaning.
- **The installer renders in memory** from the policy it compiles in (`include_bytes!`). It reads
  no template at run time. The image still ships the committed file at
  `/usr/share/substrate/apparmor/substrate-host-execution` (`Dockerfile:59`). That is the default
  rendering, unchanged.

## 4. The installer: exact source, exact name

`substrate-container-profiles` gains `--apparmor-profile <name>`, default `substrate-host-exec-v1`.
The `--check` and `--hold` modes are unchanged, and the readiness probe passes the same name.

- The source file is `/node-profiles/<name>.apparmor`. For the default that is today's
  `substrate-host-exec-v1.apparmor`.
- The expected bytes are `render(<name>)`. Every comparison the installer makes today against the
  compiled-in policy is made against those bytes: the refusal of a loaded profile without its exact
  retained source, the refusal of an existing file whose bytes differ, and `--check`. A file
  holding another name's rendering, the default's included, is refused as differing bytes.
- "Loaded" means a registry line beginning with `<name> (`. "Enforced" means a line that is exactly
  `<name> (enforce)`. A loaded `substrate-host-exec-v1` does not count as loaded
  `substrate-host-exec-v1-lane-…`, and the reverse holds too.
- The parser loads only `<name>.apparmor`, whose one profile is `<name>`. The installer never
  reads, replaces or removes any other loaded profile, the default included. As today, it removes
  nothing.

The seccomp file is unchanged: one fixed name and fixed bytes, installed idempotently. Two
installers on one node with different AppArmor names write the same seccomp bytes to the same path,
which `install_file` already accepts.

## 5. The bootstrap learns the name from its own argument

`substrate-container-exec` gains `--apparmor-profile <name>`, default `substrate-host-exec-v1`,
parsed before `--` and never passed to the daemon. The label check becomes
`label.trim() == "<name> (enforce)"`. A label for another name, the default included when a name
is set, and a complain-mode label are refused with today's message. The bootstrap reads no
environment variable and no file to learn the name. The deployment states it in the container's
own arguments, beside the `localhostProfile` that applies it.

## 6. The chart value

`execution.appArmorProfile`, default `substrate-host-exec-v1`, in `values.yaml` and required by
the closed `values.schema.json` with the grammar of § 2 as its `pattern`. The value is stated once
and read by both workloads that need it:

- the daemon container's `appArmorProfile.localhostProfile`;
- the bootstrap's `--apparmor-profile <name>`, before `--`, after `--project-quotas`;
- the node-profile installer's `--apparmor-profile <name>`, in its command and its readiness probe.

Both flags are rendered only when the value is not the default, and the `localhostProfile` line is
the literal it is today when it is. So with the value unset `helm template` renders exactly
today's bytes. Every interpolated name is quoted, which the template quoting test requires.
`substrate.checks` refuses a name outside the grammar when the schema is skipped
(`--skip-schema-validation`). The static lane renders a named profile among its accepted cases, and
its pod checks require the selected `localhostProfile`, the bootstrap flag and the installer flag
to agree with the value. They also require both flags to be absent at the default.

## 7. The chart lane: a name of its own, and only its own profile touched

`cargo xtask chart-lane cluster --node-profiles` names its profile
`substrate-host-exec-v1-lane-<ulid>`, with a fresh lowercased ULID for each run, and passes it as
`execution.appArmorProfile`. The run then holds that name throughout:

- Its prerequisite refuses the run only if *its own* name is already loaded. It no longer refuses
  because `substrate-host-exec-v1` or any other profile is loaded, and it never reads one to decide
  anything else.
- Scenario 3 requires the daemon and its init to run under `<lane name> (enforce)`, and the
  container's declared and runtime AppArmor profile to be the lane name.
- At teardown it removes only the lane name, through the node, with the parser reading
  `/var/lib/kubelet/seccomp/b10x-substrate/<lane name>.apparmor`, the file its own installer wrote.
  A failed removal is reported with the exact name, and the run prints no `Chart-lane:` record.
- A run that dies before teardown leaves its own uniquely named profile loaded until the host
  reboots or someone removes it by name. It blocks no later run, because each run has its own name,
  and it is never another workload's profile.

The verb's `--node-profiles` mode lives on the open branch of PR 190
(`story:chart-lane-local-execution-record`), which is not part of this story's base. This story
lands the parameter. The lane's use of it is applied on that branch when it is rebased onto this
work, which the blocker's ordering already requires.

## 8. Unset is today, byte for byte, and why this needs no ADR

With the parameter unset:

| surface | bytes |
|---|---|
| rendered policy | `render("substrate-host-exec-v1")` is the committed file (SHA-256 `e3ff9b10…843a`) |
| installer | source file `substrate-host-exec-v1.apparmor`, the same expected bytes, the same loaded and enforced lines, the same parser invocation |
| bootstrap | requires exactly `substrate-host-exec-v1 (enforce)` |
| `helm template` | identical output for every values file that does not set `execution.appArmorProfile` |

**No rule a deployed profile enforces changes.** The policy body is the same text. Only the
name differs, and its self-references follow it (§ 3). No deployment that leaves the value unset
loads, selects or checks a different profile.

**No ADR is needed.** ADR 0046 decision 5 selects "the Localhost seccomp and AppArmor profiles of
design 25". It does not fix the name, and the profile stays design 25's profile. Decision 4's closed
schema stays closed: the new property is bounded by a pattern. Design 25 requires the installer to
install "only the image's compiled-in versioned bytes" and to refuse a loaded name without its
exact retained source. Both still hold. The compiled-in bytes are rendered with a name from a
closed grammar, and the comparison is exact against that rendering. Nothing reaches the wire,
a contract bundle or an accepted refusal.

## 9. Verification

Tests are written before code, and each runs its scenario once:

- **Rendering**: the template holds the token exactly three times, at the declaration and the
  two peers. Rendering with the default returns the committed bytes. Rendering with a lane name
  changes exactly those three places.
- **Grammar**: the default and family names are accepted. `unconfined`, another version, an empty
  or upper-case suffix, a trailing `-`, a space, a line break, `{` and a 64-character name are
  refused.
- **Installer**: the source file is `<name>.apparmor`. The rendered bytes for one name are refused
  as the source of another. Loaded and enforced match the exact name only.
- **Bootstrap**: `<name> (enforce)` is accepted. A complain label, another family name and the
  default when a name is set are refused. At the default the required label is exactly today's.
- **Chart**: the schema admits a family name and refuses one outside the family, offline. The
  template refuses an injected name with the schema skipped. The static lane renders a named
  profile and checks all three uses. `helm template` of the default values is diffed against the
  base commit's output.
- **Lane**: its unit tests hold the lane name to the grammar and make it unique per run. The
  prerequisite ignores a loaded default, and removal names only the lane's own file. The
  cluster run itself is a workstation lane (ADR 0046 K9 C) and is recorded when it runs.

## 10. Not decided here

- Removing a profile in production. The installer still removes nothing (design 25).
- A parameterised seccomp file name. The seccomp profile lives in a kubelet directory, not in
  the shared kernel, so the lane's kind node has its own copy.
- A second policy version. A `v2` policy is a new template and a new default, not a suffix.
