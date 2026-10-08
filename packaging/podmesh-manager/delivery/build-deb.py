#!/usr/bin/env python3
"""Package already-built immutable manager bytes; never compile, pull or activate."""
import argparse
import hashlib
import gzip
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile

SOURCE = "6ba91890a8ff24230064ce3c709f563c85f4370e"
APP = "3098a589d3a7c946d1ded0777b5ab273cdfb8320c46f7c9e65af1a2a43af6bfc"
APP_MANIFEST = "sha256:73ff161f7183acb8f5da4a2f4fd0d89a8b5d2b6f22d74a5ad5c76226f2f91c43"
DB = "53ef799caed285438d88678b529b4ff24406d4788f47ab8a74bb2212c707899a"
DB_ARCHIVE = "087a378584383840073ebc347e383229f9765572785b7b4cbf09f22a274b4efb"
DB_MANIFEST = "sha256:4ca2b8d82f602cefca23a2f270a591c6b27e27d81d5e32ca8ecdf92cdad50a15"
PINNED = {
    "podmesh-managerd": "71bc510ac369a271141ec25d98867563c0da77324a358902e3136fb2c324ad29",
    "application.oci.tar": "17da3325c3a2a72d6f7fdf1485a5a1c37803a979c670938abdf6b4629fe900d7",
}


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
        diffs = config["rootfs"]["diff_ids"]
        if len(diffs) != len(manifest["layers"]):
            raise ValueError("OCI rootfs layer count differs")
        for layer, expected in zip(manifest["layers"], diffs):
            compressed = blob(layer)
            if layer["mediaType"].endswith("+gzip"):
                stream = gzip.GzipFile(fileobj=io.BytesIO(compressed))
            elif layer["mediaType"] == "application/vnd.oci.image.layer.v1.tar":
                stream = io.BytesIO(compressed)
            else:
                raise ValueError("unsupported OCI layer compression")
            with stream:
                actual = "sha256:" + hashlib.file_digest(stream,"sha256").hexdigest()
            if actual != expected:
                raise ValueError("OCI rootfs diff ID differs")
        if image == APP and (config["config"]["User"] != "podmesh-manager:podmesh-manager"
                             or config["config"]["Entrypoint"] != ["/usr/lib/podmesh-manager/podmesh-managerd"]):
            raise ValueError("application boundary differs")
        return index["manifests"][0]["digest"]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--kit", required=True, type=Path, help="directory with exact manager executable and application OCI archive")
    p.add_argument("--manifest", required=True, type=Path)
    p.add_argument("--database-oci", required=True, type=Path, help="export of exact cached DB base; never a pull")
    p.add_argument("--recipe-revision", required=True)
    p.add_argument("--output", required=True, type=Path, help="new absent output directory")
    a = p.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", a.recipe_revision):
        raise ValueError("full recipe revision required")
    m = json.loads(a.manifest.read_text())
    if m["source"]["revision"] != SOURCE or m["oci"]["imageId"] != "sha256:" + APP:
        raise ValueError("wrong source manifest")
    for name, digest in PINNED.items():
        if sha(a.kit / name) != digest or (m["artifacts"]["one"][name]["sha256"] if name == "podmesh-managerd" else m["oci"]["sha256"]) != digest:
            raise ValueError("wrong immutable payload")
    app_manifest = oci(a.kit / "application.oci.tar", APP)
    if app_manifest != APP_MANIFEST or app_manifest != m["oci"]["manifestDigest"]:
        raise ValueError("wrong application OCI manifest")
    if sha(a.database_oci) != DB_ARCHIVE:
        raise ValueError("wrong exact database OCI archive")
    db_manifest = oci(a.database_oci, DB)
    if db_manifest != DB_MANIFEST:
        raise ValueError("wrong exact database OCI manifest")
    bundle = SOURCE[:12] + "-" + a.recipe_revision[:12]
    a.output.mkdir(mode=0o700)  # Refuse existing destinations, including symlinks.
    stage = a.output / "stage"
    payload = stage / "usr/lib/podmesh-manager-private" / bundle
    payload.mkdir(parents=True)
    source_dir = Path(__file__).resolve().parent
    for name in PINNED:
        shutil.copyfile(a.kit / name, payload / name)
    shutil.copyfile(a.database_oci, payload / "database.oci.tar")
    shutil.copyfile(source_dir / "instance.py", payload / "instance.py")
    shutil.copyfile(source_dir / "README.md", payload / "README.md")
    shutil.copyfile(source_dir.parents[2] / "LICENSE", payload / "LICENSE")
    shutil.copyfile(source_dir.parents[2] / "NOTICE", payload / "NOTICE")
    public = {"bundle": bundle, "binary_source_revision": SOURCE,
              "recipe_revision": a.recipe_revision, "application_image": "sha256:" + APP,
              "database_image": "sha256:" + DB,
              "application_manifest": app_manifest, "database_manifest": db_manifest,
              "base_registry_digest": "sha256:93fc3fe333b6cdfb061425869c2c3a5bb2851c0c74dbebc2c940f024e7482d76",
              "source_date_epoch": m["source"]["sourceDateEpoch"],
              "payload_sha256": {f.name: sha(f) for f in sorted(payload.iterdir())}}
    (payload / "bundle.json").write_text(json.dumps(public, indent=2, sort_keys=True) + "\n")
    for f in payload.iterdir():
        f.chmod(0o755 if f.name in ("instance.py", "podmesh-managerd") else 0o644)
    control = stage / "DEBIAN"
    control.mkdir()
    (control / "control").write_text(
        f"Package: podmesh-manager-private-{bundle}\nVersion: 0.1.0\nArchitecture: amd64\n"
        "Maintainer: PodMesh maintainers\nDepends: python3 (>= 3.11), podman, systemd, iproute2, libc6 (>= 2.34), libgcc-s1\n"
        "Description: Immutable private manager and owning database\n"
        " Installation supplies payloads only. Explicit root operator prepares a new instance.\n")
    # No maintscripts, accounts, credentials, service files or automatic activation.
    for path in sorted(stage.rglob("*")) + [stage]:
        os.utime(path, (public["source_date_epoch"], public["source_date_epoch"]))
    deb = a.output / f"podmesh-manager-private-{bundle}_0.1.0_amd64.deb"
    subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(stage), str(deb)],
                   check=True, env={**os.environ, "SOURCE_DATE_EPOCH": str(public["source_date_epoch"]), "TZ": "UTC"})
    (a.output / "delivery-manifest.json").write_text(json.dumps(
        {**public, "deb": deb.name, "deb_sha256": sha(deb), "deb_bytes": deb.stat().st_size},
        indent=2, sort_keys=True) + "\n")
    print(deb)


if __name__ == "__main__":
    main()
