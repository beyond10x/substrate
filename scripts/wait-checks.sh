#!/bin/sh
# Wait for the checks of a pull request or of a commit, print each with its outcome, and exit by
# what they came to.
#
#   scripts/wait-checks.sh [options] <pull-request-number>
#   scripts/wait-checks.sh [options] --commit <sha>
#   scripts/wait-checks.sh --from <file>|-
#
# A pull request's checks are read with `gh pr checks <n> --json name,state,bucket`, a commit's
# check runs with `gh api repos/{owner}/{repo}/commits/<sha>/check-runs`. The script asks again
# every `--interval` seconds (default 30) until no check is pending, for at most `--timeout`
# seconds (default 3600) of wall time. `--repo <owner/name>` names the repository; without it
# `gh` takes the one of the current directory. `--from` decides once, on a list read from a file
# or from standard input in either form, and waits for nothing. A number is decimal digits, at
# most nine after its leading zeros; a sha is 7 to 40 lower-case hex digits.
#
# A check's outcome is its `state` in the pull-request form and its `conclusion` in the commit
# form, read without regard to letter case:
#   success, skipped, neutral                                   succeeded
#   failure, error, cancelled, timed_out, action_required,
#   startup_failure, stale                                      did not succeed
#   anything else                                               pending, except that a
#                                                               completed check run did not
#                                                               succeed
#
# Exit status:
#   0  every check succeeded, or was skipped or neutral
#   1  a check did not succeed
#   2  a usage error, a list that is not one of the two forms, or a `gh` that failed on three
#      asks in a row or on the last ask before `--timeout`; its last error line is printed
#   3  no check is reported, or a check is still pending: at the end of `--timeout`, or in the
#      list `--from` read
#
# Reads only; needs `gh` for waiting and `jq` for every form. Starts no build.
set -eu

usage() {
  printf '%s\n' \
    'usage: wait-checks.sh [--repo <owner/name>] [--interval <s>] [--timeout <s>] <pr-number>' \
    '       wait-checks.sh [--repo <owner/name>] [--interval <s>] [--timeout <s>] --commit <sha>' \
    '       wait-checks.sh --from <file>|-' >&2
  exit 2
}

# Sets `value` to the decimal number `$1` names, without its leading zeros, which shell
# arithmetic would read as octal; refuses anything else and more than nine digits.
number() {
  case "$1" in
    '' | *[!0-9]*) usage ;;
  esac
  value=${1#"${1%%[!0]*}"}
  [ -n "$value" ] || value=0
  [ "${#value}" -le 9 ] || usage
}

interval=30
timeout=3600
mode=
target=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      [ "$#" -ge 2 ] || usage
      GH_REPO=$2
      export GH_REPO
      shift 2
      ;;
    --interval)
      [ "$#" -ge 2 ] || usage
      number "$2"
      interval=$value
      shift 2
      ;;
    --timeout)
      [ "$#" -ge 2 ] || usage
      number "$2"
      timeout=$value
      shift 2
      ;;
    --commit | --from)
      [ "$#" -ge 2 ] && [ -z "$mode" ] || usage
      mode=${1#--}
      target=$2
      shift 2
      ;;
    -*) usage ;;
    *)
      [ -z "$mode" ] || usage
      number "$1"
      mode=pr
      target=$value
      shift
      ;;
  esac
done
[ -n "$mode" ] || usage
if [ "$mode" = commit ]; then
  case "$target" in
    '' | *[!0-9a-f]*) usage ;;
  esac
  [ "${#target}" -ge 7 ] && [ "${#target}" -le 40 ] || usage
fi

