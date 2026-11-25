#!/bin/sh
# Measurement for story an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner,
# step 1. Not a test and not a gate step: it runs once, from a branch that is not merged.
#
# It repeats the shape of the gate's case an_oci_run_over_its_memory_bound_names_the_memory_limit
# with the runtime's own client: the arguments the oci adapter passes to `create`, then
# `start --attach`, then reads of the container's state. Per run it records
#   - the client's exit status,
#   - whether the first read of the state holds OOMKilled, and otherwise how many milliseconds
#     after the client returned a read first held it, up to WAIT_MS,
#   - whether the runtime sent an `oom` event for the container,
#   - whether the kernel log names the container's cgroup in an OOM kill,
#   - in phase D, whether a reader on the host saw `oom_kill` above zero in the container's
#     `memory.events` before the file was gone,
#   - in phase E, where the growing process is a child of the container's command, whether that
#     command read `oom_kill` above zero in its own cgroup after the child was killed.
#
# Usage: measure-oci-oom-flag.sh <image> <out dir>
set -u

IMAGE=$1
OUT=$2
WAIT_MS=5000
POLL_S=0.05
A_RUNS=${A_RUNS:-60}
B_RUNS=${B_RUNS:-150}
C_RUNS=${C_RUNS:-50}
D_RUNS=${D_RUNS:-100}
E_RUNS=${E_RUNS:-100}
LOAD=${LOAD:-8}
SCRIPT='grow=x; while : ; do grow="$grow$grow"; done'
# Phase E: the same loop in a child; the command prints the child's status and its cgroup's count.
SCRIPT_INSIDE="sh -c '$SCRIPT'; echo \"inside \$? \$(grep '^oom_kill ' /sys/fs/cgroup/memory.events)\""

mkdir -p "$OUT"
WS=$(mktemp -d)
chmod 755 "$WS"
UIDGID="$(id -u):$(id -g)"

now_ms() { echo $(( $(date +%s%N) / 1000000 )); }

# create_run <name> <script>: prints the container id.
create_run() {
    docker create --name "$1" --label substrate.measure=oom-flag \
        --pull never --log-driver none --init \
        --user "$UIDGID" --network none --ipc none --cap-drop ALL \
        --security-opt no-new-privileges --read-only --tmpfs /tmp \
        --pids-limit 8 --memory 16777216 --memory-swap 16777216 --cpus 2.000 \
        --mount "type=bind,source=$WS,target=/workspace" -w /workspace \
        -e PATH=/usr/bin:/bin -e HOME=/tmp -e LANG=C.UTF-8 -e LC_ALL=C.UTF-8 -e TZ=UTC \
        --entrypoint "" -- "$IMAGE" /bin/sh -c "$2"
}

# read_state <name>: prints "<OOMKilled> <ExitCode> <Status>".
read_state() {
    docker inspect --format '{{.State.OOMKilled}} {{.State.ExitCode}} {{.State.Status}}' "$1" 2>/dev/null
}

# cgroup_reader <id> <result file>: reads the container's memory.events until the file is gone
# and writes the largest oom_kill it saw, or "unseen" when it never read the file.
cgroup_reader() {
    # The systemd cgroup driver's place, then the cgroupfs driver's.
    file=/sys/fs/cgroup/system.slice/docker-$1.scope/memory.events
    other=/sys/fs/cgroup/docker/$1/memory.events
    seen=unseen
    tries=0
    while [ ! -r "$file" ] && [ "$tries" -lt 400 ]; do
        if [ -r "$other" ]; then file=$other; break; fi
        tries=$((tries + 1))
        sleep 0.005
    done
    while value=$(awk '$1 == "oom_kill" { print $2 }' "$file" 2>/dev/null) && [ -n "$value" ]; do
        seen=$value
    done
    echo "$seen" > "$2"
}

# one_run <phase> <index> <reader: 0 none | 1 on the host | 2 the container's command>
one_run() {
    name="m-$1-$2"
    script=$SCRIPT
    wait_ms=$WAIT_MS
    if [ "$3" = 2 ]; then script=$SCRIPT_INSIDE; wait_ms=500; fi
    id=$(create_run "$name" "$script") || { echo "$1 $2 create-failed" >> "$OUT/runs.txt"; return; }
    reader=-
    if [ "$3" = 1 ]; then
        cgroup_reader "$id" "$OUT/reader-$name" &
        reader_pid=$!
    fi
    started=$(now_ms)
    docker start --attach "$name" > "$OUT/stdout-$name" 2> /dev/null
    status=$?
    returned=$(now_ms)
    state=$(read_state "$name")
    first=${state%% *}
    after=never
    if [ "$first" = true ]; then
        after=0
    else
        while [ $(( $(now_ms) - returned )) -lt "$wait_ms" ]; do
            sleep "$POLL_S"
            state=$(read_state "$name")
            if [ "${state%% *}" = true ]; then
                after=$(( $(now_ms) - returned ))
                break
            fi
        done
    fi
    if [ "$3" = 1 ]; then
        wait "$reader_pid"
        reader=$(cat "$OUT/reader-$name")
        rm -f "$OUT/reader-$name"
    fi
    if [ "$3" = 2 ]; then
        # "inside <child status> oom_kill <count>"
        reader=$(awk '$1 == "inside" && $3 == "oom_kill" { print $4 "/child-" $2 }' "$OUT/stdout-$name")
        reader=${reader:-unseen}
    fi
    rm -f "$OUT/stdout-$name"
    docker rm --force "$name" > /dev/null 2>&1
    # phase index id client-status run-ms first-read ms-to-flag last-state reader
    echo "$1 $2 $id $status $((returned - started)) ${first:-unread} $after $(echo "${state:-unread}" | tr ' ' '/') $reader" >> "$OUT/runs.txt"
}

