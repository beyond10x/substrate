#!/usr/bin/env bash
# Runs the runtime-vector suite's delegated lane.
#
# The delegated cases need a cgroup v2 subtree carrying cpu/memory/pids that the test process is
# itself inside, and whose root is process-free. A user session's own scope is root-owned, so
# `mkdir` in it fails and the lane reports itself absent (invariant 3) rather than passed. This
# asks systemd for a delegated scope instead, which needs no privilege.
#
#     bash scripts/delegated-lane.sh
#
# Without it the lane is absent, not green: `cargo test` alone proves the portable refusal only.
# It runs both delegated lanes: the host crate's own cases and the clean-room runner's.
#
# **It does not cover the real project-quota cases.** Those are `#[ignore]`d, this script passes no
# `--ignored`, and nothing here provisions the filesystem they need — so a green run of this lane
# says nothing about them. It closes by naming them, through `cargo xtask quota-lane --inventory`;
# `cargo xtask quota-lane` is what runs them, and refuses by name when their fixture is absent.
set -euo pipefail

if [[ "${SUBSTRATE_DELEGATED_INNER:-}" != "1" ]]; then
  exec systemd-run --user -p Delegate=yes --scope --quiet \
    -- env SUBSTRATE_DELEGATED_INNER=1 "$0" "$@"
fi

root="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
controllers="$(cat "${root}/cgroup.controllers")"
for want in cpu memory pids; do
  case " ${controllers} " in
    *" ${want} "*) ;;
    *) echo "delegated-lane: ${root} does not carry ${want} (has: ${controllers})" >&2; exit 1 ;;
  esac
done

# The delegation root must be process-free: move this process into a child group.
mkdir -p "${root}/runner"
echo $$ > "${root}/runner/cgroup.procs"
echo "+cpu +memory +pids" > "${root}/cgroup.subtree_control"

echo "delegated-lane: root ${root}, controllers ${controllers}"
export SUBSTRATE_VECTORS_CGROUP_ROOT="${root}"

if ! command -v jq >/dev/null 2>&1; then
  echo "delegated-lane: jq is required to read the paths cargo reports" >&2
  exit 1
fi

# The executable gate (design 35): the helper that holds a run to its executable allow-list. The
# linux adapter of this lane is opened with it, because every `linux` scenario's open step states
# `exec_subpaths`; without it those steps fail on the missing fact instead of passing silently.
#
# **The helper is statically linked, and this lane never falls back to a dynamic one** (D29). The
# driver refuses a helper whose program headers hold `PT_INTERP` (`config.exec-gate-invalid`): a
# loader would run before the helper and read files a profile can mount. So the helper is built
# for the host's musl target, as the `executor` stage of the `Dockerfile` builds
# `substrate-executor`, and a build that fails, or yields an interpreter, ends the lane here by
# name. `rust-toolchain.toml` lists `x86_64-unknown-linux-musl`; the package depends on `libc`
# alone, so the target's own self-contained C runtime links it and no musl C compiler is needed.
#
# The path is the one cargo reports, for the reason given at the daemon below. The lane names a
# copy, not cargo's output: cargo hard-links `substrate-exec-gate` to its file under `deps/`, and
# the driver refuses a helper with a second link (`config.exec-gate-invalid`), which may lie where
# a run can write.
gate_host="$(rustc -vV | sed -n 's/^host: //p')"
case "${gate_host}" in
  *-linux-gnu) gate_target="${gate_host%-gnu}-musl" ;;
  *-linux-musl) gate_target="${gate_host}" ;;
  *) echo "delegated-lane: no static target is known for the host '${gate_host}'; the executable gate must be statically linked and the lane does not fall back to a dynamic helper" >&2; exit 1 ;;
esac
if ! command -v readelf >/dev/null 2>&1; then
  echo "delegated-lane: readelf is required to show that the executable gate has no PT_INTERP" >&2
  exit 1
fi
# The named executable of one build of the helper: cargo's own report, or nothing.
gate_executable() {
  cargo build -p b10x-substrate-exec-gate --bin substrate-exec-gate --locked "$@" \
    --message-format=json-render-diagnostics \
    | jq -r 'select(.reason == "compiler-artifact" and .target.name == "substrate-exec-gate"
                    and .executable != null) | .executable'
}
gate_built="$(gate_executable --target "${gate_target}")" || gate_built=""
if [[ -z "${gate_built}" || "${gate_built}" == *$'\n'* || ! -x "${gate_built}" ]]; then
  echo "delegated-lane: the executable gate could not be built statically for ${gate_target} (got: '${gate_built}'); install the target with 'rustup target add ${gate_target}'. The lane does not fall back to a dynamic helper" >&2
  exit 1
fi
gate_headers="$(readelf -lW "${gate_built}")"
if ! grep -q '^Program Headers:' <<<"${gate_headers}" || grep -q 'INTERP' <<<"${gate_headers}"; then
  echo "delegated-lane: the executable gate built for ${gate_target} is not statically linked (${gate_built} has a PT_INTERP, or no program headers); the lane does not fall back to a dynamic helper" >&2
  exit 1
fi
gate_bin="$(dirname "${gate_built}")/substrate-exec-gate.lane"
rm -f "${gate_bin}"
cp "${gate_built}" "${gate_bin}"
echo "delegated-lane: executable gate ${gate_bin} (static, ${gate_target})"
export SUBSTRATE_EXEC_GATE="${gate_bin}"

# Cargo's default build of the same helper, which is dynamically linked: the one case that hands
# it to the driver requires the open refused (`ESS confinement linux exec helper dynamic`). A
# copy for the same reason as above; the lane names it and never uses it as the helper.
gate_default="$(gate_executable)" || gate_default=""
if [[ -z "${gate_default}" || "${gate_default}" == *$'\n'* || ! -x "${gate_default}" ]]; then
  echo "delegated-lane: cargo reported no single executable for the default build of substrate-exec-gate (got: '${gate_default}')" >&2
  exit 1