# One line per check, `<class>	<outcome>	<name>`, from either form on standard input: the
# array `gh pr checks --json name,state,bucket` prints, or one or more check-run pages of the
# REST API (`{"check_runs": [...]}`, as `gh api --paginate` prints them). The class is `pass`,
# `fail` or `pending`, by the table of the header.
classify() {
  jq -r -s '
    def class:
      ascii_downcase
      | if IN("success", "skipped", "neutral") then "pass"
        elif IN("failure", "error", "cancelled", "timed_out", "action_required",
                "startup_failure", "stale") then "fail"
        else "pending" end;
    [ .[]
      | if type == "array" then .[]
        elif type == "object" and has("check_runs") then .check_runs[]
        else error("not a check list") end ]
    | .[]
    | if type == "object" and has("bucket") and (.state | type) == "string" then
        { name, outcome: (.state | ascii_downcase), class: (.state | class) }
      elif type == "object" and has("status") then
        if .status != "completed" then
          { name, outcome: .status, class: "pending" }
        else
          ((.conclusion // "none") | tostring | ascii_downcase) as $conclusion
          | { name,
              outcome: $conclusion,
              class: ($conclusion | class | if . == "pending" then "fail" else . end) }
        end
      else error("not a check") end
    | "\(.class)\t\(.outcome)\t\(.name)"'
}

# How many lines of `$lines` are of class `$1`.
count() {
  printf '%s\n' "$lines" | awk -F '\t' -v class="$1" '$1 == class { n++ } END { print n + 0 }'
}

show() {
  printf '%s\n' "$lines" | awk -F '\t' 'NF >= 3 { printf "%-16s %s\n", $2, $3 }'
}

# The exit status the list `$lines` comes to, with a closing line on standard error.
decide() {
  passed=$(count pass)
  failed=$(count fail)
  pending=$(count pending)
  if [ $((passed + failed + pending)) -eq 0 ]; then
    printf 'wait-checks: no check is reported\n' >&2
    return 3
  fi
  if [ "$failed" -gt 0 ]; then
    printf 'wait-checks: %s of %s checks did not succeed\n' "$failed" \
      $((passed + failed + pending)) >&2
    return 1
  fi
  if [ "$pending" -gt 0 ]; then
    printf 'wait-checks: %s of %s checks are pending\n' "$pending" \
      $((passed + failed + pending)) >&2
    return 3
  fi
  printf 'wait-checks: all %s checks succeeded or were skipped\n' "$passed" >&2
  return 0
}

if [ "$mode" = from ]; then
  if [ "$target" = - ]; then
    lines=$(classify) || exit 2
  else
    [ -r "$target" ] || usage
    lines=$(classify < "$target") || exit 2
  fi
  show
  status=0
  decide || status=$?
  exit "$status"
fi

fetch() {
  if [ "$mode" = pr ]; then
    gh pr checks "$target" --json name,state,bucket
  else
    gh api --paginate "repos/{owner}/{repo}/commits/$target/check-runs?per_page=100"
  fi
}

errors=$(mktemp)
trap 'rm -f "$errors"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# gh's last error line, and the end of the wait with 2.
unread() {
  last=$(awk 'NF { line = $0 } END { print line }' "$errors")
  printf 'wait-checks: gh failed on the last %s of its asks: %s\n' "$failed_asks" \
    "${last:-no message}" >&2
  exit 2
}

started=$(date +%s)
slept=0
failed_asks=0
while :; do
  if json=$(fetch 2>"$errors"); then
    failed_asks=0
    lines=$(printf '%s\n' "$json" | classify) || exit 2
  elif grep -q 'no checks reported' "$errors"; then
    # gh's answer for a pull request whose checks have not started yet.
    failed_asks=0
    lines=
  else
    failed_asks=$((failed_asks + 1))
    lines=
    [ "$failed_asks" -lt 3 ] || unread
  fi
  if [ -n "$lines" ] && [ "$(count pending)" -eq 0 ]; then
    break
  fi
  # The wait is the wall time since the first ask, and never less than the intervals slept.
  waited=$(($(date +%s) - started))
  [ "$waited" -ge "$slept" ] || waited=$slept
  if [ "$waited" -ge "$timeout" ]; then
    [ "$failed_asks" -eq 0 ] || unread
    printf 'wait-checks: still waiting after %s s\n' "$waited" >&2
    break
  fi
  printf 'wait-checks: %s checks pending, asking again in %s s\n' \
    "$(count pending)" "$interval" >&2
  sleep "$interval"
  slept=$((slept + interval))
done
show
status=0
decide || status=$?
exit "$status"
