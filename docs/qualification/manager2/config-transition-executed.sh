#!/bin/sh
set -eu
config=/etc/podmesh-manager/config.json
runtime=/run/podmesh-manager
backup=/root/podmesh-manager1-config-before-ff77b1f946e8.json
binary=/usr/lib/podmesh-manager/podmesh-managerd

[ ! -e "$backup" ]
test -f "$config" && test ! -L "$config"
test "$(stat -c '%F %a %U %G %h' -- "$config")" = 'regular file 640 root podmesh-manager 1'
test ! -e "$runtime"
install -m 0600 -o root -g root "$config" "$backup"
candidate=$(mktemp --tmpdir=/etc/podmesh-manager .config.json.XXXXXX)
trap 'rm -f -- "$candidate"' EXIT HUP INT TERM
jq 'if has("observation_writer_uid") then error("observation_writer_uid already exists") else . + {observation_writer_uid: 0} end' "$config" > "$candidate"
chown root:podmesh-manager "$candidate"
chmod 0640 "$candidate"
install -d -o podmesh-manager -g podmesh-manager -m 0700 "$runtime"
runuser -u podmesh-manager -- "$binary" --config "$candidate" --state-dir /var/lib/podmesh-manager --runtime-dir "$runtime" --validate-config
mv -fT -- "$candidate" "$config"
trap - EXIT HUP INT TERM
test "$(stat -c '%F %a %U %G %h' -- "$config")" = 'regular file 640 root podmesh-manager 1'
runuser -u podmesh-manager -- "$binary" --config "$config" --state-dir /var/lib/podmesh-manager --runtime-dir "$runtime" --validate-config
test -z "$(find "$runtime" -mindepth 1 -print -quit)"
rmdir "$runtime"
test ! -e "$runtime"
systemctl is-active --quiet podmesh-manager.service && exit 1 || :
printf '%s\n' 'CONFIG_TRANSITION_OK'