fi
if ! readelf -lW "${gate_default}" | grep -q 'INTERP'; then
  echo "delegated-lane: the default build of substrate-exec-gate has no PT_INTERP, so the case that requires a dynamic helper refused has nothing to hand the driver" >&2
  exit 1
fi
gate_dynamic="$(dirname "${gate_default}")/substrate-exec-gate.dynamic"
rm -f "${gate_dynamic}"
cp "${gate_default}" "${gate_dynamic}"
export SUBSTRATE_EXEC_GATE_DYNAMIC="${gate_dynamic}"

# The host crate has delegated cases of its own — a pty session's echo, controlling terminal,
# SIGWINCH, output bound and whole-tree cleanup (design 13). They read the same variable and are
# *absent* without it, so a lane that ran only the daemon's runner would leave them looking green
# while never running.
#
# **No name filter.** Selecting them by the substring `pty` skipped two cases whose names do not
# carry it, and a filter that matches nothing still exits 0 — so a rename would have made this lane
# green having run nothing, which is the exact failure it exists to prevent. The
# absent-without-SUBSTRATE_VECTORS_CGROUP_ROOT guard inside each case does the selecting, exactly as
# the daemon half already does.
#
# One thread, because these cases share one delegation root: `ProcessRuntime::new` reconciles every
# `substrate-ex_*` cgroup under its configured root at construction, which is right for the one
# daemon that owns a root and fatal for six drivers that share one.
cargo test -p b10x-substrate-host --locked -- --nocapture --test-threads=1

# The confinement contract's linux scenarios (ADR 0109) ran above, but a case that reported itself
# absent exits 0 too. Run them again by name and require the count the case printed, so an absent
# or empty lane fails here instead of reading as green.
confinement_log="$(cargo test -p b10x-substrate-host --locked --test ess_confinement_linux \
  -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${confinement_log}" >&2; exit 1; }
printf '%s\n' "${confinement_log}"
confinement_line="$(printf '%s\n' "${confinement_log}" \
  | grep -E '^ESS confinement linux delegated lane: [1-9][0-9]* scenarios executed' || true)"
if [[ -z "${confinement_line}" ]]; then
  echo "delegated-lane: the confinement linux scenarios did not execute" >&2
  exit 1
fi
echo "delegated-lane: ${confinement_line}"

# The confinement contract's systemd scenarios, both inner adapters, against a transient user scope
# the adapter asks this session's user manager for. Required by count for the same reason.
systemd_log="$(cargo test -p b10x-substrate-host --locked --test ess_confinement_systemd \
  -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${systemd_log}" >&2; exit 1; }
printf '%s\n' "${systemd_log}"
systemd_line="$(printf '%s\n' "${systemd_log}" \
  | grep -E '^ESS confinement systemd delegated lane: [1-9][0-9]* scenarios executed' || true)"
if [[ -z "${systemd_line}" ]]; then
  echo "delegated-lane: the confinement systemd scenarios did not execute" >&2
  exit 1
fi
echo "delegated-lane: ${systemd_line}"

# The confinement contract's adversary and security-review cases that need the delegated root ran
# above too, and are absent, exit 0, without it. Run each binary again and require every case's
# own `executed` line and no `ABSENT` line, so a case that did not run fails here. `-p <package>`
# first names a package other than the host crate.
require_executed() {
  local package=b10x-substrate-host
  if [[ "$1" == "-p" ]]; then
    package="$2"
    shift 2
  fi
  local binary="$1"
  shift
  local log
  log="$(cargo test -p "${package}" --locked --test "${binary}" \
    -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${log}" >&2; exit 1; }
  printf '%s\n' "${log}"
  if grep -q 'ABSENT' <<<"${log}"; then
    echo "delegated-lane: ${binary} reported a case absent" >&2
    exit 1
  fi
  local line
  for line in "$@"; do
    if ! grep -qF "${line}" <<<"${log}"; then
      echo "delegated-lane: ${binary} did not print '${line}'" >&2
      exit 1
    fi
  done
  echo "delegated-lane: ${binary} executed its delegated cases"
}
require_executed adversary_confinement_s0_delegated \
  'adversary writable root: executed' 'adversary render parity: executed'
require_executed security_review_confinement 'security review linux start env: executed' \
  'egress listener review: executed, none held during the run, baseline restored'
require_executed sandbox_parity \
  'sandbox parity bubblewrap delegated: executed, 23 rows present'
