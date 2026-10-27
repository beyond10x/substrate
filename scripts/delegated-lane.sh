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
# own `executed` line and no `ABSENT` line, so a case that did not run fails here.
require_executed() {
  local binary="$1"
  shift
  local log
  log="$(cargo test -p b10x-substrate-host --locked --test "${binary}" \
    -- --nocapture --test-threads=1 2>&1)" || { printf '%s\n' "${log}" >&2; exit 1; }
  printf '%s\n' "${log}"
  if printf '%s\n' "${log}" | grep -q 'ABSENT'; then
    echo "delegated-lane: ${binary} reported a case absent" >&2
    exit 1
  fi
  local line
  for line in "$@"; do
    if ! printf '%s\n' "${log}" | grep -qF "${line}"; then
      echo "delegated-lane: ${binary} did not print '${line}'" >&2
      exit 1
    fi
  done
  echo "delegated-lane: ${binary} executed its delegated cases"
}
require_executed adversary_confinement_s0_delegated \
  'adversary writable root: executed' 'adversary render parity: executed'
require_executed security_review_confinement 'security review linux start env: executed'

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
if ! command -v jq >/dev/null 2>&1; then
  echo "delegated-lane: jq is required to read the daemon path cargo reports" >&2
  exit 1
fi
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

SUBSTRATE_TEST_DAEMON="${daemon_bin}" \
  cargo test -p b10x-substrate-sdk --test managed --locked -- --nocapture --test-threads=1

SUBSTRATE_TEST_DAEMON="${daemon_bin}" \
  cargo test -p b10x-substrate-sdk --test unrecorded --locked -- --nocapture --test-threads=1

# The remote SDK journey reuses the shipped daemon only after the managed instance has stopped. It
# proves that the same typed session path crosses TLS 1.3 and WSS with a channel-bound, one-use
# authority, then attempts a reconnect and observes the fresh-mint refusal rather than replaying
# the redeemed authority.
SUBSTRATE_TEST_DAEMON="${daemon_bin}" \
  cargo test -p b10x-substrate-sdk --test remote --locked -- --nocapture --test-threads=1

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

cargo test -p b10x-substrate-daemon --test runtime_vectors -- --nocapture "$@"

# What this lane did *not* run. A green run above covers the delegated cgroup cases and nothing
# about project quotas: the real-quota cases need an ext4/XFS filesystem mounted with project
# quotas and an exclusive identity range, which no step here provisions. Saying so is the same
# discipline invariant 3 applies to the delegated cases themselves — an absent lane is not a green
# one, and the only way to tell them apart is to name what was skipped.
cargo xtask quota-lane --inventory
