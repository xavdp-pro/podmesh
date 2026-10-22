#!/bin/bash
# ONLY inside a new image build. No collector/host path invokes this installer.
set -euo pipefail
export LC_ALL=C PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME=/root
unset APT_CONFIG LD_PRELOAD LD_LIBRARY_PATH PYTHONPATH PYTHONHOME MYSQL_PWD DPKG_ROOT DPKG_ADMINDIR BASH_ENV ENV CDPATH http_proxy https_proxy all_proxy ftp_proxy no_proxy
root=/opt/podmesh-python-closure
expected=${1:?mandatory trusted closure SHA256}
[[ $expected =~ ^[0-9a-f]{64}$ ]] || exit 64
[[ ${PODMESH_CANONICAL_IMAGE_BUILD:-} == 1 && $EUID == 0 ]] || exit 64
[[ -f /.dockerenv || -f /run/.containerenv ]] || { echo 'image-build boundary missing' >&2; exit 64; }
for helper in sha256sum awk cmp cp sort find stat dpkg-deb dpkg-query dpkg apt-get mktemp wc grep df; do
    command -v "$helper" >/dev/null || { echo "missing offline installer helper: $helper" >&2; exit 65; }
done
[[ -d $root && ! -L $root && -f $root/closure.lock && ! -L $root/closure.lock ]] || exit 65
[[ $(stat -c %s "$root/closure.lock") -le 131072 ]] || exit 65
printf '%s  %s\n' "$expected" "$root/closure.lock" | sha256sum --check --strict - >/dev/null
# The lock is data, never sourced/eval'd. Reject malformed lines, duplicates and unsafe fields.
work=$(mktemp -d /tmp/podmesh-python-offline.XXXXXXXX)
chmod 700 "$work"
policy_created=0
policy_saved=0
cleanup() {
    if [[ $policy_created == 1 ]]; then rm -f /usr/sbin/policy-rc.d; fi
    if [[ $policy_saved == 1 ]]; then mv "$work/policy-rc.d.saved" /usr/sbin/policy-rc.d; fi
    # On failure retain diagnostics; image build must fail rather than claim installation.
}
trap cleanup EXIT
awk -F '\t' '
function die() { exit 65 }
NR==1 { if ($0!="PODMESH_PYTHON_CLOSURE_V1") die(); next }
$1=="PACKAGE" {
 if (ended || NF!=8 || $2!~/^[a-z0-9][a-z0-9+.-]+$/ || $3!~/^[0-9][A-Za-z0-9.+:~\055]*$/ || ($4!="all" && $4!="amd64") || length($5)!=64 || $5!~/^[0-9a-f]+$/ || $6!~/^[1-9][0-9]*$/ || $7!=$5".deb" || $8!~/^[0-9]+$/ || names[$2]++ || hashes[$5]++) die();
 count++; total+=$6; print $2"\t"$3"\t"$4"\t"$5"\t"$6"\t"$7; next
}
$1=="END" { if (ended++ || NF!=3 || $2!=count || $3!=total || count<1 || count>64 || total>268435456) die(); next }
{
 if (ended || NF!=2 || seen[$1]++) die();
 if ($1=="REGISTRY") { if ($2!="sha256:93fc3fe333b6cdfb061425869c2c3a5bb2851c0c74dbebc2c940f024e7482d76") die() }
 else if ($1=="IMAGE") { if ($2!="sha256:53ef799caed285438d88678b529b4ff24406d4788f47ab8a74bb2212c707899a") die() }
 else if ($1=="ARCHIVE") { if ($2!="087a378584383840073ebc347e383229f9765572785b7b4cbf09f22a274b4efb") die() }
 else if ($1=="SUITE") { if ($2!="noble") die() }
 else if ($1=="ARCH") { if ($2!="amd64") die() }
 else if ($1=="STATUS" || $1=="OS_RELEASE" || $1=="KEY" || $1=="BINDING" || $1=="EVIDENCE") { if (length($2)!=64 || $2!~/^[0-9a-f]+$/) die() }
 else die()
}
END { if (!ended || count<1 || !names["python3"] || !names["libpython3-stdlib"]) die(); split("REGISTRY IMAGE ARCHIVE SUITE ARCH STATUS OS_RELEASE KEY BINDING EVIDENCE",required," "); for (i in required) if (!seen[required[i]]) die() }
' "$root/closure.lock" > "$work/packages.tsv"
field() { awk -F '\t' -v key="$1" '$1==key {print $2}' "$root/closure.lock"; }
printf '%s  %s\n' "$(field STATUS)" /var/lib/dpkg/status | sha256sum --check --strict - >/dev/null
printf '%s  %s\n' "$(field OS_RELEASE)" /etc/os-release | sha256sum --check --strict - >/dev/null
printf '%s  %s\n' "$(field KEY)" /usr/share/keyrings/ubuntu-archive-keyring.gpg | sha256sum --check --strict - >/dev/null
[[ -f $root/evidence.json && ! -L $root/evidence.json && -d $root/debs && ! -L $root/debs ]] || exit 65
printf '%s  %s\n' "$(field EVIDENCE)" "$root/evidence.json" | sha256sum --check --strict - >/dev/null
[[ -z $(find "$root" -type l -print -quit) ]] || exit 65
[[ $(find "$root" -name '*.deb' -printf '%P\n' | wc -l) -eq $(wc -l < "$work/packages.tsv") ]] || exit 65
[[ $(find "$root/debs" -mindepth 1 -maxdepth 1 -printf '%f\n' | wc -l) == $(wc -l < "$work/packages.tsv") ]] || exit 65
# Validate every byte/identity before any privileged installation, including unknown extra artifacts.
debs=()
while IFS=$'\t' read -r name version arch hash bytes filename; do
    artifact="$root/debs/$filename"
    [[ -f $artifact && ! -L $artifact && $(stat -c %s "$artifact") == "$bytes" ]] || exit 65
    printf '%s  %s\n' "$hash" "$artifact" | sha256sum --check --strict - >/dev/null
    [[ $(dpkg-deb --field "$artifact" Package) == "$name" && $(dpkg-deb --field "$artifact" Version) == "$version" && $(dpkg-deb --field "$artifact" Architecture) == "$arch" ]] || exit 65
    # New packages only; no image base reinstall or upgrade is authorized.
    if dpkg-query --show --showformat='${Status}' "$name" 2>/dev/null | grep -qx 'install ok installed'; then exit 65; fi
    debs+=("$artifact")