# Design 29: scoped workspace write and output retention, each case by its own line. Design 35:
# the `exec` lines, runs held to an executable allow-list by the helper built above. The first
# of them is the acceptance: a program beneath a listed directory runs, and one outside it fails
# inside the run with status 126. The `exec deny` lines are the deny-list beside it
# (`story:executable-deny-list`): probes K2 and K8 through the driver, the acceptance (`/usr/bin`
# allowed, one program beneath it denied), and the two cases kept at the stakeholder's request,
# a program that is a link to a denied program and one directory under two names, the second in
# the base view's form (`/usr/bin` listed, `/bin/id` denied) and from two declared roots. The
# base view's form is absent, so refused here, on a host whose `/bin` is not a link to `/usr/bin`.
# The `limit` lines are design 36's story 1: runs ended by their memory bound, their CPU budget
# and their timeout name that limit, and a run that exits 0 names none. The `run ended` line is
# its story 3: a run over its memory bound that nobody observes puts one `run_ended` naming the
# memory limit into its profile's sink. The `process limit hits` lines are its story 2: a run
# over its process bound counts the refused creations on its ended observation and on its
# `run_ended`, a run under it counts zero on both, and `observe` of the first run while it still
# runs counts above zero. The `usage` lines are design 36 § 3a: a run that sleeps one second in
# eight processes, allocates 64 MiB and spends one second of CPU carries at least each figure on
# its ended observation and its `run_ended`, and a run its timeout ended carries both the limit
# and its usage.
require_executed ess_confinement_linux \
  'ESS confinement linux scoped write: executed' \
  'ESS confinement linux scoped git: executed' \
  'ESS confinement linux subtree symlink: executed' \
  'ESS confinement linux nested git: executed' \
  'ESS confinement linux hard link: executed' \
  'ESS confinement linux submodule subtree: executed' \
  'ESS confinement linux planted repository: executed' \
  'ESS confinement linux output while running: executed' \
  'ESS confinement linux retention head: executed' \
  'ESS confinement linux retention tail: executed' \
  'ESS confinement linux retention per stream: executed' \
  'ESS confinement linux stdin echo: executed' \
  'ESS confinement linux stdin cancel: executed' \
  'ESS confinement linux stdin close waits: executed' \
  'ESS confinement linux stdin after the end: executed' \
  'ESS confinement linux stdin pending at the end: executed' \
  'ESS confinement linux stdin recorded output: executed' \
  'ESS confinement linux live natural end: executed' \
  'ESS confinement linux live stall: executed' \
  'ESS confinement linux live bounded: executed' \
  'ESS confinement linux live both streams: executed' \
  'ESS confinement linux live kill: executed' \
  'ESS confinement linux live pending read: executed' \
  'ESS confinement linux exec allow-list: executed' \
  'ESS confinement linux exec entries: executed' \
  'ESS confinement linux exec script refused: executed' \
  'ESS confinement linux exec limit interpreter: executed' \
  'ESS confinement linux exec limit loader: executed' \
  'ESS confinement linux exec rename: executed' \
  'ESS confinement linux exec limit rename into: executed' \
  'ESS confinement linux exec limit recreated: executed' \
  'ESS confinement linux exec limit second mount: executed' \
  'ESS confinement linux exec refused at start: executed' \
  'ESS confinement linux exec root in a workspace: executed' \
  'ESS confinement linux exec follow into a writable mount: executed' \
  'ESS confinement linux exec swapped link: executed' \
  'ESS confinement linux exec place unserved: executed' \
  'ESS confinement linux exec helper after open: executed' \
  'ESS confinement linux exec helper unset: executed' \
  'ESS confinement linux exec helper unrestricting: executed' \
  'ESS confinement linux exec helper protocol: executed' \
  'ESS confinement linux exec helper writable: executed' \
  'ESS confinement linux exec withheld: executed' \
  'ESS confinement linux exec limit preload: executed' \
  'ESS confinement linux exec variables: executed' \
  'ESS confinement linux exec helper preload half: executed' \
  'ESS confinement linux exec path limit: executed' \
  'ESS confinement linux exec helper dynamic: executed' \
  'ESS confinement linux exec limit preload file: executed' \
  'ESS confinement linux exec deny K2: executed' \
  'ESS confinement linux exec deny K8: executed' \
  'ESS confinement linux exec deny-list: executed' \
  'ESS confinement linux exec deny link to denied: executed' \
  'ESS confinement linux exec deny two names base view: executed' \
  'ESS confinement linux exec deny two names declared roots: executed' \
  'ESS confinement linux exec deny-only: executed' \
  'ESS confinement linux exec deny shapes: executed' \
  'ESS confinement linux exec deny loader: executed' \
  'ESS confinement linux exec deny limit hard link: executed' \
  'ESS confinement linux exec deny limit copy: executed' \
  'ESS confinement linux exec deny limit loader: executed' \
  'ESS confinement linux exec deny limit script: executed' \
  'ESS confinement linux exec deny limit replaced: executed' \
  'ESS confinement linux exec deny limit unreadable directory: executed' \
  'ESS confinement linux exec deny probe held mount: executed' \
  'ESS confinement linux limit memory: executed' \
  'ESS confinement linux limit cpu time: executed' \
  'ESS confinement linux limit wall time: executed' \
  'ESS confinement linux limit none: executed' \
  'ESS confinement linux run ended memory: executed' \
  'ESS confinement linux process limit hits over: executed' \
  'ESS confinement linux process limit hits under: executed' \
  'ESS confinement linux process limit hits running: executed' \
  'ESS confinement linux usage: executed' \
  'ESS confinement linux usage limit: executed'
# Design 36, story 1: the limit that ended a run, around `local` from the scope's own watch and
# the run cgroup's OOM count, around `linux` from the driver. Story 3: one `run_ended` per run
# nobody observes, around each inner adapter, with the outer adapter's name. Story 2: around
# `local`, the run cgroup's count of refused process creations, above zero over the bound, on
# `observe` while the run runs and on its end, and zero under it, on the ended observation and
# on `run_ended`. Design 36 § 3a: the usage run around `local`, read by the end task from the
# run cgroup and its own clock, and beside the timeout's limit; around `linux`, the inner
# driver's figures, relabelled.
require_executed ess_confinement_systemd \
  'ESS confinement systemd local usage: executed' \
  'ESS confinement systemd local usage limit: executed' \
  'ESS confinement systemd linux usage: executed' \
  'ESS confinement systemd local limits memory: executed' \
  'ESS confinement systemd local limits cpu time: executed' \
  'ESS confinement systemd local limits wall time: executed' \
  'ESS confinement systemd linux limit wall time: executed' \
  'ESS confinement systemd local run ended: executed' \
  'ESS confinement systemd linux run ended: executed' \
  'ESS confinement systemd local process limit hits running: executed' \
  'ESS confinement systemd local process limit hits over: executed' \
  'ESS confinement systemd local process limit hits under: executed' \
  'ESS confinement systemd local scoped write: executed' \
  'ESS confinement systemd linux scoped write: executed' \
  'ESS confinement systemd local retention tail: executed' \
  'ESS confinement systemd local stdin: executed' \
  'ESS confinement systemd linux stdin: executed' \
  'ESS confinement systemd local live natural end: executed' \
  'ESS confinement systemd local live stall: executed' \
  'ESS confinement systemd linux live natural end: executed' \
  'ESS confinement systemd linux live stall: executed' \
  'ESS confinement systemd linux exec_subpaths admission: executed' \
  'ESS confinement systemd linux exec_denied_paths admission: executed' \
  'ESS confinement systemd linux exec allow-list: executed' \
  'ESS confinement systemd linux exec deny-list: executed' \
  'ESS confinement systemd linux exec preload: executed' \
  'ESS confinement systemd linux exec withheld: executed'

