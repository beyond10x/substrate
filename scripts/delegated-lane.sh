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

exec cargo test -p b10x-substrate-daemon --test runtime_vectors -- --nocapture "$@"