done < "$work/packages.tsv"
[[ $(dpkg --print-architecture) == amd64 ]] || exit 65
# Empty APT sources/config/hooks; dpkg/status is this IMAGE, never the collector host.
mkdir -p "$work/etc/parts" "$work/etc/sources.d" "$work/state/lists/partial" "$work/cache/archives/partial" "$work/log"
: > "$work/etc/empty.conf"
: > "$work/etc/sources.list"
: > "$work/etc/trusted.gpg"
: > "$work/state/extended_states"
: > "$work/etc/preferences"
: > "$work/etc/auth.conf"
mkdir -p "$work/etc/preferences.d" "$work/etc/auth.d"
cat > "$work/apt.conf" <<EOF
Dir::Etc "$work/etc";
Dir::Etc::main "$work/etc/empty.conf";
Dir::Etc::parts "$work/etc/parts";
Dir::Etc::sourcelist "$work/etc/sources.list";
Dir::Etc::sourceparts "$work/etc/sources.d";
Dir::Etc::trusted "$work/etc/trusted.gpg";
Dir::Etc::trustedparts "$work/etc/parts";
Dir::Etc::preferences "$work/etc/preferences";
Dir::Etc::preferencesparts "$work/etc/preferences.d";
Dir::Etc::netrc "$work/etc/auth.conf";
Dir::Etc::netrcparts "$work/etc/auth.d";
Dir::State "$work/state";
Dir::State::status "/var/lib/dpkg/status";
Dir::State::extended_states "$work/state/extended_states";
Dir::State::lists "$work/state/lists";
Dir::Cache "$work/cache";
Dir::Cache::archives "$work/cache/archives";
Dir::Cache::pkgcache "$work/cache/pkgcache.bin";
Dir::Cache::srcpkgcache "$work/cache/srcpkgcache.bin";
Dir::Log "$work/log";
APT::Install-Recommends "false";
APT::Install-Suggests "false";
Acquire::Retries "0";
Acquire::http::Proxy "http://127.0.0.1:9";
Acquire::https::Proxy "http://127.0.0.1:9";
#clear DPkg::Pre-Invoke;
#clear DPkg::Post-Invoke;
#clear DPkg::Pre-Install-Pkgs;
EOF
export APT_CONFIG="$work/apt.conf" DEBIAN_FRONTEND=noninteractive
apt-get --simulate --no-download --no-upgrade --no-remove --no-install-recommends install "${debs[@]}" > "$work/plan.log"
# Reject extra resolution, version changes and removals before invoking installation.
awk -F '\t' 'NR==FNR {v[$1]=$2; next} /^Remv |^Purg / {exit 65} /^Inst / {n=$2; sub(/:amd64$/,"",n); if (!(n in v) || index($0,"[" v[n] "]")) exit 65; want="("v[n]" "; if (!index($0,want)) exit 65; found[n]++} END {for (n in v) if (found[n]!=1) exit 65}' "$work/packages.tsv" "$work/plan.log"
# Bind base package versions before native configuration, then compare exact full set.
dpkg-query --show --showformat='${Package}\t${Version}\t${Architecture}\t${Status}\n' | sort > "$work/packages-before.tsv"
installed_kib=$(awk -F '\t' '$1=="PACKAGE" {n+=$8} END {printf "%.0f",n}' "$root/closure.lock")
free_kib=$(df -Pk / | awk 'NR==2 {print $4}')
[[ $installed_kib =~ ^[0-9]+$ && $free_kib =~ ^[0-9]+$ && $installed_kib -lt $free_kib ]] || exit 65
# Native package resolver preserves Pre-Depends/configuration order; never force dependencies.
if [[ -e /usr/sbin/policy-rc.d || -L /usr/sbin/policy-rc.d ]]; then
    [[ -f /usr/sbin/policy-rc.d && ! -L /usr/sbin/policy-rc.d ]] || exit 65
    mv /usr/sbin/policy-rc.d "$work/policy-rc.d.saved"
    policy_saved=1