# Design 35, the adversary pass and the security review of the allow-list: runs held to a list
# against a preload object named by the run's own environment (D28), other names of an unlisted
# program, the kernel's path limit, and the floor of an unlisted start. The preload cases build
# a shared object with the host's `cc`, and are absent, so refused here, without one.
require_executed adversary_exec_lists \
  'adversary exec list preload: executed' \
  'adversary exec list aliases: executed' \
  'adversary exec list entry names: executed' \
  'adversary exec list link cycle: executed' \
  'adversary exec list read-only file: executed' \
  'adversary exec list writable root: executed' \
  'adversary exec list path limit: executed, refused before dispatch' \
  'adversary exec list unmounted writable root: executed' \
  'adversary2 exec list ld.so.preload: executed' \
  'adversary3 exec list helper shadow: executed, refused before dispatch'
require_executed security_exec_gate \
  'security exec gate floor argv: executed' \
  'security exec gate floor run: executed' \
  'security exec gate preload: executed' \
  'security exec gate preload before the helper: executed' \
  'security exec gate without landlock: executed' \
  'security exec gate root spelling: executed' \
  'security exec gate published facts: executed'
# D29, the measured case: a read-only file a profile mounts at `/etc/ld.so.preload` names an
# object of the workspace, and the run prints what it prints without the file. The adversary's
# case of the same is required above.
require_executed security2_exec_gate \
  'security2 exec gate loaded where: executed' \
  'security2 exec gate ld.so.preload: executed, the list held' \
  'security2 exec gate helper mount: executed' \
  'security2 exec gate value visibility: executed' \
  'security2 exec gate rule 7: executed' \
  'security2 exec gate piped: executed'
# The third look. Its trace case runs the driver's own start of one listed run with bubblewrap
# under `/usr/bin/strace -f` and reads which programs are executed before the ruleset; it is
# absent, so refused here, on a host without strace or without a `cc` that builds a shared object.
require_executed security3_exec_gate \
  'security3 exec gate trace: executed' \
  'security3 exec gate binfmt: executed' \
  'security3 exec gate unstartable image: executed, the byte test'

# ADR 0050: one confined run calls `EXT4_IOC_MOVE_EXT` on two files of its writable workspace and
# must be answered `EACCES`, with both files unchanged when read cold on the host. The case builds
# its program with the host's `cc -static` and stages its workspace under cargo's target
# directory, which has to be on ext4: on any other file system the request does not exist and
# the case would show nothing.
#
# The case's `executed` line is required, with one absence taken instead: the case's own line
# that the staging file system is not ext4, which a workstation's target directory may not be.
# It is named below as absent, never as passed. Every other absence (no delegated root, no
# bubblewrap, no `cc` that links a static program) fails the lane. On a hosted runner
# (`GITHUB_ACTIONS` is `true`) that absence is not taken either: the lane there records a
# release, so the `executed` line is required. Not `require_staged`: on a host that grants a
# user namespace no mount, its `?<name>|<reason>` form takes any `ABSENT (` line of the case,
# and a hosted runner is such a host.
require_executed_unless() {
  local binary="$1" name="$2" reason="$3"
  local log
  log="$(cargo test -p b10x-substrate-host --locked --test "${binary}" \
    -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${log}" >&2; exit 1; }
  printf '%s\n' "${log}"
  local absent
  absent="$(grep -c 'ABSENT' <<<"${log}" || true)"
  if grep -qF "${name}: executed" <<<"${log}"; then
    if [[ "${absent}" != 0 ]]; then
      echo "delegated-lane: ${binary} reported a case absent" >&2
      exit 1
    fi
    echo "delegated-lane: ${binary} executed its delegated cases"
    return
  fi
  if [[ "${GITHUB_ACTIONS:-}" != "true" && "${absent}" == 1 ]] \
    && grep -qF "${name}: ABSENT (${reason}" <<<"${log}"; then
    echo "delegated-lane: ${reason}; absent, not passed: ${binary}: ${name}"
    return
  fi
  echo "delegated-lane: ${binary} did not print '${name}: executed'" >&2
  exit 1
}
require_executed_unless floor_refuses_block_exchange \
  'floor block exchange' 'the staging file system is not ext4'
# The adversary's cases of ADR 0050: the filter's answer for the f2fs request, and the ext4
# request refused at every entry of an exec, a pipe session and a pty session. Required on the
# hosted runner, which stages on ext4. Elsewhere they may be absent for the reason the case above
# names; the host-crate run further up has run them, and an absence there is not read as run.
if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
  require_executed adversary_floor_block_exchange \
    'adversary floor block exchange: f2fs survey: executed' \
    'adversary floor block exchange: exec: executed' \
    'adversary floor block exchange: pipe session: executed' \
    'adversary floor block exchange: pty session: executed'
fi

# The helper's own cases start cargo's test build of `substrate-exec-gate`, which is dynamically
# linked, with no sandbox and no driver around it: what they hold is the helper's own argument
# grammar, ruleset and environment rule, which do not depend on how it is linked. They need no
# delegated root, only a kernel with Landlock ABI 2 and a scratch directory on a listed file
# system (`ext4`, `overlay`, `tmpfs`), and are absent, exit 0, without either; so each is
# required by its line here, where the lists are proved.
require_executed -p b10x-substrate-exec-gate gate \
  'exec gate listed directory: executed' \
  'exec gate file entry: executed' \
  'exec gate refer: executed' \
  'exec gate limit link out of a listed directory: executed' \
  'exec gate follow into a writable mount: executed' \
  'exec gate unestablished place: executed' \
  'exec gate program exec failure: executed' \
  'exec gate descriptors: executed' \
  'exec gate environment: executed' \
  'exec gate own environment: executed' \
  'exec gate preload operand: executed' \
  'exec gate deny listed file: executed' \
  'exec gate deny directory: executed' \
  'exec gate deny program and loader: executed' \
  'exec gate deny limit hard link elsewhere: executed' \
  'exec gate deny limit replaced and copied: executed' \
  'exec gate deny entry into a writable mount: executed' \
  'exec gate deny entry beneath a file: executed' \
  'exec gate deny beside a name that is not UTF-8: executed'

