#!/usr/bin/env bash
# Mint one short-lived GitHub App installation token for b10x-bot.
set -euo pipefail

config_root="${XDG_CONFIG_HOME:-${HOME}/.config}/b10x"
config="${B10X_BOT_CONFIG:-${config_root}/b10x-bot.json}"
key="${B10X_BOT_KEY:-${config_root}/b10x-bot.private-key.pem}"
org="${B10X_BOT_ORG:-beyond10x}"

# Each credential file is there, is a regular file and no symbolic link, belongs to the
# effective user and opens for reading; the configuration is not writable by group or other
# (mask 022) and the key grants group and other nothing (mask 077). Both files are checked and
# every refusal is printed before either is read, without a byte or a path of either. POSIX
# `find` tests only: `find` does not follow a symbolic link it is given, and a test that cannot
# run prints nothing, which refuses.
refusals=0
refuse() {
  printf 'b10x-bot credential input %s\n' "$1" >&2
  refusals=$((refusals + 1))
}

check_credential_file() {
  local file="$1" message="$2" bit
  shift 2
  case "$file" in
    /*) ;;
    *) file="./${file}" ;;
  esac
  if [ ! -e "$file" ] && [ ! -L "$file" ]; then
    refuse "is unavailable"
    return
  fi
  if [ -z "$(find "$file" -prune -type f -user "$(id -u)" 2>/dev/null || :)" ]; then
    refuse "must be a current-user-owned regular file"
    return
  fi
  local safe=()
  for bit in "$@"; do
    safe+=(! -perm "-${bit}")
  done
  if [ -z "$(find "$file" -prune "${safe[@]}" 2>/dev/null || :)" ]; then
    refuse "$message"
  fi
  if ! { : <"$file"; } 2>/dev/null; then
    refuse "is not readable"
  fi
}

check_credential_file "$config" "must be not group/world-writable" 020 002
check_credential_file "$key" "must be owner-only" 040 020 010 004 002 001
if [ "$refusals" -ne 0 ]; then
  exit 1
fi

app_id="${B10X_BOT_APP_ID:-$(jq -er '.app_id' "$config")}"

b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

now="$(date +%s)"
header="$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)"
payload="$(printf '{"iat":%d,"exp":%d,"iss":"%s"}' "$((now - 60))" "$((now + 540))" "$app_id" | b64url)"
signature="$(printf '%s.%s' "$header" "$payload" | openssl dgst -sha256 -sign "$key" -binary | b64url)"
jwt="${header}.${payload}.${signature}"

api() {
  curl -fsS \
    -H "Authorization: Bearer ${jwt}" \
    -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "$@"
}

installation_id="$(
  api https://api.github.com/app/installations \
    | jq -er --arg org "$org" '.[] | select(.account.login == $org) | .id' \
    | head -1
)"

api -X POST "https://api.github.com/app/installations/${installation_id}/access_tokens" \
  | jq -er '.token'