fi
printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d
chmod 755 /usr/sbin/policy-rc.d
policy_created=1
apt-get --yes --no-download --no-upgrade --no-remove --no-install-recommends install "${debs[@]}"
while IFS=$'\t' read -r name version arch _hash _bytes _filename; do
    [[ $(dpkg-query --show --showformat='${Version}' "$name") == "$version" && $(dpkg-query --show --showformat='${Architecture}' "$name") == "$arch" && $(dpkg-query --show --showformat='${Status}' "$name") == 'install ok installed' ]] || exit 65
    [[ -z $(dpkg --verify "$name") ]] || exit 65
done < "$work/packages.tsv"
cp "$work/packages-before.tsv" "$work/packages-expected.tsv"
while IFS=$'\t' read -r name version arch _hash _bytes _filename; do
    printf '%s\t%s\t%s\tinstall ok installed\n' "$name" "$version" "$arch" >> "$work/packages-expected.tsv"
done < "$work/packages.tsv"
sort "$work/packages-expected.tsv" > "$work/packages-expected-sorted.tsv"
dpkg-query --show --showformat='${Package}\t${Version}\t${Architecture}\t${Status}\n' | sort > "$work/packages-after.tsv"
cmp "$work/packages-expected-sorted.tsv" "$work/packages-after.tsv"
/usr/bin/python3 -I -c 'import hashlib,json,pathlib,secrets,socket,subprocess,tempfile,unittest,os; assert hasattr(os,"WNOWAIT")'
# Restore service policy successfully BEFORE recording any completed installation.
cleanup
policy_created=0
policy_saved=0
trap - EXIT
rm -rf -- "$work"
# Retain lock, packages, signed evidence and this verification result in the image.
printf '%s\n' "$expected" > "$root/installed.lock.sha256"