# Design 35 D30, what is mounted beneath a location a run can write, and the symbolic links a
# followed entry reads: these roots make their own mounts, each case in a user and mount
# namespace of its own, and need a host that grants such a namespace a mount. A workstation
# does; a hosted runner's AppArmor gives a user namespace no capability. Each case is required
# by name. Where the mount is granted it must print its `executed` line. Where it is not, its
# own `ABSENT (` line is taken instead, so a case that neither ran nor said why still fails,
# and the lane says in one line which cases did not run: there they are workstation evidence,
# never a passed case. A name written `?<name>|<reason>` depends on something else the host
# may lack (a mount this user cannot open, `/usr/local`, an overlay in a user namespace): its
# `ABSENT (` line is taken on any host where it gives that reason, and named. A name written
# `=<name>` is a case of such a root that makes no mount: its `executed` line is required on
# every host.
if unshare --user --map-root-user --mount -- mount -t tmpfs tmpfs /tmp >/dev/null 2>&1; then
  mounts_granted=1
else
  mounts_granted=0
fi
unstaged=()
require_staged() {
  local package=b10x-substrate-host
  if [[ "$1" == "-p" ]]; then
    package="$2"
    shift 2
  fi
  local binary="$1"
  shift
  local log
  log="$(cargo test -p "${package}" --locked --test "${binary}" \
    -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${log}" >&2; exit 1; }
  printf '%s\n' "${log}"
  local name reason always
  for name in "$@"; do
    reason=""
    always=0
    if [[ "${name}" == \?* ]]; then
      name="${name#\?}"
      reason="${name#*|}"
      name="${name%%|*}"
    elif [[ "${name}" == =* ]]; then
      name="${name#=}"
      always=1
    fi
    if grep -qF "${name}: executed" <<<"${log}"; then
      continue
    fi
    if (( ! mounts_granted && ! always )) && grep -qF "${name}: ABSENT (" <<<"${log}"; then
      unstaged+=("${binary}: ${name}")
      continue
    fi
    if [[ -n "${reason}" ]] && grep -F "${name}: ABSENT (" <<<"${log}" | grep -qF "${reason}"; then
      unstaged+=("${binary}: ${name}")
      continue
    fi
    echo "delegated-lane: ${binary} did not print '${name}: executed'" >&2
    exit 1
  done
  echo "delegated-lane: ${binary} answered for each of its cases"
}
require_staged exec_lists_view_mounts \
  'ESS confinement linux exec view mounts'
require_staged security5_view_mounts \
  'security5 view mounts link through a skipped mount' \
  '?security5 view mounts ordinary|this host has no /usr/local/src'
require_staged adversary5_view_mounts \
  'adversary5 exec view mounts unlisted' \
  'adversary5 exec view mounts bind' \
  'adversary5 exec view mounts workspace root is a mount point' \
  '?adversary5 exec view mounts usr local|no /usr/local on this host' \
  'adversary5 exec view mounts stack beneath a root' \
  '?adversary5 exec view mounts unopenable mount beneath a root|this host has no mount of a listed type'
# The second review's roots: a root the count classes `beneath` is refused before dispatch,
# every entry is placed where it ends, and a followed path that passes through a directory a
# run can write is refused.
require_staged adversary6_view_mounts \
  'adversary6 exec view mounts beneath by the count' \
  'adversary6 exec view mounts entry on an unlisted type' \
  'adversary6 exec view mounts links'
require_staged security6_view_mounts \
  'security6 view mounts link on a mount beneath a root only' \
  '?security6 view mounts root with mounts this user cannot open|this host has no mount beneath' \
  '=security6 view mounts trusted link through the workspace'
# The third review's roots: a chain of roots classed alike in either order, a base directory
# that shows a workspace, an entry's end in a mount a run can write, and the driver's walk and
# the helper's ending at the same file.
require_staged adversary7_view_mounts \
  'adversary7 exec view mounts chain of roots' \
  '?adversary7 exec view mounts base directory|no /usr/local on this host' \
  'adversary7 exec view mounts ends' \
  'adversary7 exec view mounts walks'
require_staged security7_view_mounts \
  '?security7 view mounts base directory that holds the workspace root|this host has no /usr/local/src'
require_staged -p b10x-substrate-exec-gate view_mounts_gate \
  'exec gate view mounts follow into a mount beneath a writable mount' \
  'exec gate view mounts links of a followed path' \
  'exec gate view mounts path through a writable directory' \
  'exec gate view mounts beneath path placed' \
  'exec gate view mounts unlisted mount beneath a writable mount'
require_staged -p b10x-substrate-exec-gate adversary6_links_gate \
  'adversary6 exec gate links unplaced link' \
  'adversary6 exec gate links writable link' \
  'adversary6 exec gate links magic links' \
  'adversary6 exec gate links walk' \
  'adversary6 exec gate links no writable mount'
require_staged -p b10x-substrate-exec-gate adversary7_walk_gate \
  'adversary7 exec gate walk dot dot' \
  'adversary7 exec gate walk passed directory' \
  'adversary7 exec gate walk beneath ends'
require_staged -p b10x-substrate-exec-gate adversary5_view_mounts_gate \
  'adversary5 exec gate view mounts file bind' \
  'adversary5 exec gate view mounts chain and escapes' \
  'adversary5 exec gate view mounts beneath an unlisted type' \
  'adversary5 exec gate view mounts writable root is a mount point' \
  'adversary5 exec gate view mounts planted link on an unlisted type' \
  'adversary5 exec gate view mounts covered mounts' \
  '?adversary5 exec gate view mounts limit: overlay upper|no overlay mount in a user namespace on this host'
