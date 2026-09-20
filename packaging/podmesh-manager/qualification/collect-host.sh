#!/bin/bash
# Read-only local collector. It never contacts another host or changes host state.
set -euo pipefail
export LC_ALL=C

usage() { echo "Usage: $0 --host-alias <stable-alias> --stage <pre-install|post-install> --salt-file <private-salt> --output <evidence.json> [--candidate-verification <report.json>]" >&2; exit 2; }
host_alias= stage= output= salt_file= candidate_verification=
while [ "$#" -gt 0 ]; do case "$1" in
  --host-alias) host_alias=${2-}; shift 2 ;; --stage) stage=${2-}; shift 2 ;;
  --salt-file) salt_file=${2-}; shift 2 ;; --candidate-verification) candidate_verification=${2-}; shift 2 ;;
  --output) output=${2-}; shift 2 ;; *) usage ;; esac; done
[ "$(id -u)" -eq 0 ] || { echo "Run as root to inventory the rootful Podman store" >&2; exit 2; }
[[ "$host_alias" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$ ]] || usage
case "$stage" in pre-install|post-install) ;; *) usage ;; esac
[ -n "$output" ] && [ -f "$salt_file" ] || usage
[ "$(stat -c '%a:%u:%F' -- "$salt_file")" = '600:0:regular file' ] || { echo "Salt file must be root-owned mode 0600" >&2; exit 2; }
[ "$(stat -c '%s' -- "$salt_file")" -ge 32 ] || { echo "Salt file must contain at least 32 bytes" >&2; exit 2; }
case "$stage" in pre-install) [ -z "$candidate_verification" ] || usage ;; post-install) [ -f "$candidate_verification" ] || usage ;; esac
for command in jq podman systemctl dpkg-query dpkg getent stat sha256sum find wc readlink tr; do command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }; done
work=$(mktemp -d); trap 'rm -rf -- "$work"' EXIT

package_json() {
  local package=$1 status rc
  set +e; status=$(dpkg-query -W -f='${db:Status-Status}' "$package" 2>"$work/dpkg-$package.err"); rc=$?; set -e
  if [ "$rc" -eq 0 ]; then
    case "$status" in installed) jq -cn --arg version "$(dpkg-query -W -f='${Version}' "$package")" '{status:"installed",version:$version}' ;; not-installed|config-files) jq -cn '{status:"absent",version:null}' ;; *) echo "Unexpected dpkg status for $package: $status" >&2; return 1 ;; esac
  elif [ "$rc" -eq 1 ] && grep -Fq 'no packages found matching' "$work/dpkg-$package.err"; then jq -cn '{status:"absent",version:null}'
  else echo "dpkg-query failed for $package" >&2; return 1; fi
}

commit() { { cat -- "$salt_file"; printf '\n'; jq -cS .; } | sha256sum | awk '{print "sha256:" $1}'; }

