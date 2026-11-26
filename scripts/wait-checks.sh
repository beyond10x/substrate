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
# seconds (default 3600). `--repo <owner/name>` names the repository; without it `gh` takes the
# one of the current directory. `--from` decides once, on a list read from a file or from
# standard input in either form, and waits for nothing.
#
# Exit status:
#   0  every check succeeded, or was skipped or neutral
#   1  a check ended any other way: failure, cancelled, timed out, action required, stale
#   2  a usage error, or a list that is not one of the two forms
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

number() {
  case "$1" in
    '' | *[!0-9]*) usage ;;
  esac
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
      interval=$2
      shift 2
      ;;
    --timeout)
      [ "$#" -ge 2 ] || usage
      number "$2"
      timeout=$2
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
      target=$1
      shift
      ;;
  esac
done
[ -n "$mode" ] || usage
if [ "$mode" = commit ]; then
  case "$target" in
    '' | *[!0-9a-f]*) usage ;;
  esac
fi

# One line per check, `<class>	<outcome>	<name>`, from either form on standard input: the
# array `gh pr checks --json name,state,bucket` prints, or one or more check-run pages of the
# REST API (`{"check_runs": [...]}`, as `gh api --paginate` prints them). The class is `pass`,
# `fail` or `pending`; an outcome the script does not know is `fail`.
classify() {
  jq -r -s '
    [ .[]
      | if type == "array" then .[]
        elif type == "object" and has("check_runs") then .check_runs[]
        else error("not a check list") end ]
    | .[]
    | if type == "object" and has("bucket") then
        { name,
          outcome: ((.state // .bucket) | ascii_downcase),
          class: (if .bucket == "pass" or .bucket == "skipping" then "pass"
                  elif .bucket == "pending" then "pending"
                  else "fail" end) }
      elif type == "object" and has("status") then
        if .status != "completed" then
          { name, outcome: .status, class: "pending" }
        else
          { name,
            outcome: (.conclusion // "none"),
            class: (if .conclusion == "success" or .conclusion == "skipped"
                       or .conclusion == "neutral" then "pass" else "fail" end) }
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

waited=0
while :; do
  if json=$(fetch); then
    lines=$(printf '%s\n' "$json" | classify) || exit 2
  else
    lines=
  fi
  if [ -n "$lines" ] && [ "$(count pending)" -eq 0 ]; then
    break
  fi
  if [ "$waited" -ge "$timeout" ]; then
    printf 'wait-checks: still waiting after %s s\n' "$waited" >&2
    break
  fi
  printf 'wait-checks: %s checks pending, asking again in %s s\n' \
    "$(count pending)" "$interval" >&2
  sleep "$interval"
  waited=$((waited + interval))
done
show
status=0
decide || status=$?
exit "$status"