# The deny-list across the view's mounts (`story:executable-deny-list`, D30): a program denied
# in a mount beneath a listed directory, a listed mount beneath a denied directory, a program
# in a mount beneath a denied directory, and the mounts that cannot be compared. The same
# rule: each staged case by name, its `executed` line where a user namespace is granted a
# mount and its own `ABSENT (` line where it is not. The cases of these roots that make no
# mount are written `=<name>` and are required on every host. The adversary's nested host
# mount mounts in the test process itself, which no lane lets it: it is absent everywhere,
# named below, and `exec_denied_view_mounts` stages its shape in a namespace of its own.
require_staged exec_denied_view_mounts \
  'ESS confinement linux exec deny view mounts'
require_staged security4_exec_denied \
  '?security4 exec denied host mount|this host has no /usr/local/src' \
  '?security4 exec denied unplaced|this host has no vfat mount at /boot/efi' \
  '=security4 exec denied trace'
# The second security review's root. Its first case is the stated limit of a deny entry in a
# listed directory the run can write (design 35 § 8.2): it holds the measured figures. Three
# cases make no mount; the fourth stages its mounts in a namespace of its own.
require_staged security5_exec_denied \
  '=security5 exec denied limit created or moved by the run' \
  '=security5 exec denied unreadable expanded directory' \
  '=security5 exec denied program in a declared mount' \
  'security5 exec denied names across mounts'
# The adversary's second round. Its routes case makes no mount. The other stages a mount of a
# type outside the list beside, one directory below and beneath the deny entry.
require_staged adversary5_exec_denied_round2 \
  '=adversary5 exec deny routes' \
  'adversary5 exec deny beside unlisted mount'
# The third security look's root (S9-1, S9-2): one staged case of fourteen starts. The
# refusal before dispatch for a mount among the entries of an expanded directory is what the
# helper would end with status 125, and no start the helper would run.
require_staged security6_exec_denied \
  'security6 exec denied the refusal for a mount among the entries'
require_staged adversary4_exec_denied \
  '=adversary4 exec deny other names' \
  '=adversary4 exec deny limit created later' \
  '=adversary4 exec deny directory renamed' \
  '=adversary4 exec deny spellings' \
  '=adversary4 exec deny option names' \
  '=adversary4 exec deny program refused' \
  '=adversary4 exec deny equals allow' \
  '=adversary4 exec deny link entry' \
  '=adversary4 exec deny rule stop' \
  '?adversary4 exec deny nested host mount|this process may not mount'
require_staged -p b10x-substrate-exec-gate namespace_gate \
  'exec gate namespace host bind mount' \
  'exec gate namespace tmpfs' \
  '?exec gate namespace overlay|no overlay mount in a user namespace on this host' \
  '?exec gate namespace limit overlay lower directory|no overlay mount in a user namespace on this host'
require_staged -p b10x-substrate-exec-gate adversary4_namespace_gate \
  'adversary4 exec gate namespace second mount nested in a listed directory' \
  'adversary4 exec gate namespace deny entry beneath a nested mount' \
  'adversary4 exec gate namespace deny entry on a nested tmpfs' \
  'adversary4 exec gate namespace listed mount inside a denied directory' \
  'adversary4 exec gate namespace control without a nested mount'
require_staged -p b10x-substrate-exec-gate security4_gate \
  'security4 exec gate deny in a mount beneath a listed directory' \
  'security4 exec gate listed mount beneath a denied directory' \
  '=security4 exec gate rule bound'
# The adversary's second round on the helper alone: every case makes its namespace.
require_staged -p b10x-substrate-exec-gate adversary5_denied_reach_gate \
  'adversary5 exec gate denied reach probe shape, second listed directory' \
  'adversary5 exec gate denied reach probe shape, the expanded directory' \
  'adversary5 exec gate denied reach unlisted mount among the entries' \
  'adversary5 exec gate denied reach listed mount above an unlisted one' \
  'adversary5 exec gate denied reach denied root of a file system' \
  'adversary5 exec gate denied reach deny entry beneath a file'
