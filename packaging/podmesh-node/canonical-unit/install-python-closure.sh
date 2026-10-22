#!/bin/bash
# ONLY inside a new image build. No collector/host path invokes this installer.
set -Eeuo pipefail
# BEGIN OFFLINE_INSTALLER_SUPPORT
# Fixed diagnostic fields only: never print commands, environment or lock contents.
stage=bootstrap
guard=initialization
policy_created=0
policy_saved=0
cleanup_done=0
cleanup_status=0
failure_reported=0
report_failure() {
    failure_reported=1
    printf 'offline-installer stage=%s guard=%s kind=%s line=%s exit=%s\n' \
        "$stage" "$guard" "$1" "$2" "$3" >&2
}
refuse() {
    report_failure guard "${BASH_LINENO[0]}" "$1"
    exit "$1"
}
cleanup() {
    if [[ $cleanup_done == 1 ]]; then return "$cleanup_status"; fi
    cleanup_done=1
    if [[ $policy_created == 1 ]]; then
        if rm -f -- "$policy_path"; then policy_created=0
        else
            cleanup_status=$?
            report_failure cleanup-remove "$LINENO" "$cleanup_status"
            return "$cleanup_status" # Keep the saved original; never overwrite on removal failure.
        fi
    fi
    if [[ $policy_saved == 1 ]]; then
        if mv -- "$work/policy-rc.d.saved" "$policy_path"; then policy_saved=0
        else
            cleanup_status=$?
            report_failure cleanup-restore "$LINENO" "$cleanup_status"
            return "$cleanup_status" # Retain the only saved original and work directory.
        fi
    fi
}
on_exit() {
    local line=$1 original=$2 restored=0
    trap - ERR EXIT
    set +e
    if [[ $original != 0 && $failure_reported == 0 ]]; then report_failure exit "$line" "$original"; fi
    cleanup || restored=$?
    if [[ $original != 0 ]]; then exit "$original"; fi
    exit "$restored"
}
# The parser is also extracted verbatim by isolated fixture tests; no installer bypass.
validate_plan() {
    awk '
function die(reason) {
 failed=1;
 printf "offline-installer stage=plan_validate guard=%s input_line=%d exit=65\n",reason,FNR > "/dev/stderr";
 exit 65
}
FILENAME==ARGV[1] {
 if (length($0)>4096 || ++packages>64) die("manifest-bound");
 k=split($0,row,"\t");
 if (k!=6 || row[1]!~/^[a-z0-9][a-z0-9+.-]+$/ || row[2]!~/^[0-9][A-Za-z0-9.+:~\055]*$/ || (row[3]!="amd64" && row[3]!="all") || (row[1] in v)) die("manifest-identity");
 v[row[1]]=row[2]; a[row[1]]=row[3]; next
}
{
 if (++lines>8192 || length($0)>4096) die("plan-bound");
 line=$0; sub(/^[ \t]+/,"",line); sub(/[ \t]+$/,"",line);
 if (line!~/^(Inst|Conf|Remv|Purg)/) next;
 k=split(line,t,/[ \t]+/);
 if (t[1]=="Remv" || t[1]=="Purg") die("removal");
 if ((t[1]!="Inst" && t[1]!="Conf") || k<4) die("action-shape");
 count=split(t[2],qualified,":"); n=qualified[1];
 if (!(n in v) || count>2 || (count==2 && qualified[2]!=a[n])) die("package-architecture");
 # Current installed versions appear as [old] BEFORE the candidate: always refuse.
 if (t[3]!="(" v[n] || t[k]!="[" a[n] "])") die("candidate-version-architecture");
 for (i=4;i<k;i++) if (t[i]!~/^[-A-Za-z0-9._:+~\/,]+$/) die("release-shape");
 if (t[1]=="Inst") {if (++found[n]!=1) die("duplicate-install")}
 else {if (found[n]!=1 || ++configured[n]!=1) die("unexpected-configure")}
}
END {
 if (failed) exit 65;
 if (packages<1) die("empty-manifest");
 for (n in v) if (found[n]!=1 || configured[n]!=1) die("missing-action")
}
' "$1" "$2"
}
# END OFFLINE_INSTALLER_SUPPORT
trap 'report_failure command "$LINENO" "$?"' ERR
trap 'on_exit "$LINENO" "$?"' EXIT
policy_path=/usr/sbin/policy-rc.d
export LC_ALL=C PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME=/root
unset APT_CONFIG LD_PRELOAD LD_LIBRARY_PATH PYTHONPATH PYTHONHOME MYSQL_PWD DPKG_ROOT DPKG_ADMINDIR BASH_ENV ENV CDPATH http_proxy https_proxy all_proxy ftp_proxy no_proxy
root=/opt/podmesh-python-closure
guard=trusted-lock-argument
[[ $# == 1 ]] || refuse 64
expected=$1
stage=boundary guard=trusted-lock-shape
[[ $expected =~ ^[0-9a-f]{64}$ ]] || refuse 64
guard=image-build-authority
[[ ${PODMESH_CANONICAL_IMAGE_BUILD:-} == 1 && $EUID == 0 ]] || refuse 64
guard=image-build-marker
[[ -f /.dockerenv || -f /run/.containerenv ]] || { echo 'image-build boundary missing' >&2; refuse 64; }
stage=prerequisites guard=required-tool
for helper in sha256sum awk cmp cp sort find stat dpkg-deb dpkg-query dpkg apt-get mktemp wc grep df; do
    command -v "$helper" >/dev/null || { echo "missing offline installer helper: $helper" >&2; refuse 65; }
done
stage=lock_verify guard=lock-path
[[ -d $root && ! -L $root && -f $root/closure.lock && ! -L $root/closure.lock ]] || refuse 65
guard=lock-size
[[ $(stat -c %s "$root/closure.lock") -le 131072 ]] || refuse 65
guard=trusted-lock-checksum
printf '%s  %s\n' "$expected" "$root/closure.lock" | sha256sum --check --strict - >/dev/null
# The lock is data, never sourced/eval'd. Reject malformed lines, duplicates and unsafe fields.
stage=work_prepare guard=private-work-create
work=$(mktemp -d /tmp/podmesh-python-offline.XXXXXXXX)
chmod 700 "$work"
stage=lock_decode guard=lock-schema
awk -F '\t' '
function die() { failed=1; print "offline-installer stage=lock_decode guard=lock-schema input_line=" NR " exit=65" > "/dev/stderr"; exit 65 }
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
END { if (failed) exit 65; if (!ended || count<1 || !names["python3"] || !names["libpython3-stdlib"]) die(); split("REGISTRY IMAGE ARCHIVE SUITE ARCH STATUS OS_RELEASE KEY BINDING EVIDENCE",required," "); for (i in required) if (!seen[required[i]]) die() }
' "$root/closure.lock" > "$work/packages.tsv"
field() { awk -F '\t' -v key="$1" '$1==key {print $2}' "$root/closure.lock"; }
stage=base_verify guard=base-status-checksum
printf '%s  %s\n' "$(field STATUS)" /var/lib/dpkg/status | sha256sum --check --strict - >/dev/null
guard=base-os-checksum
printf '%s  %s\n' "$(field OS_RELEASE)" /etc/os-release | sha256sum --check --strict - >/dev/null
guard=base-key-checksum
printf '%s  %s\n' "$(field KEY)" /usr/share/keyrings/ubuntu-archive-keyring.gpg | sha256sum --check --strict - >/dev/null
stage=bundle_verify guard=evidence-path
[[ -f $root/evidence.json && ! -L $root/evidence.json && -d $root/debs && ! -L $root/debs ]] || refuse 65
guard=evidence-checksum
printf '%s  %s\n' "$(field EVIDENCE)" "$root/evidence.json" | sha256sum --check --strict - >/dev/null
guard=bundle-no-symlinks
[[ -z $(find "$root" -type l -print -quit) ]] || refuse 65
guard=bundle-deb-count
[[ $(find "$root" -name '*.deb' -printf '%P\n' | wc -l) -eq $(wc -l < "$work/packages.tsv") ]] || refuse 65
guard=bundle-artifact-count
[[ $(find "$root/debs" -mindepth 1 -maxdepth 1 -printf '%f\n' | wc -l) == $(wc -l < "$work/packages.tsv") ]] || refuse 65
# Validate every byte/identity before any privileged installation, including unknown extra artifacts.
stage=artifact_verify guard=artifact-identity
debs=()
while IFS=$'\t' read -r name version arch hash bytes filename; do
    artifact="$root/debs/$filename"
    guard=artifact-path-size
    [[ -f $artifact && ! -L $artifact && $(stat -c %s "$artifact") == "$bytes" ]] || refuse 65
    guard=artifact-checksum
    printf '%s  %s\n' "$hash" "$artifact" | sha256sum --check --strict - >/dev/null
    guard=artifact-package-version-architecture
    [[ $(dpkg-deb --field "$artifact" Package) == "$name" && $(dpkg-deb --field "$artifact" Version) == "$version" && $(dpkg-deb --field "$artifact" Architecture) == "$arch" ]] || refuse 65
    # New packages only; no image base reinstall or upgrade is authorized.
    guard=new-package-only
    if dpkg-query --show --showformat='${Status}' "$name" 2>/dev/null | grep -qx 'install ok installed'; then refuse 65; fi
    debs+=("$artifact")
done < "$work/packages.tsv"
guard=native-architecture
[[ $(dpkg --print-architecture) == amd64 ]] || refuse 65
# Empty APT sources/config/hooks; dpkg/status is this IMAGE, never the collector host.
stage=apt_prepare guard=isolated-configuration
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
stage=plan_simulate guard=offline-native-resolver
apt-get --simulate --no-download --no-upgrade --no-remove --no-install-recommends install "${debs[@]}" > "$work/plan.log"
# Reject extra resolution, version changes and removals before invoking installation.
stage=plan_validate guard=plan-byte-bound
[[ $(stat -c %s "$work/plan.log") -le 4194304 ]] || refuse 65
guard=exact-new-install-plan
validate_plan "$work/packages.tsv" "$work/plan.log"
# Bind base package versions before native configuration, then compare exact full set.
stage=package_snapshot guard=full-package-set
dpkg-query --show --showformat='${Package}\t${Version}\t${Architecture}\t${Status}\n' | sort > "$work/packages-before.tsv"
stage=capacity guard=installed-space-bound
installed_kib=$(awk -F '\t' '$1=="PACKAGE" {n+=$8} END {printf "%.0f",n}' "$root/closure.lock")
free_kib=$(df -Pk / | awk 'NR==2 {print $4}')
[[ $installed_kib =~ ^[0-9]+$ && $free_kib =~ ^[0-9]+$ && $installed_kib -lt $free_kib ]] || refuse 65
# Native package resolver preserves Pre-Depends/configuration order; never force dependencies.
stage=policy_prepare guard=service-policy-original
if [[ -e "$policy_path" || -L "$policy_path" ]]; then
    [[ -f "$policy_path" && ! -L "$policy_path" ]] || refuse 65
    mv "$policy_path" "$work/policy-rc.d.saved"
    policy_saved=1
fi
policy_created=1 # Track even a partially written replacement for EXIT restoration.
printf '#!/bin/sh\nexit 101\n' > "$policy_path"
chmod 755 "$policy_path"
stage=install guard=offline-native-install
apt-get --yes --no-download --no-upgrade --no-remove --no-install-recommends install "${debs[@]}"
stage=installed_verify guard=installed-package-identity
while IFS=$'\t' read -r name version arch _hash _bytes _filename; do
    guard=installed-package-identity
    [[ $(dpkg-query --show --showformat='${Version}' "$name") == "$version" && $(dpkg-query --show --showformat='${Architecture}' "$name") == "$arch" && $(dpkg-query --show --showformat='${Status}' "$name") == 'install ok installed' ]] || refuse 65
    guard=installed-package-bytes
    [[ -z $(dpkg --verify "$name") ]] || refuse 65
done < "$work/packages.tsv"
cp "$work/packages-before.tsv" "$work/packages-expected.tsv"
stage=installed_verify guard=installed-package-identity
while IFS=$'\t' read -r name version arch _hash _bytes _filename; do
    printf '%s\t%s\t%s\tinstall ok installed\n' "$name" "$version" "$arch" >> "$work/packages-expected.tsv"
done < "$work/packages.tsv"
sort "$work/packages-expected.tsv" > "$work/packages-expected-sorted.tsv"
stage=package_snapshot guard=full-package-set
dpkg-query --show --showformat='${Package}\t${Version}\t${Architecture}\t${Status}\n' | sort > "$work/packages-after.tsv"
stage=installed_verify guard=full-package-set-equality
cmp "$work/packages-expected-sorted.tsv" "$work/packages-after.tsv"
stage=runtime_helpers guard=python-imports-WNOWAIT
/usr/bin/python3 -I -c 'import hashlib,json,pathlib,secrets,socket,subprocess,tempfile,unittest,os; assert hasattr(os,"WNOWAIT")'
# Restore service policy successfully BEFORE recording any completed installation.
stage=policy_restore guard=service-policy-restore
cleanup
trap - EXIT
stage=finalize guard=private-work-remove
rm -rf -- "$work"
# Retain lock, packages, signed evidence and this verification result in the image.
guard=completion-marker
printf '%s\n' "$expected" > "$root/installed.lock.sha256"
