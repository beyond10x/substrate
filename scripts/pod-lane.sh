#!/usr/bin/env bash
# Runs the `pod` confinement lane: the ESS pod scenarios and the pod adversary and security-review
# cases, against the cluster the environment names (design 28).
#
#     SUBSTRATE_POD_LANE=1 \
#     SUBSTRATE_POD_KUBECONFIG=<kubeconfig.json> \
#     SUBSTRATE_POD_KUBECONFIG_NOLOG=<kubeconfig.json without pods/log> \
#     SUBSTRATE_POD_KIND_NODE=<kind node container> \
#       bash scripts/pod-lane.sh [<extra cargo test arguments>...]
#
# The gate workflow's `pod-lane` job runs it against a disposable `kind` cluster. Every lane case
# is absent, exit 0, without its variable, and a test filter that matches nothing exits 0 too, so
# each suite is required by the harness's own `test result:` line, with at least one case passed,
# and by the absence of any `ABSENT` line. Extra arguments go to every suite's test harness.
set -euo pipefail

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
# `ABSENT` line, or a summary that is missing or reports no case passed.
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
  if printf '%s\n' "${log}" | grep -q 'ABSENT'; then
    echo "pod-lane: ${binary} reported a case absent" >&2
    exit 1
  fi
  local summary
  summary="$(printf '%s\n' "${log}" | grep -E '^test result: ' || true)"
  if [[ "$(printf '%s\n' "${summary}" | grep -c .)" -ne 1 ]]; then
    echo "pod-lane: ${binary} printed no single 'test result:' line" >&2
    exit 1
  fi
  if ! printf '%s\n' "${summary}" | grep -qE '^test result: ok\. [1-9][0-9]* passed; 0 failed;'; then
    echo "pod-lane: ${binary} selected no case: ${summary}" >&2
    exit 1
  fi
  echo "pod-lane: ${binary}: ${summary}"
}

require_suite ess_confinement_pod "$@"
# One thread: the kill case stops the node's kubelet, which would stall any other run meanwhile.
require_suite adversary_pod --test-threads=1 "$@"
require_suite security_review_pod "$@"