if (( ${#unstaged[@]} > 0 )); then
  if (( mounts_granted )); then
    why="this host lacks what they need"
  else
    why="a user namespace is granted no mount on this host"
  fi
  echo "delegated-lane: ${why}; absent, not passed: $(IFS=';'; echo "${unstaged[*]}")"
fi
# The mount table as a tree, on tables written as text: no mount and no namespace, so it runs
# on every host. Required by the lines of its two long tables.
require_executed -p b10x-substrate-exec-gate adversary5_mount_tree \
  'adversary5 mount tree 10000 rows side by side: linked in' \
  'adversary5 mount tree 10000 rows deep: linked in'
require_executed -p b10x-substrate-exec-gate adversary6_reach \
  'adversary6 reach: '
# The helper's walk of a followed path, on directories of its own: no mount and no namespace.
# Its last case holds the helper under `/usr/bin/strace`, as the trace case of
# `security3_exec_gate` does the driver's start.
require_executed -p b10x-substrate-exec-gate security6_gate \
  'security6 exec gate links: program and loader: executed' \
  'security6 exec gate links: absolute targets: executed' \
  'security6 exec gate links: a writable directory on the way: executed' \
  'security6 exec gate links: a directory moved during the walk: executed'
# A directory moved under the shipped helper's walk, held under `/usr/bin/strace` where its
# open returns: absent, so refused here, without `strace` or where it may not trace.
require_executed -p b10x-substrate-exec-gate security7_gate \
  'security7 exec gate walk: no `..` asked of the kernel: executed'
require_executed -p b10x-substrate-exec-gate adversary_gate \
  'adversary landlock failures: executed'
# The helper's expansion at its edges, with no mount and no namespace: the stop of the walk at
# its exact count, entries that are no regular file, a directory it cannot read, and a name
# that is not UTF-8.
require_executed -p b10x-substrate-exec-gate adversary4_gate \
  'adversary4 exec gate rule stop: executed' \
  'adversary4 exec gate special entries: executed' \
  'adversary4 exec gate unreadable expanded directory: executed' \
  'adversary4 exec gate non-UTF-8 name: executed'
require_executed -p b10x-substrate-exec-gate security2_gate \
  'security2 exec gate refusals: executed'
require_executed -p b10x-substrate-exec-gate security_gate \
  'security exec gate landlock refused: executed' \
  'security exec gate preload: executed' \
  'security exec gate option-like operands: executed' \
  'security exec gate long list: executed' \
  'security exec gate no_new_privs: executed'

# `b10x-substrate confine run` opens the linux adapter on a private group beneath the delegated
# root; its delegated cases are absent, exit 0, without it, so each is required by its line.
require_executed -p b10x-substrate-cli confine \
  'confine delegated dry run render: executed' 'confine delegated private group: executed' \
  'confine delegated reaps a killed group: executed' \
  'confine delegated keeps a populated dead group: executed' \
  'confine delegated names a populated dead group: executed' \
  'confine delegated names a populated dead group in json: executed' \
  'confine delegated reaps a killed, unreaped group: executed' \
  "confine delegated leaves a live owner's empty group: executed"
require_executed -p b10x-substrate-cli confine_adversary \
  'confine adversary delegated: executed, two runs on one root' \
  'confine adversary delegated: executed, stderr' \
  'confine adversary delegated: executed, json envelope' \
  'confine adversary delegated: executed, relative ro' \
  'confine adversary delegated: executed, linux run' \
  'confine adversary delegated: executed, baseline variables'
require_executed -p b10x-substrate-cli confine_security_review \
  'security review confine concurrent cgroup root: executed' \
  'security review confine json stderr: executed' \
  'security review confine linux env: executed' \
  'security review confine ro and argv: executed' \
  'security review confine reaper zombie owner: executed' \
  'security review confine reaper json refusal: executed' \
  'security review confine reaper json interrupt: executed'

# The confinement module as its consumers call it: every need chosen by facts and run on an
# adapter not declared insecure. Absent, exit 0, without the delegated root, so required by count.
consumer_log="$(cargo test -p b10x-substrate-host --locked --test consumer_surface \
  -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${consumer_log}" >&2; exit 1; }
printf '%s\n' "${consumer_log}"
consumer_line="$(printf '%s\n' "${consumer_log}" \
  | grep -oE 'consumer surface delegated lane: [1-9][0-9]* needs executed of [1-9][0-9]*' || true)"
if [[ -z "${consumer_line}" ]]; then
  echo "delegated-lane: the consumer surface needs did not execute" >&2
  exit 1
fi
echo "delegated-lane: ${consumer_line}"
# Design 36's acceptance: one run held to an allow-list and a deny-list that hits its process
# bound and then its timeout, read through public items only. Absent, exit 0, without the root
# or the helper, so required by its own line.
if ! grep -qF 'consumer surface governed run: executed' <<<"${consumer_log}"; then
  echo "delegated-lane: consumer_surface did not print 'consumer surface governed run: executed'" >&2
  exit 1
fi

# The public SDK owns a separate clean-room journey against the shipped daemon binary. It proves
# PTY resize, live metrics and orderly whole-tree cleanup through SDK types only; without this
# explicit command that test would be absent from the daemon-only lane below.
#
# The binary is wherever cargo put it, and only cargo knows where that is. `CARGO_TARGET_DIR`,
# `CARGO_BUILD_TARGET_DIR`, a `build.target-dir` in any Cargo config file up the directory tree or
# in `CARGO_HOME`, and `--config` all move the build directory; a path guessed here from `${PWD}`
# or from one of those variables was, under any of the others, a path nothing had ever written, so
# both SDK cases below failed with `Startup("No such file or directory (os error 2)")` — which
# reads as a daemon defect rather than a missing build artefact. So no path is derived here: the
# build itself reports the `executable` it produced (`--message-format=json-render-diagnostics`
# keeps the human diagnostics on stderr), and the script refuses, by name, if it reported none.
daemon_bin="$(
  cargo build -p b10x-substrate-daemon --bin substrate-daemon --locked \
    --message-format=json-render-diagnostics \
    | jq -r 'select(.reason == "compiler-artifact" and .target.name == "substrate-daemon"
                    and .executable != null) | .executable'
)"
if [[ -z "${daemon_bin}" || "${daemon_bin}" == *$'\n'* || ! -x "${daemon_bin}" ]]; then
  echo "delegated-lane: cargo reported no single executable for substrate-daemon (got: '${daemon_bin}')" >&2
  exit 1
fi
echo "delegated-lane: shipped daemon ${daemon_bin}"

# Required by its line: a writable-root or working-directory case that reported itself absent
# would exit 0 too.
SUBSTRATE_TEST_DAEMON="${daemon_bin}" require_executed -p b10x-substrate-sdk managed \
  'sdk writable root delegated: executed' \
  'sdk workdir delegated: executed'

# The CLI's retried `session signal` signals a live session, retires it and signals again under
# the same operation id. Its retried `exec run` runs a program, destroys the workspace, runs
# again under the same operation id, and is refused once the exec is retired. Both need confined
# exec, so each case is absent, exit 0, without the delegated root, and is required by its line.
# Two more cases need it: a replayed `exec run` keeps its exit status, its truncation flags and
# a timeout, and the limit case dispatches one exec before the daemon is started again.
SUBSTRATE_TEST_DAEMON="${daemon_bin}" require_executed -p b10x-substrate-cli scenarios \
  'cli session signal replay: executed' \
  'cli exec run replay: executed' \
  'adversary exec run replay fidelity: executed' \
  'limit exec run under another capability snapshot: executed, 1 exec dispatched'

SUBSTRATE_TEST_DAEMON="${daemon_bin}" \
  cargo test -p b10x-substrate-sdk --test unrecorded --locked -- --nocapture --test-threads=1

# The remote SDK journey reuses the shipped daemon only after the managed instance has stopped. It
# proves that the same typed session path crosses TLS 1.3 and WSS with a channel-bound, one-use
# authority, then attempts a reconnect and observes the fresh-mint refusal rather than replaying
# the redeemed authority.
SUBSTRATE_TEST_DAEMON="${daemon_bin}" \
  cargo test -p b10x-substrate-sdk --test remote --locked -- --nocapture --test-threads=1

