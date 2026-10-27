#!/usr/bin/env bash
# Runs the `pod` confinement lane: the ESS pod scenarios and the pod adversary cases, against the
# cluster the environment names (design 28).
#
#     SUBSTRATE_POD_LANE=1 \
#     SUBSTRATE_POD_KUBECONFIG=<kubeconfig.json> \
#     SUBSTRATE_POD_KUBECONFIG_NOLOG=<kubeconfig.json without pods/log> \
#     SUBSTRATE_POD_KIND_NODE=<kind node container> \
#       bash scripts/pod-lane.sh
#
# The gate workflow's `pod-lane` job runs it against a disposable `kind` cluster. Every lane case
# is absent, exit 0, without its variable, and an ignored or filtered-out case exits 0 too, so the
# script takes no arguments and requires each suite by the harness's own `test result:` line: at
# least one case passed, none failed, ignored or filtered out, and no `ABSENT` line. The pod
# security-review cases read no lane variable and run in the workspace gate, not here.
set -euo pipefail

if [[ "$#" -ne 0 ]]; then
  echo "pod-lane: takes no arguments; a filter or harness flag could deselect a lane case: $*" >&2
  exit 1
fi
for variable in SUBSTRATE_POD_LANE SUBSTRATE_POD_KUBECONFIG SUBSTRATE_POD_KUBECONFIG_NOLOG \
  SUBSTRATE_POD_KIND_NODE; do
  if [[ -z "${!variable:-}" ]]; then
    echo "pod-lane: ${variable} is unset; the lane would be absent" >&2
    exit 1
  fi
done
if [[ "${SUBSTRATE_POD_LANE}" != "1" ]]; then
  echo "pod-lane: SUBSTRATE_POD_LANE is '${SUBSTRATE_POD_LANE}', not 1" >&2
  exit 1
fi

# require_suite <test binary> [<harness argument>...]: runs one suite and refuses a failed run, an
# `ABSENT` line, or a summary that is missing, reports no case passed, or reports any case failed,
# ignored, measured or filtered out.
require_suite() {
  local binary="$1"
  shift
  local log status=0
  log="$(cargo test -p b10x-substrate-host --release --locked --test "${binary}" \
    -- --nocapture "$@" 2>&1)" || status=$?
  printf '%s\n' "${log}"
  if [[ "${status}" -ne 0 ]]; then
    echo "pod-lane: ${binary} failed, exit ${status}" >&2
    exit 1
  fi
  if grep -q 'ABSENT' <<<"${log}"; then
    echo "pod-lane: ${binary} reported a case absent" >&2
    exit 1
  fi
  local summary
  summary="$(grep -E '^test result: ' <<<"${log}" || true)"
  if [[ "$(grep -c . <<<"${summary}")" -ne 1 ]]; then
    echo "pod-lane: ${binary} printed no single 'test result:' line" >&2
    exit 1
  fi
  if ! grep -qE '^test result: ok\. [1-9][0-9]* passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;' \
    <<<"${summary}"; then
    echo "pod-lane: ${binary} did not run every case: ${summary}" >&2
    exit 1
  fi
  echo "pod-lane: ${binary}: ${summary}"
}

require_suite ess_confinement_pod
# One thread: the kill case stops the node's kubelet, which would stall any other run meanwhile.
# Its two manual `cleanup_pass1` cases print a skip line and return unless
# SUBSTRATE_POD_MANUAL_CLEANUP=1, which the gate workflow's `pod-lane` job does not set.
require_suite adversary_pod --test-threads=1
