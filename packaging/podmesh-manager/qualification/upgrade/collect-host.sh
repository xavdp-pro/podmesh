#!/bin/bash
# Read-only inactive-upgrade collector. It never installs, starts or enables a service.
set -euo pipefail
export LC_ALL=C
umask 077

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
usage() {
  echo "Usage: $0 --host-alias <stable-alias> --stage <pre-upgrade|post-upgrade> --salt-file <private-salt> --candidate-verification <report.json> --output <evidence.json>" >&2
  exit 2
}

host_alias= stage= salt_file= candidate_verification= output=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --host-alias) host_alias=${2-}; shift 2 ;;
    --stage) stage=${2-}; shift 2 ;;
    --salt-file) salt_file=${2-}; shift 2 ;;
    --candidate-verification) candidate_verification=${2-}; shift 2 ;;
    --output) output=${2-}; shift 2 ;;
    *) usage ;;
  esac
done
case "$stage" in pre-upgrade|post-upgrade) ;; *) usage ;; esac
[ -n "$host_alias" ] && [ -n "$output" ] && [ -f "$salt_file" ] && [ -f "$candidate_verification" ] || usage
for command in jq sha256sum find sort stat readlink; do
  command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }
done

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

salted_commit_file() {
  local label=$1 path=$2
  { cat -- "$salt_file"; printf '\000%s\000' "$label"; cat -- "$path"; } |
    sha256sum | awk '{print "sha256:" $1}'
}

config_commitment() {
  local directory=${PODMESH_PATH_ROOT:-}/etc/podmesh-manager
  local path=$directory/config.json directory_metadata metadata
  [ -d "$directory" ] && [ ! -L "$directory" ] || {
    echo "Protected manager configuration directory must be a non-symlink directory for upgrade evidence" >&2
    return 1
  }
  [ -f "$path" ] && [ ! -L "$path" ] || {
    echo "Protected manager configuration must be a regular non-symlink file for upgrade evidence" >&2
    return 1
  }
  directory_metadata=$(stat --printf='%F\t%a\t%u\t%g' -- "$directory") || return 1
  metadata=$(stat --printf='%F\t%a\t%u\t%g\t%s' -- "$path") || return 1
  { printf '%s\000%s\000' "$directory_metadata" "$metadata"; cat -- "$path"; } > "$work/config-material"
  salted_commit_file manager-config-v1 "$work/config-material"
}

state_commitment() {
  local base=${PODMESH_PATH_ROOT:-}/var/lib/podmesh-manager relative path kind metadata digest target
  [ -d "$base" ] && [ ! -L "$base" ] || {
    echo "Manager state path must be a non-symlink directory for upgrade evidence" >&2
    return 1
  }
  : > "$work/state-records.jsonl"
  find -P "$base" -mindepth 1 -printf '%P\0' | sort -z > "$work/state-paths"
  while IFS= read -r -d '' relative; do
    path=$base/$relative
    metadata=$(stat --printf='%F\t%a\t%u\t%g\t%s' -- "$path") || return 1
    kind=${metadata%%$'\t'*}
    digest=null
    target=null
    if [ "$kind" = "regular file" ]; then
      digest=$(sha256sum -- "$path" | awk '{print $1}') || return 1
    elif [ "$kind" = "symbolic link" ]; then
      target=$(readlink -- "$path") || return 1
    fi
    jq -cn --arg path "$relative" --arg metadata "$metadata" --arg kind "$kind" --arg digest "$digest" --arg target "$target" \
      '{path:$path,metadata:$metadata,content_sha256:(if $kind=="regular file" then $digest else null end),link_target:(if $kind=="symbolic link" then $target else null end)}' >> "$work/state-records.jsonl"
  done < "$work/state-paths"
  jq -csS 'sort_by(.path)' "$work/state-records.jsonl" > "$work/state-manifest.json"
  salted_commit_file manager-state-v1 "$work/state-manifest.json"
}

collect_base() {
  "$root/collect-host.sh" --host-alias "$host_alias" --stage post-install \
    --salt-file "$salt_file" --candidate-verification "$candidate_verification" --output "$1"
}

collect_base "$work/base-before.json"
config_before=$(config_commitment)
state_before=$(state_commitment)
config=$(config_commitment)
state=$(state_commitment)
collect_base "$work/base-after.json"

if [ "$config_before" != "$config" ] || [ "$state_before" != "$state" ]; then
  echo "Protected manager configuration or state changed while commitments were collected" >&2
  exit 2
fi

if ! jq -e -n --slurpfile before "$work/base-before.json" --slurpfile after "$work/base-after.json" \
  '($before[0] | del(.captured_at_utc)) == ($after[0] | del(.captured_at_utc))' >/dev/null; then
  echo "Host evidence changed while protected-path commitments were collected" >&2
  exit 2
fi

jq -nS --arg schema_version podmesh-manager-inactive-upgrade-evidence/v1 \
  --arg stage "$stage" --slurpfile host "$work/base-after.json" \
  --arg config_commitment "$config" --arg state_commitment "$state" \
  '{schema_version:$schema_version,stage:$stage,host_evidence:$host[0],config_commitment:$config_commitment,state_commitment:$state_commitment}' > "$work/upgrade.json"
mkdir -p -- "$(dirname -- "$output")"
mv -- "$work/upgrade.json" "$output"
sha256sum -- "$output" > "$output.sha256"