# The clean-room runner (design 31) certifies the same daemon over HTTPS from outside it; its
# delegated scenario is absent, exit 0, without the root, so each group is required by its line.
SUBSTRATE_TEST_DAEMON="${daemon_bin}" require_executed -p b10x-substrate-conformance local_endpoint \
  'conformance delegated exec: executed' \
  'conformance delegated resources: executed' \
  'conformance delegated sessions: executed' \
  'conformance delegated restart exec: executed'

# The MCP adapter owns the same kind of exclusive delegated root, but runs after the SDK daemon has
# fully stopped. Its shipped-binary journey executes sha256sum, reads exact output and metrics, then
# leaves an active workload for EOF cleanup and proves both its pid and exact cgroup absent.
SUBSTRATE_MCP_CGROUP_ROOT="${root}" \
  cargo test -p b10x-substrate-mcp --test stdio --locked -- --nocapture --test-threads=1

# The hosted WSS authority journey uses the same delegated root to start a real confined pipe
# session, round-trip bytes through its TLS-bound attachment, and then prove one-use replay. Its
# portable form seeds only the durable preflight state, so this explicit invocation is the evidence
# that the network surface reaches a real sandbox rather than merely completing a handshake.
cargo test -p b10x-substrate-daemon --test tls_listener \
  hosted_wss_attachment_authority_is_one_use_and_channel_bound --locked -- --nocapture

# The host-process cases (ADR 0036) start real process trees in cgroups below the same root: start,
# whole-tree stop, leases, keeps, deadlines and restart reattachment. Without this line they are
# absent from every lane, and a delegated-only regression in them went unseen.
cargo test -p b10x-substrate-daemon --test processes --locked -- --nocapture --test-threads=1

# The clean-room runner's delegated lane spans several test functions of this one binary, each
# with a daemon of its own and its own asserted count. Each prints one line when its delegated
# cases passed, naming how many it ran and the lane's whole inventory (`DELEGATED_CASES`). A
# function that a filter left out, or that ran the portable lane, prints no such line and the
# binary still exits 0, so the lines are summed and a run that did not execute the whole
# inventory fails here. A filter passed to this script therefore ends in that refusal: it shows
# one function's output and is never a lane run.
#
# The executable lists on the wire (design 37): the runner starts the daemon of its function
# `runtime_clean_room_holds_runs_to_executable_lists` with `--exec-gate` naming the static helper
# exported above as `SUBSTRATE_EXEC_GATE`, and the case that a dynamically linked helper stops
# the daemon before it listens reads `SUBSTRATE_EXEC_GATE_DYNAMIC`. Each of those cases prints
# its own `executed` line, or an `ABSENT (` line where the helper is unset or the open withheld
# a fact, and the function then reports no delegated count; so the lines are required by name
# and no `ABSENT` line is taken.
vectors_log="$(cargo test -p b10x-substrate-daemon --test runtime_vectors \
  -- --nocapture "$@" 2>&1)" || { printf '%s\n' "${vectors_log}" >&2; exit 1; }
printf '%s\n' "${vectors_log}"
if grep -qF ': ABSENT (' <<<"${vectors_log}"; then
  echo "delegated-lane: runtime_vectors reported a case absent" >&2
  exit 1
fi
for line in \
  'runtime exec lists unserved without a helper: executed' \
  'runtime exec lists facts: executed' \
  'runtime exec lists exec start: executed' \
  'runtime exec lists session start: executed' \
  'runtime exec lists start without the members: executed' \
  'runtime exec lists pty session: executed' \
  'runtime exec lists entry unrepresentable: executed' \
  'runtime exec lists entry undeclared: executed' \
  'runtime exec lists capsule start: executed' \
  'runtime exec lists program unlisted: executed' \
  'runtime exec lists shape: executed' \
  'runtime exec lists git root: executed' \
  'runtime exec lists dynamic helper: executed'; do
  if ! grep -qF "${line}" <<<"${vectors_log}"; then
    echo "delegated-lane: runtime_vectors did not print '${line}'" >&2
    exit 1
  fi
done
vectors_counted="$(printf '%s\n' "${vectors_log}" \
  | { grep -oE 'passed \(delegated lane\): [0-9]+ of [0-9]+ cases' || true; } \
  | awk '{ ran += $4; inventory = $6 } END { print ran + 0, inventory + 0 }')"
vectors_ran="${vectors_counted% *}"
vectors_inventory="${vectors_counted#* }"
if [[ "${vectors_inventory}" == 0 || "${vectors_ran}" != "${vectors_inventory}" ]]; then
  echo "delegated-lane: the runtime vectors executed ${vectors_ran} delegated cases of an inventory of ${vectors_inventory} (0: no function reported its delegated cases)" >&2
  exit 1
fi
# The inventory the functions printed is the one the source declares, which is the count
# `release.yml` reads for a `Confinement-lane:` record.
vectors_source="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/crates/substrate-daemon/tests/runtime_vectors.rs"
vectors_declared="$(sed -n 's/^const DELEGATED_CASES: usize = \([0-9][0-9]*\);$/\1/p' \
  "${vectors_source}")"
if [[ "${vectors_inventory}" != "${vectors_declared}" ]]; then
  echo "delegated-lane: the runtime vectors reported an inventory of ${vectors_inventory}; the source declares DELEGATED_CASES = '${vectors_declared}'" >&2
  exit 1
fi
echo "delegated-lane: the runtime vectors executed ${vectors_ran} of ${vectors_inventory} delegated cases"

# What this lane did *not* run. A green run above covers the delegated cgroup cases and nothing
# about project quotas: the real-quota cases need an ext4/XFS filesystem mounted with project
# quotas and an exclusive identity range, which no step here provisions. Saying so is the same
# discipline invariant 3 applies to the delegated cases themselves — an absent lane is not a green
# one, and the only way to tell them apart is to name what was skipped.
cargo xtask quota-lane --inventory