unit_json() {
  local unit=$1 raw value invocation commitment
  raw=$(systemctl show "$unit" --no-page --property=LoadState --property=ActiveState --property=SubState --property=UnitFileState --property=MainPID --property=ExecMainPID --property=InvocationID --property=ExecMainStartTimestampMonotonic --property=NRestarts)
  value=$(jq -Rn --arg unit "$unit" --arg raw "$raw" '
    reduce ($raw | split("\n")[] | select(length > 0) | split("=") | {key:.[0],value:(.[1:]|join("="))}) as $row
    ({unit:$unit,load_state:"unknown",active_state:"unknown",sub_state:"unknown",unit_file_state:"unknown",main_pid:null,exec_main_pid:null,invocation_id:null,start_monotonic_usec:null,n_restarts:null};
      if $row.key=="LoadState" then .load_state=$row.value elif $row.key=="ActiveState" then .active_state=$row.value elif $row.key=="SubState" then .sub_state=$row.value elif $row.key=="UnitFileState" then .unit_file_state=$row.value elif $row.key=="MainPID" then .main_pid=($row.value|tonumber) elif $row.key=="ExecMainPID" then .exec_main_pid=($row.value|tonumber) elif $row.key=="InvocationID" then .invocation_id=($row.value|if length==0 then null else . end) elif $row.key=="ExecMainStartTimestampMonotonic" then .start_monotonic_usec=($row.value|tonumber) elif $row.key=="NRestarts" then .n_restarts=($row.value|tonumber) else . end) |
      if (.load_state=="unknown" or .active_state=="unknown" or .sub_state=="unknown" or .unit_file_state=="unknown" or .main_pid==null or .exec_main_pid==null or .start_monotonic_usec==null or .n_restarts==null) then error("incomplete systemd unit reading") else . end')
  invocation=$(jq -r '.invocation_id // empty' <<<"$value")
  if [ -n "$invocation" ]; then commitment=$(printf '%s' "$invocation" | jq -R . | commit); jq -cS --arg commitment "$commitment" '.invocation_id=$commitment' <<<"$value"; else printf '%s\n' "$value"; fi
}

socket_json() { local path=$1; if [ -e "$path" ] || [ -L "$path" ]; then stat --printf='%F\t%a\t%u\t%g\t%i\t%s\n' -- "$path" | jq -R --arg path "$path" 'split("\t")|{path:$path,present:true,file_type:.[0],mode:.[1],uid:(.[2]|tonumber),gid:(.[3]|tonumber),inode:(.[4]|tonumber),bytes:(.[5]|tonumber)}'; else jq -cn --arg path "$path" '{path:$path,present:false,file_type:null,mode:null,uid:null,gid:null,inode:null,bytes:null}'; fi; }

manager_account_json() {
  local name=podmesh-manager passwd_entry passwd_rc group_entry group_rc
  set +e
  passwd_entry=$(getent passwd "$name" 2>"$work/getent-passwd.err")
  passwd_rc=$?
  set -e
  if [ "$passwd_rc" -eq 2 ] && [ -z "$passwd_entry" ]; then
    jq -cn --arg name "$name" '{name:$name,present:false,uid:null,primary_gid:null,primary_group:null,home:null,shell:null}'
    return
  fi
  [ "$passwd_rc" -eq 0 ] && [ -n "$passwd_entry" ] || { echo "Cannot read manager account" >&2; return 1; }

  set +e
  group_entry=$(getent group "$name" 2>"$work/getent-group.err")
  group_rc=$?
  set -e
  [ "$group_rc" -eq 0 ] && [ -n "$group_entry" ] || { echo "Cannot read manager primary group" >&2; return 1; }

  jq -cen --arg name "$name" --arg passwd_entry "$passwd_entry" --arg group_entry "$group_entry" '
    ($passwd_entry | split("\n")) as $passwd_lines |
    ($group_entry | split("\n")) as $group_lines |
    if ($passwd_lines | length) != 1 or ($group_lines | length) != 1 then error("ambiguous manager account identity") else
      ($passwd_lines[0] | split(":")) as $passwd |
      ($group_lines[0] | split(":")) as $group |
      if ($passwd | length) != 7 or ($group | length) != 4 or
         $passwd[0] != $name or $group[0] != $name or
         ($passwd[2] | test("^[0-9]+$") | not) or
         ($passwd[3] | test("^[0-9]+$") | not) or
         ($group[2] | test("^[0-9]+$") | not) or
         $passwd[3] != $group[2] or
         ($passwd[5] | length) == 0 or ($passwd[6] | length) == 0
      then error("invalid manager account identity")
      else {name:$name,present:true,uid:($passwd[2] | tonumber),primary_gid:($passwd[3] | tonumber),primary_group:$group[0],home:$passwd[5],shell:$passwd[6]}
      end
    end'
}

manager_state_directory_json() {
  local path=/var/lib/podmesh-manager actual_path=${PODMESH_PATH_ROOT:-}/var/lib/podmesh-manager metadata file_type entry_count=null
  if [ -e "$actual_path" ] || [ -L "$actual_path" ]; then
    metadata=$(stat --printf='%F\t%U\t%G\t%a\n' -- "$actual_path") || return 1
    file_type=${metadata%%$'\t'*}
    if [ "$file_type" = directory ]; then
      entry_count=$(find -P "$actual_path" -mindepth 1 -maxdepth 1 -printf '.' | wc -c | tr -d '[:space:]') || return 1
    fi
    jq -Ren --arg path "$path" --arg metadata "$metadata" --argjson entry_count "$entry_count" '
        ($metadata | split("\t")) as $fields |
        if ($fields | length) != 4 or any($fields[0:3][]; length == 0) or ($fields[3] | test("^[0-7]{3,4}$") | not)
        then error("invalid manager state directory metadata")
        else {path:$path,present:true,file_type:$fields[0],owner:$fields[1],group:$fields[2],mode:$fields[3],entry_count:$entry_count}
        end'
  else
    jq -cn --arg path "$path" '{path:$path,present:false,file_type:null,owner:null,group:null,mode:null,entry_count:null}'
  fi
}

path_present_json() {
  local path=$1 actual_path=${PODMESH_PATH_ROOT:-}$1
  if [ -e "$actual_path" ] || [ -L "$actual_path" ]; then printf '%s\n' true; else printf '%s\n' false; fi
}

podman_commitment() {
  local object=$1 projection=$2 raw="$work/$1.json"
  case "$object" in
    containers) podman ps -a --format json > "$raw" || return 1 ;;
    images) podman images --format json > "$raw" || return 1 ;;
    volumes) podman volume ls --format json > "$raw" || return 1 ;;
    networks) podman network ls --format json > "$raw" || return 1 ;;
    pods) podman pod ls --format json > "$raw" || return 1 ;;
    *) return 2 ;;
  esac
  [ -s "$raw" ] || return 1
  jq -e -cS "$projection" "$raw" | commit
}

