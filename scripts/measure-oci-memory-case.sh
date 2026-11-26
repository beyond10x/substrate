#!/bin/sh
# One measurement for story an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner,
# the repeat its acceptance names: the oci lane's memory case, run through the changed adapter
# many times, counted by the end it read. Never merged; no gate step calls it.
#
# Usage: measure-oci-memory-case.sh <test binary of ess_confinement_oci> <output directory>
# Environment: A_RUNS (60, one at a time, no load), B_RUNS (150, one at a time beside LOAD busy
# loops), C_RUNS (50 per worker, four workers at a time beside LOAD busy loops), LOAD (8).
set -u
bin=$1
out=$2
mkdir -p "$out"
: "${A_RUNS:=60}" "${B_RUNS:=150}" "${C_RUNS:=50}" "${LOAD:=8}"
case_name=an_oci_run_over_its_memory_bound_names_the_memory_limit
runs="$out/runs.txt"
: > "$runs"

one() {
    phase=$1
    index=$2
    log="$out/run-$phase-$index.log"
    began=$(date +%s%N)
    SUBSTRATE_OCI_LANE=1 "$bin" --exact "$case_name" --nocapture --test-threads 1 > "$log" 2>&1
    status=$?
    ms=$(( ($(date +%s%N) - began) / 1000000 ))
    if grep -q 'held OOMKilled: Memory' "$log"; then
        end=memory
    elif grep -q 'held no OOMKilled: the kill is stated' "$log"; then
        end=stated
    else
        end=other
    fi
    ran=$(sed -n 's/.*the run ended after \([0-9.]*[a-zµ]*s\).*/\1/p' "$log" | head -n 1)
    echo "$phase $index $status $end ${ran:-none} $ms" >> "$runs"
    [ "$end" = other ] || rm -f "$log"
}

load_on() {
    pids=""
    i=0
    while [ "$i" -lt "$LOAD" ]; do
        sh -c 'while : ; do : ; done' &
        pids="$pids $!"
        i=$((i + 1))
    done
}

load_off() {
    # shellcheck disable=SC2086
    kill $pids 2>/dev/null
    wait 2>/dev/null
}

i=0
while [ "$i" -lt "$A_RUNS" ]; do one A "$i"; i=$((i + 1)); done

load_on
i=0
while [ "$i" -lt "$B_RUNS" ]; do one B "$i"; i=$((i + 1)); done
load_off

load_on
w=0
workers=""
while [ "$w" -lt 4 ]; do
    (
        i=0
        while [ "$i" -lt "$C_RUNS" ]; do one C "$w-$i"; i=$((i + 1)); done
    ) &
    workers="$workers $!"
    w=$((w + 1))
done
# shellcheck disable=SC2086
wait $workers
load_off

echo "phase end count"
awk '{print $1, $4}' "$runs" | sort | uniq -c | awk '{print $2, $3, $1}'
echo "all:"
awk '{print $4}' "$runs" | sort | uniq -c
others=$(awk '$4 == "other"' "$runs" | wc -l)
echo "runs with neither a named limit nor the stated kill: $others"
for log in "$out"/run-*.log; do
    [ -f "$log" ] || continue
    echo "== $log"
    tail -n 20 "$log"
done
