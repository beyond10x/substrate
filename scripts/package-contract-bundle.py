#!/usr/bin/env python3
"""Package a substrate-wire contract bundle as a deterministic OCI image layout.

    python3 scripts/package-contract-bundle.py 0.4.0 --out <dir>

Reads `contracts/substrate-wire/<version>/` READ-ONLY and writes an OCI Image
Layout (`oci-layout`, `index.json`, `blobs/sha256/<hex>`) at `--out`. It never
writes into `contracts/` and never rewrites a bundle byte: AGENTS.md invariant 6
freezes every released bundle directory, so the packager reads the bundle and
the bundle learns nothing about the packager. Publishing and signing the layout
is separate release work and is not done here.

## Layout choice

One manifest, `application/vnd.oci.image.manifest.v1+json`:

* **config = `bundle.json`, verbatim.** The config blob is the bundle's own
  `bundle.json` bytes, byte-identical to `contracts/substrate-wire/<version>/bundle.json`
  (nothing is added, reordered or re-rendered), with media type
  `application/vnd.b10x.substrate-wire.bundle.v1+json`. Per OCI image-spec 1.1 a
  manifest with no explicit `artifactType` takes its artifact type from
  `config.mediaType`, so that media type is also the artifact type. This is what
  `docs/design/07-specification-and-conformance.md` § 1 asks for: the outer
  manifest digest pins `bundle.json`, which in turn carries the media type, byte
  length and digest of every other bundle path — no recursive self-hash, every
  distributed byte covered.
* **One layer per bundle file**, not one tar layer. `docs/design/07` wants every
  per-file digest visible, and a file-per-layer manifest makes each layer
  descriptor's digest equal to the `sha256` that `bundle.json` already lists for
  that path, so the two can be compared without unpacking anything. Each layer
  carries the file's declared media type from `bundle.json` (`application/json`
  today) and one annotation, `org.opencontainers.image.title`, holding the
  bundle-relative POSIX path — the ORAS convention for reassembling a file tree.
  `bundle.json` itself is the config and is not repeated as a layer.
  (`contracts/substrate-wire/<version>/packaging.json` describes a `posix-tar`
  archive; that authority governs the *source tarball* release blocker, not this
  OCI layout, which distributes the same bytes with per-file digests exposed.)

## Determinism

Two runs over identical bundle bytes produce byte-identical output:

* paths sorted UTF-8 bytewise (`packaging.json.archive.path_order`);
* no timestamps, no build host, no absolute paths, no environment anywhere in
  the emitted JSON;
* canonical JSON for every emitted document: sorted keys, two-space indent, no
  trailing whitespace, one trailing `\n` — the same form the checked-in bundle
  JSON uses;
* fixed annotations only — `org.opencontainers.image.version` = the bundle
  version and `dev.b10x.contract.status` = `development`, on both the manifest
  and the index entry (publication does not make a development bundle stable);
  the index entry additionally carries `org.opencontainers.image.ref.name` = the
  version, the layout's tag pointer, which is derived from the version and adds
  no new input;
* blobs are written 0644 and directories 0755, and only referenced blobs exist.

## Refusals (exit 2)

* `--out` inside any `contracts/` tree, or a parent of one;
* `--out` non-empty without `--force`; with `--force`, `--out` must contain
  nothing but a previous layout (`oci-layout`, `index.json`, `blobs`);
* an unknown `<version>`, a symlink inside the bundle, or a bundle whose file
  set disagrees with `bundle.json`'s `files` list.

Byte-level agreement between `bundle.json` and the files it lists is
`scripts/check-contract-bundle-<version>.py`'s job, not this script's: a
disagreement is reported on stderr and the descriptors follow the actual bytes,
so a changed byte always changes the manifest digest.

Prints the manifest digest on stdout on success.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONTRACTS = ROOT / "contracts"
BUNDLE_NAME = "substrate-wire"

CONFIG_MEDIA_TYPE = "application/vnd.b10x.substrate-wire.bundle.v1+json"
MANIFEST_MEDIA_TYPE = "application/vnd.oci.image.manifest.v1+json"
INDEX_MEDIA_TYPE = "application/vnd.oci.image.index.v1+json"
TITLE_ANNOTATION = "org.opencontainers.image.title"
VERSION_ANNOTATION = "org.opencontainers.image.version"
REF_ANNOTATION = "org.opencontainers.image.ref.name"
STATUS_ANNOTATION = "dev.b10x.contract.status"
BUNDLE_STATUS = "development"
LAYOUT_ENTRIES = frozenset({"oci-layout", "index.json", "blobs"})

FILE_MODE = 0o644
DIRECTORY_MODE = 0o755


class Refusal(Exception):
    """A named refusal: the script declines and changes nothing."""


def canonical_json(value: object) -> bytes:
    """Sorted keys, two-space indent, no trailing whitespace, one final newline."""
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def is_within(path: Path, ancestor: Path) -> bool:
    return path == ancestor or ancestor in path.parents


def resolve_out(raw: str, contracts_roots: list[Path]) -> Path:
    out = Path(raw).expanduser().resolve()
    for root in contracts_roots:
        if is_within(out, root):
            raise Refusal(
                f"refusing to write into contracts/: {out} is inside {root} "
                "(AGENTS.md invariant 6 — a released bundle directory is immutable)"
            )
        if is_within(root, out):
            raise Refusal(
                f"refusing to write to {out}: it contains the contracts/ tree at {root}"
            )
    return out


def prepare_out(out: Path, force: bool) -> None:
    if out.exists() and not out.is_dir():
        raise Refusal(f"refusing to write to {out}: not a directory")
    if out.is_dir():
        entries = sorted(entry.name for entry in out.iterdir())
        if entries and not force:
            raise Refusal(
                f"refusing to overwrite non-empty {out} "
                f"({len(entries)} entries); pass --force"
            )
        foreign = [name for name in entries if name not in LAYOUT_ENTRIES]
        if foreign:
            raise Refusal(
                f"refusing to --force {out}: it holds entries that are not part of "
                f"an OCI image layout ({', '.join(foreign)})"
            )
        for name in entries:
            target = out / name
            shutil.rmtree(target) if target.is_dir() else target.unlink()
    out.mkdir(parents=True, exist_ok=True)
    out.chmod(DIRECTORY_MODE)


def bundle_files(bundle: Path) -> list[str]:
    """Every regular file under the bundle, bundle-relative, sorted bytewise."""
    paths: list[str] = []
    for path in bundle.rglob("*"):
        if path.is_symlink():
            raise Refusal(
                f"refusing to package {path.relative_to(bundle).as_posix()}: symlink"
            )
        if path.is_file():
            paths.append(path.relative_to(bundle).as_posix())
    return sorted(paths, key=lambda name: name.encode("utf-8"))


def write_blob(out: Path, data: bytes) -> str:
    digest = sha256_hex(data)
    blob = out / "blobs" / "sha256" / digest
    blob.parent.mkdir(parents=True, exist_ok=True)
    blob.parent.chmod(DIRECTORY_MODE)
    (blob.parent.parent).chmod(DIRECTORY_MODE)
    blob.write_bytes(data)
    blob.chmod(FILE_MODE)
    return f"sha256:{digest}"


def package(version: str, contracts_root: Path, out: Path, force: bool) -> str:
    bundle = contracts_root / BUNDLE_NAME / version
    if not bundle.is_dir():
        raise Refusal(f"no bundle at {bundle}: unknown version {version}")

    manifest_json = bundle / "bundle.json"
    if not manifest_json.is_file():
        raise Refusal(f"no bundle.json in {bundle}")
    config_bytes = manifest_json.read_bytes()
    try:
        declared = json.loads(config_bytes)
    except json.JSONDecodeError as error:
        raise Refusal(f"{manifest_json} is not JSON: {error}") from error

    entries = declared.get("files")
    if not isinstance(entries, list) or not entries:
        raise Refusal(f"{manifest_json} lists no files")
    listed: dict[str, dict] = {}
    for entry in entries:
        path = entry.get("path")
        media_type = entry.get("media_type")
        if not isinstance(path, str) or not isinstance(media_type, str):
            raise Refusal(f"{manifest_json}: files entry without path/media_type")
        listed[path] = entry

    present = [path for path in bundle_files(bundle) if path != "bundle.json"]
    missing = sorted(set(listed) - set(present))
    extra = sorted(set(present) - set(listed))
    if missing or extra:
        raise Refusal(
            f"{manifest_json} does not describe {bundle}: "
            f"listed-but-absent={missing or '[]'} present-but-unlisted={extra or '[]'}"
        )

    prepare_out(out, force)

    layers: list[dict] = []
    disagreeing: list[str] = []
    for path in present:
        entry = listed[path]
        data = (bundle / path).read_bytes()
        digest = write_blob(out, data)
        if entry.get("sha256") != digest.removeprefix("sha256:") or entry.get(
            "byte_length"
        ) != len(data):
            disagreeing.append(path)
        layers.append(
            {
                "annotations": {TITLE_ANNOTATION: path},
                "digest": digest,
                "mediaType": entry["media_type"],
                "size": len(data),
            }
        )

    annotations = {STATUS_ANNOTATION: BUNDLE_STATUS, VERSION_ANNOTATION: version}
    manifest = {
        "annotations": annotations,
        "config": {
            "digest": write_blob(out, config_bytes),
            "mediaType": CONFIG_MEDIA_TYPE,
            "size": len(config_bytes),
        },
        "layers": layers,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "schemaVersion": 2,
    }
    manifest_bytes = canonical_json(manifest)
    manifest_digest = write_blob(out, manifest_bytes)

    index = {
        "manifests": [
            {
                "annotations": {**annotations, REF_ANNOTATION: version},
                "digest": manifest_digest,
                "mediaType": MANIFEST_MEDIA_TYPE,
                "size": len(manifest_bytes),
            }
        ],
        "mediaType": INDEX_MEDIA_TYPE,
        "schemaVersion": 2,
    }
    for name, data in (
        ("oci-layout", canonical_json({"imageLayoutVersion": "1.0.0"})),
        ("index.json", canonical_json(index)),
    ):
        target = out / name
        target.write_bytes(data)
        target.chmod(FILE_MODE)

    if disagreeing:
        print(
            f"warning: bundle.json disagrees with the bytes of "
            f"{len(disagreeing)} path(s): {', '.join(disagreeing[:5])}"
            f"{' …' if len(disagreeing) > 5 else ''}; descriptors follow the bytes. "
            f"scripts/check-contract-bundle-{version}.py is the authority.",
            file=sys.stderr,
        )
    return manifest_digest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Package a substrate-wire contract bundle as a deterministic "
        "OCI image layout.",
    )
    parser.add_argument("version", help="bundle version, e.g. 0.4.0")
    parser.add_argument(
        "--out", required=True, help="output directory for the OCI image layout"
    )
    parser.add_argument(
        "--contracts-root",
        default=str(DEFAULT_CONTRACTS),
        help="contracts/ tree to read (default: this repository's; tests point it "
        "at a copy so the checked-in bundle is never touched)",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="overwrite a non-empty --out that holds a previous layout",
    )
    args = parser.parse_args(argv)

    try:
        contracts_root = Path(args.contracts_root).expanduser().resolve()
        out = resolve_out(args.out, [contracts_root, DEFAULT_CONTRACTS.resolve()])
        digest = package(args.version, contracts_root, out, args.force)
    except Refusal as refusal:
        print(f"package-contract-bundle: {refusal}", file=sys.stderr)
        return 2
    print(digest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
