#!/usr/bin/env python3
"""Package a new immutable candidate kit; never compile, pull, activate or relabel old proofs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile

SOURCE = PROVIDER_SOURCE = APP = DB = DB_ARCHIVE = DB_MANIFEST = BASE_REGISTRY_DIGEST = None
PINNED = {}


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def oci(path, image):
    # Read named members only; no archive extraction or archive-controlled paths.
    with tarfile.open(path, "r:*") as archive:
        members = {}
        for item in archive.getmembers():
            name = item.name.removeprefix("./")
            if name in members or item.issym() or item.islnk():
                raise ValueError("ambiguous OCI archive")
            members[name] = item
        def read(name):
            item = members[name]
            if not item.isfile():
                raise ValueError("OCI blob is not regular")
            return archive.extractfile(item).read()
        def blob(descriptor):
            digest = descriptor["digest"]
            if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
                raise ValueError("unsupported OCI digest")
            data = read("blobs/sha256/" + digest[7:])
            if len(data) != descriptor["size"] or hashlib.sha256(data).hexdigest() != digest[7:]:
                raise ValueError("OCI content digest mismatch")
            return data
        index = json.loads(read("index.json"))
        if len(index["manifests"]) != 1:
            raise ValueError("one exact OCI image required")
        manifest = json.loads(blob(index["manifests"][0]))
        config = json.loads(blob(manifest["config"]))
        if manifest["config"]["digest"] != "sha256:" + image:
            raise ValueError("unexpected image ID")
        if config["architecture"] != "amd64" or config["os"] != "linux":
            raise ValueError("unexpected platform")
        for layer in manifest["layers"]:
            blob(layer)
        if image == APP and (config["config"]["User"] != "podmesh-node:podmesh-node"
                             or config["config"]["Entrypoint"] != ["/usr/lib/podmesh-node/podmeshd"]):
            raise ValueError("application boundary differs")
        return index["manifests"][0]["digest"]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--candidate-lock", required=True, type=Path,
                   help="explicit immutable source/provider/database/base-registry lock for this new candidate")
    p.add_argument("--kit", required=True, type=Path, help="directory with three manifest-selected payloads")
    p.add_argument("--manifest", required=True, type=Path)
    p.add_argument("--database-oci", required=True, type=Path, help="export of exact cached DB base; never a pull")
    p.add_argument("--recipe-revision", required=True)
    p.add_argument("--output", required=True, type=Path, help="new absent output directory")
    a = p.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", a.recipe_revision):
        raise ValueError("full recipe revision required")
    global SOURCE, PROVIDER_SOURCE, APP, DB, DB_ARCHIVE, DB_MANIFEST, BASE_REGISTRY_DIGEST, PINNED
    lock = json.loads(a.candidate_lock.read_text())
    expected = {"source_revision", "provider_source_revision", "database_image", "database_archive_sha256",
                "database_manifest_digest", "base_registry_digest"}
    if not isinstance(lock, dict) or set(lock) != expected:
        raise ValueError("unknown or incomplete candidate lock")
    for field in ("source_revision", "provider_source_revision"):
        if not isinstance(lock[field], str) or not re.fullmatch(r"[0-9a-f]{40}", lock[field]):
            raise ValueError("immutable full source/provider revision required")
    for field in ("database_image", "database_manifest_digest", "base_registry_digest"):
        if not isinstance(lock[field], str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", lock[field]):
            raise ValueError("immutable full database/base digest required")
    if not re.fullmatch(r"[0-9a-f]{64}", lock["database_archive_sha256"]):
        raise ValueError("database archive checksum required")
    SOURCE, PROVIDER_SOURCE = lock["source_revision"], lock["provider_source_revision"]
    DB, DB_ARCHIVE = lock["database_image"][7:], lock["database_archive_sha256"]
    DB_MANIFEST, BASE_REGISTRY_DIGEST = lock["database_manifest_digest"], lock["base_registry_digest"]
    m = json.loads(a.manifest.read_text())
    if not isinstance(m["oci"]["imageId"], str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", m["oci"]["imageId"]):
        raise ValueError("immutable application image ID required")
    if m["provider_source_revision"] != PROVIDER_SOURCE:
        raise ValueError("provider source manifest differs from candidate lock")
    APP = m["oci"]["imageId"][7:]
    PINNED = {name: m["artifacts"][name]["sha256"] for name in
              ("podmeshd", "podmesh-host-adapter", "application.oci.tar")}
    if any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value) for value in PINNED.values()):
        raise ValueError("candidate payload checksums required")
    if m["source"]["revision"] != SOURCE or m["oci"]["imageId"] != "sha256:" + APP:
        raise ValueError("wrong source manifest")
    for name, digest in PINNED.items():
        if sha(a.kit / name) != digest or m["artifacts"][name]["sha256"] != digest:
            raise ValueError("wrong immutable payload")
    app_manifest = oci(a.kit / "application.oci.tar", APP)
    if app_manifest != m["oci"]["manifestDigest"]:
        raise ValueError("wrong application OCI manifest")
    if sha(a.database_oci) != DB_ARCHIVE:
        raise ValueError("wrong exact database OCI archive")
    db_manifest = oci(a.database_oci, DB)
    if db_manifest != DB_MANIFEST:
        raise ValueError("wrong exact database OCI manifest")
    bundle = SOURCE[:12] + "-" + a.recipe_revision[:12]
    a.output.mkdir(mode=0o700)  # Refuse existing destinations, including symlinks.
    stage = a.output / "stage"
    payload = stage / "usr/lib/podmesh-node-private" / bundle
    payload.mkdir(parents=True)
    source_dir = Path(__file__).resolve().parent
    for name in PINNED:
        shutil.copyfile(a.kit / name, payload / name)
    shutil.copyfile(a.database_oci, payload / "database.oci.tar")
    shutil.copyfile(a.candidate_lock, payload / "candidate-lock.json")
    shutil.copyfile(source_dir / "instance.py", payload / "instance.py")
    shutil.copyfile(source_dir / "README.md", payload / "README.md")
    shutil.copyfile(source_dir.parents[2] / "LICENSE", payload / "LICENSE")
    shutil.copyfile(source_dir.parents[2] / "NOTICE", payload / "NOTICE")
    public = {"bundle": bundle, "binary_source_revision": SOURCE,
              "provider_source_revision": PROVIDER_SOURCE,
              "recipe_revision": a.recipe_revision, "application_image": "sha256:" + APP,
              "database_image": "sha256:" + DB,
              "application_manifest": app_manifest, "database_manifest": db_manifest,
              "base_registry_digest": BASE_REGISTRY_DIGEST,
              "candidate_lock_sha256": sha(a.candidate_lock),
              "source_date_epoch": m["source"]["sourceDateEpoch"],
              "payload_sha256": {f.name: sha(f) for f in sorted(payload.iterdir())}}
    (payload / "bundle.json").write_text(json.dumps(public, indent=2, sort_keys=True) + "\n")
    for f in payload.iterdir():
        f.chmod(0o755 if f.name in ("instance.py", "podmeshd", "podmesh-host-adapter") else 0o644)
    control = stage / "DEBIAN"
    control.mkdir()
    (control / "control").write_text(
        f"Package: podmesh-node-private-{bundle}\nVersion: 0.1.0\nArchitecture: amd64\n"
        "Maintainer: PodMesh maintainers\nDepends: python3 (>= 3.11), podman, systemd, libc6 (>= 2.34), libgcc-s1\n"
        "Description: Immutable private node and separate scoped root provider\n"
        " Installation supplies payloads only. Explicit root operator prepares a new instance.\n")
    # No maintscripts, accounts, credentials, service files or automatic activation.
    for path in sorted(stage.rglob("*")) + [stage]:
        os.utime(path, (public["source_date_epoch"], public["source_date_epoch"]))
    deb = a.output / f"podmesh-node-private-{bundle}_0.1.0_amd64.deb"
    subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(stage), str(deb)],
                   check=True, env={**os.environ, "SOURCE_DATE_EPOCH": str(public["source_date_epoch"]), "TZ": "UTC"})
    (a.output / "delivery-manifest.json").write_text(json.dumps(
        {**public, "deb": deb.name, "deb_sha256": sha(deb), "deb_bytes": deb.stat().st_size},
        indent=2, sort_keys=True) + "\n")
    print(deb)


if __name__ == "__main__":
    main()