# worker <phase> <first index> <count> <reader>
worker() {
    index=$2
    last=$(( $2 + $3 ))
    while [ "$index" -lt "$last" ]; do
        one_run "$1" "$index" "$4"
        index=$((index + 1))
    done
}

spinners=""
start_load() {
    count=0
    while [ "$count" -lt "$1" ]; do
        sh -c 'while : ; do : ; done' &
        spinners="$spinners $!"
        count=$((count + 1))
    done
}
stop_load() {
    for pid in $spinners; do kill "$pid" 2>/dev/null; done
    spinners=""
}

echo "== host"
uname -r
nproc
docker version --format 'client {{.Client.Version}} server {{.Server.Version}}'
docker info --format 'cgroup driver {{.CgroupDriver}} v{{.CgroupVersion}}, runtime {{.DefaultRuntime}}, init {{.InitBinary}}, containerd {{.ContainerdCommit.ID}}, runc {{.RuncCommit.ID}}'
stat -fc 'cgroup file system %T' /sys/fs/cgroup

: > "$OUT/runs.txt"
docker events --filter type=container --filter event=oom \
    --format '{{.Actor.Attributes.name}}' > "$OUT/events.txt" 2>/dev/null &
events_pid=$!
sleep 1

echo "== phase A: one at a time, no load"
worker A 0 "$A_RUNS" 0

echo "== phase B: one at a time, $LOAD busy loops"
start_load "$LOAD"
worker B 0 "$B_RUNS" 0

echo "== phase C: four at a time, $LOAD busy loops"
worker C 0 "$C_RUNS" 0 &
w1=$!
worker C "$C_RUNS" "$C_RUNS" 0 &
w2=$!
worker C $((C_RUNS * 2)) "$C_RUNS" 0 &
w3=$!
worker C $((C_RUNS * 3)) "$C_RUNS" 0 &
w4=$!
wait "$w1" "$w2" "$w3" "$w4"

echo "== phase D: one at a time, $LOAD busy loops, a host reader of memory.events"
worker D 0 "$D_RUNS" 1

echo "== phase E: one at a time, $LOAD busy loops, the growing process is a child of the container's command"
worker E 0 "$E_RUNS" 2
stop_load

sleep 6
kill "$events_pid" 2>/dev/null
sudo journalctl -k --no-pager -b 2>/dev/null | grep -o 'oom_memcg=[^,]*' \
    | grep -o '[0-9a-f]\{64\}' | sort -u > "$OUT/kernel.txt"
echo "kernel log: $(wc -l < "$OUT/kernel.txt") container cgroups named in an OOM kill, $(sudo journalctl -k --no-pager -b 2>/dev/null | grep -c 'Memory cgroup out of memory: Killed process') kill lines (the report that names the cgroup is rate-limited by the kernel, the kill line is not)"
echo "runtime: $(sort -u "$OUT/events.txt" | wc -l) containers with an oom event"

echo "== per phase"
for phase in A B C D E; do
    awk -v phase="$phase" -v events="$OUT/events.txt" -v kernel="$OUT/kernel.txt" '
        BEGIN {
            while ((getline line < events) > 0) event[line] = 1
            while ((getline line < kernel) > 0) killed[line] = 1
        }
        $1 == phase && $3 == "create-failed" { failed++ }
        $1 == phase && $3 != "create-failed" {
            runs++
            name = "m-" $1 "-" $2
            if ($4 == 137) status137++
            if ($3 in killed) kernel_named++
            if (name in event) with_event++
            if ($7 == 0) at_first++
            else if ($7 == "never") {
                never++
                if ($3 in killed) never_killed++
                if (name in event) never_event++
                if (phase != "E") print "  no flag: " $0 " kernel=" (($3 in killed) ? "named" : "not-named") " event=" ((name in event) ? "sent" : "none")
            } else {
                late++
                if (late == 1 || $7 < least) least = $7
                if ($7 > most) most = $7
                print "  late flag: " $0
            }
            if ($9 != "-") { readers++; if ($9 != "unseen" && $9 + 0 > 0) reader_saw++; if ($9 == "unseen") reader_unseen++ }
        }
        END {
            printf "phase %s: %d runs, %d create failed, %d with client status 137, %d named by the kernel log, %d with an oom event\n", phase, runs, failed, status137, kernel_named, with_event
            printf "phase %s: flag at the first read %d, later %d (%s to %s ms), none within the wait %d (of them: kernel named %d, oom event %d)\n", phase, at_first, late, (late ? least : "-"), (late ? most : "-"), never, never_killed, never_event
            if (readers) printf "phase %s: %s read oom_kill above zero in %d of %d runs, read nothing in %d\n", phase, (phase == "E" ? "the container command" : "the host reader"), reader_saw, readers, reader_unseen
        }' "$OUT/runs.txt"
done
echo "== run times, client start to return, ms (least median most)"
awk '$3 != "create-failed" { print $5 }' "$OUT/runs.txt" | sort -n | awk '{ v[NR] = $1 } END { if (NR) print v[1], v[int((NR + 1) / 2)], v[NR] }'
rmdir "$WS" 2>/dev/null
exit 0