capture_or_fail() {
  local variable=$1 rc value
  shift
  set +e
  value=$("$@")
  rc=$?
  set -e
  [ "$rc" -eq 0 ] || { echo "Evidence acquisition failed: $1" >&2; exit 2; }
  [ -n "$value" ] || { echo "Evidence acquisition returned empty output: $1" >&2; exit 2; }
  printf -v "$variable" '%s' "$value"
}

manager_process_count() {
  local entry pid executable cmdline_bytes count=0
  local proc_root=${PODMESH_PROC_ROOT:-/proc}
  for entry in "$proc_root"/[0-9]*; do
    [ -d "$entry" ] || continue
    pid=${entry#"$proc_root"/}
    if ! executable=$(readlink -- "$entry/exe" 2>/dev/null); then
      [ ! -d "$entry" ] && continue
      if ! cmdline_bytes=$(wc -c 2>/dev/null < "$entry/cmdline"); then
        [ ! -d "$entry" ] && continue
        echo "Cannot read command line for process $pid after executable lookup failed" >&2
        return 1
      fi
      [ ! -d "$entry" ] && continue
      [[ "$cmdline_bytes" =~ ^[0-9]+$ ]] || { echo "Invalid command line byte count for process $pid" >&2; return 1; }
      [ "$cmdline_bytes" -eq 0 ] && continue
      echo "Cannot read executable for process $pid" >&2
      return 1
    fi
    executable=${executable% (deleted)}
    [ -n "$executable" ] || { echo "Empty executable for process $pid" >&2; return 1; }
    [ "$executable" = /usr/lib/podmesh-manager/podmesh-managerd ] && count=$((count + 1))
  done
  printf '%s\n' "$count"
}

manifest_commitment() {
  jq -cS . | sha256sum | awk '{print "sha256:" $1}'
}

installed_payload_manifest() {
  local package=$1 path actual_path sha256 manifest="$work/installed-payload.json"
  printf '%s\n' '[]' > "$manifest"
  dpkg-query -L "$package" > "$work/installed-files.list" || return 1
  while IFS= read -r path || [ -n "$path" ]; do
    [[ "$path" == /* ]] || { echo "dpkg-query returned a non-absolute package path" >&2; return 1; }
    actual_path=${PODMESH_PATH_ROOT:-}$path
    if [ -L "$actual_path" ]; then
      continue
    fi
    if [ -f "$actual_path" ]; then
      sha256=$(sha256sum -- "$actual_path" | awk '{print $1}') || return 1
      jq --arg path "$path" --arg sha256 "$sha256" \
        '. + [{path:$path,sha256:$sha256}]' "$manifest" > "$work/manifest.next" || return 1
      mv -- "$work/manifest.next" "$manifest" || return 1
    fi
  done < "$work/installed-files.list"
  jq -cS 'sort_by(.path)' "$manifest"
}

installed_maintainer_scripts_manifest() {
  local package=$1 name path sha256 manifest="$work/installed-maintainer-scripts.json"
  printf '%s\n' '[]' > "$manifest"
  dpkg-query --control-list "$package" > "$work/control-files.list" || return 1
  while IFS= read -r name || [ -n "$name" ]; do
    case "$name" in config|postinst|postrm|preinst|prerm) ;; *) continue ;; esac
    path=$(dpkg-query --control-path "$package" "$name") || return 1
    [ -n "$path" ] && [ -f "$path" ] && [ ! -L "$path" ] || { echo "Installed maintainer script is not a regular file: $name" >&2; return 1; }
    sha256=$(sha256sum -- "$path" | awk '{print $1}') || return 1
    jq --arg name "$name" --arg sha256 "$sha256" \
      '. + [{name:$name,sha256:$sha256}]' "$manifest" > "$work/manifest.next" || return 1
    mv -- "$work/manifest.next" "$manifest" || return 1
  done < "$work/control-files.list"
  jq -cS 'sort_by(.name)' "$manifest"
}

candidate_binding_json() {
  local report=$1 package version expected actual binary output rc payload scripts payload_commitment scripts_commitment
  jq -e '
    (keys | sort) == (["architecture","binary_sha256","deb_sha256","maintainer_scripts","package","regular_payload_files","schema_version","signed_metadata","source_commit","verified_at_utc","version"] | sort) and
    .schema_version=="podmesh-manager-candidate-verification/v2" and .package=="podmesh-manager" and
    (.version|type=="string" and length>0) and .architecture=="amd64" and
    (.deb_sha256|test("^[a-f0-9]{64}$")) and (.binary_sha256|test("^[a-f0-9]{64}$")) and
    (.source_commit|type=="string" and length>0) and (.verified_at_utc|type=="string" and length>0) and
    (.signed_metadata|type=="object" and
      (keys | sort) == (["inrelease_signature","keyring_sha256","packages_path","packages_sha256","packages_size","signing_fingerprint"] | sort) and
      .inrelease_signature=="verified-by-gpgv-and-pinned-fingerprint" and
      (.signing_fingerprint|test("^[A-F0-9]{40}$")) and (.keyring_sha256|test("^[a-f0-9]{64}$")) and
      (.packages_path|type=="string" and length>0) and (.packages_sha256|test("^[a-f0-9]{64}$")) and
      (.packages_size|type=="number" and .>=0 and floor==.)) and
    (.regular_payload_files|type=="array" and length>0 and
      all(.[]; (keys | sort)==["path","sha256"] and (.path|type=="string" and startswith("/")) and (.sha256|test("^[a-f0-9]{64}$"))) and
      (map(.path) == (map(.path)|sort|unique))) and
    (.maintainer_scripts|type=="array" and
      all(.[]; (keys | sort)==["name","sha256"] and (.name|test("^(config|postinst|postrm|preinst|prerm)$")) and (.sha256|test("^[a-f0-9]{64}$"))) and
      (map(.name) == (map(.name)|sort|unique)))
  ' "$report" >/dev/null || { echo "Invalid candidate verification report" >&2; return 1; }
  package=$(jq -r .package "$report"); version=$(jq -r .version "$report"); expected=$(jq -r .binary_sha256 "$report")
  [ "$(dpkg-query -W -f='${db:Status-Status}' "$package")" = installed ] && [ "$(dpkg-query -W -f='${Version}' "$package")" = "$version" ] || { echo "Installed package differs from candidate verification" >&2; return 1; }
  binary=${PODMESH_PATH_ROOT:-}/usr/lib/podmesh-manager/podmesh-managerd; [ -f "$binary" ] || { echo "Installed candidate binary is missing" >&2; return 1; }; actual=$(sha256sum -- "$binary"|awk '{print $1}'); [ "$actual" = "$expected" ] || { echo "Installed candidate binary differs from verified report" >&2; return 1; }
  payload=$(installed_payload_manifest "$package") || { echo "Cannot hash installed package payload" >&2; return 1; }
  scripts=$(installed_maintainer_scripts_manifest "$package") || { echo "Cannot hash installed maintainer scripts" >&2; return 1; }
  jq -e --argjson payload "$payload" --argjson scripts "$scripts" \
    '.regular_payload_files == $payload and .maintainer_scripts == $scripts' "$report" >/dev/null || { echo "Installed package content differs from candidate verification" >&2; return 1; }
  payload_commitment=$(printf '%s\n' "$payload" | manifest_commitment)
  scripts_commitment=$(printf '%s\n' "$scripts" | manifest_commitment)
  set +e; output=$(dpkg --verify "$package" 2>&1); rc=$?; set -e; [ "$rc" -eq 0 ] && [ -z "$output" ] || { echo "dpkg verification of candidate package is not clean" >&2; return 1; }
  jq -cn --arg version "$version" --arg binary_sha256 "$actual" --arg regular_payload_files_commitment "$payload_commitment" --arg maintainer_scripts_commitment "$scripts_commitment" --arg verification_commitment "$(sha256sum -- "$report"|awk '{print "sha256:" $1}')" '{package:"podmesh-manager",version:$version,binary_sha256:$binary_sha256,regular_payload_files_commitment:$regular_payload_files_commitment,maintainer_scripts_commitment:$maintainer_scripts_commitment,dpkg_verify:"clean",verification_commitment:$verification_commitment}'
}

boot_id=$(cat /proc/sys/kernel/random/boot_id) || { echo "Cannot read boot identifier" >&2; exit 2; }
boot_id_commitment=$(printf '%s' "$boot_id" | jq -R . | commit)
capture_or_fail manager_process_count manager_process_count
capture_or_fail manager_account manager_account_json
capture_or_fail manager_state_directory manager_state_directory_json
capture_or_fail containers podman_commitment containers 'if type != "array" then error("invalid container inventory") else map({id:(.Id//.ID//error("missing container id")),state:(.State//""),started_at:(.StartedAt//error("missing container start time")),pid:(.Pid//error("missing container pid")),restarts:(.Restarts//error("missing container restart count"))})|sort_by(.id) end'
capture_or_fail images podman_commitment images 'if type != "array" then error("invalid image inventory") else map({id:(.Id//.ID//error("missing image id"))})|sort_by(.id) end'
capture_or_fail volumes podman_commitment volumes 'if type != "array" then error("invalid volume inventory") else map({name:(.Name//error("missing volume name")),driver:(.Driver//"")})|sort_by(.name) end'
capture_or_fail networks podman_commitment networks 'if type != "array" then error("invalid network inventory") else map({id:(.id//.Id//.ID//error("missing network id")),driver:(.driver//.Driver//"")})|sort_by(.id) end'
capture_or_fail pods podman_commitment pods 'if type != "array" then error("invalid pod inventory") else map({id:(.Id//.ID//error("missing pod id")),status:(.Status//"")})|sort_by(.id) end'
capture_or_fail config_present path_present_json /etc/podmesh-manager/config.json
capture_or_fail runtime_present path_present_json /run/podmesh-manager
if [ "$stage" = post-install ]; then binding=$(candidate_binding_json "$candidate_verification"); else binding=null; fi
jq -n --arg schema_version 'podmesh-manager-host-evidence/v5' --arg stage "$stage" --arg host_alias "$host_alias" --arg captured_at_utc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg boot_id_commitment "$boot_id_commitment" --argjson podmesh "$(package_json podmesh)" --argjson observer "$(package_json podmesh-web-observer)" --argjson manager_package "$(package_json podmesh-manager)" --argjson lifecycle "$(unit_json podmesh.service)" --argjson observer_unit "$(unit_json podmesh-web-observer.service)" --argjson manager_unit "$(unit_json podmesh-manager.service)" --argjson lifecycle_socket "$(socket_json /run/podmesh/api.sock)" --argjson observer_socket "$(socket_json /run/podmesh-web-observer/api.sock)" --argjson binding "$binding" --argjson process_count "$manager_process_count" --argjson manager_account "$manager_account" --argjson manager_state_directory "$manager_state_directory" --argjson config_present "$config_present" --argjson runtime_present "$runtime_present" --arg containers "$containers" --arg images "$images" --arg volumes "$volumes" --arg networks "$networks" --arg pods "$pods" '{schema_version:$schema_version,stage:$stage,host_alias:$host_alias,captured_at_utc:$captured_at_utc,boot_id_commitment:$boot_id_commitment,packages:{podmesh:$podmesh,"podmesh-web-observer":$observer,"podmesh-manager":$manager_package},services:{"podmesh.service":$lifecycle,"podmesh-web-observer.service":$observer_unit,"podmesh-manager.service":$manager_unit},sockets:{"/run/podmesh/api.sock":$lifecycle_socket,"/run/podmesh-web-observer/api.sock":$observer_socket},podman_rootful:{projection_version:"v2",commitments:{containers:$containers,images:$images,volumes:$volumes,networks:$networks,pods:$pods}},manager:{process_count:$process_count,account:$manager_account,config_present:$config_present,state_directory:$manager_state_directory,runtime_present:$runtime_present,candidate_binding:$binding}}' | jq -S . > "$work/evidence.json"
mkdir -p -- "$(dirname -- "$output")"; mv -- "$work/evidence.json" "$output"; sha256sum -- "$output" > "$output.sha256"
