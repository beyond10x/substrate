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
* **One final layer holding the declared source archive**, appended after the
  per-file layers so no existing layer descriptor moves.
  `contracts/substrate-wire/<version>/packaging.json` declares the form the
  bundle is distributed in — `format: posix-tar`, `compression: none`, `uid`/`gid`
  0, empty `owner_name`/`group_name`, `mode: files-0644-directories-0755`,
  `path_order: utf8-bytewise`, `source_date_epoch: source-commit-author-seconds`
  — and this layer *is* that archive, built exactly to that declaration (see
  *The source archive* below). Media type
  `application/vnd.b10x.substrate-wire.bundle.tar`, annotation
  `org.opencontainers.image.title` = `<version>.tar`. The type is deliberately
  **not** `application/vnd.oci.image.layer.v1.tar`: that type means "a filesystem
  layer of a runnable image", and this manifest is an artifact (its config is
  `bundle.json`, its other layers are single JSON documents), so claiming rootfs
  semantics would invite a runtime or `oras pull` to union it over the per-file
  layers instead of materialising it as one file. A vendor type beside the vendor
  config type keeps the manifest honestly an artifact, and the `title` annotation
  makes `oras pull` write it out as `<version>.tar`.

## The source archive

`packaging.json.archive` is the specification; every field is honoured literally:

* **ustar** (`tarfile.USTAR_FORMAT`) — POSIX.1-1988, the tar format Python's
  stdlib writes with no variable bytes: fixed 512-byte headers, no pax extended
  headers, no GNU sparse or long-name records, no per-archive globals. Every
  bundle path fits the 100-byte `name` field (longest today is 58 bytes), so
  nothing needs the `prefix` split and nothing needs pax; a path that did not fit
  is a refusal, not a silent format upgrade.
* **Directory entries are included**, each 0755, ahead of their contents.
  `mode: files-0644-directories-0755` declares a directory mode, and a directory
  mode is only meaningful in an archive that carries directory entries, so this
  one carries them — an empty bundle directory would otherwise vanish on
  extraction.
* `uid` = `gid` = 0, `uname` = `gname` = `""`, files 0644, directories 0755 —
  no build account leaks into the bytes.
* every `mtime` = SOURCE_DATE_EPOCH = the author seconds of the last commit
  touching `contracts/substrate-wire/<version>/` (`git log -1 --format=%at`),
  printed on stdout. `--source-date-epoch <int>` overrides it for tests and for
  trees with no git; with neither, the script refuses rather than reaching for
  the clock.
* entry order is the same UTF-8 bytewise path order as everything else here
  (`path_order`), which also puts each directory ahead of its contents.
* **no compression** (`compression: none`): the layer is the tar itself.

The archive holds `bundle.json` too — it is the source form of the whole
directory, not of the manifest's layer set — so extracting it reproduces
`contracts/substrate-wire/<version>/` byte for byte.

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
* blobs are written 0644 and directories 0755, and only referenced blobs exist;
* the source archive takes every timestamp from SOURCE_DATE_EPOCH and every
  identity field from `packaging.json`, so it never sees a clock, a uid or a
  filesystem mode.

## Refusals (exit 2)

* `--out` inside any `contracts/` tree, or a parent of one;
* `--out` non-empty without `--force`; with `--force`, `--out` must contain
  nothing but a previous layout (`oci-layout`, `index.json`, `blobs`);
* an unknown `<version>`, a symlink inside the bundle, or a bundle whose file
  set disagrees with `bundle.json`'s `files` list;
* no SOURCE_DATE_EPOCH: no `--source-date-epoch` and no git commit dating
  `contracts/substrate-wire/<version>/`;
* a bundle path too long for a ustar header.

Byte-level agreement between `bundle.json` and the files it lists is
`scripts/check-contract-bundle-<version>.py`'s job, not this script's: a
disagreement is reported on stderr and the descriptors follow the actual bytes,
so a changed byte always changes the manifest digest.

On success one line goes to stdout: the manifest digest, then the source
archive's digest, its byte length and the SOURCE_DATE_EPOCH it was built with.
The digest is still the first whitespace-separated field.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONTRACTS = ROOT / "contracts"
BUNDLE_NAME = "substrate-wire"

CONFIG_MEDIA_TYPE = "application/vnd.b10x.substrate-wire.bundle.v1+json"
ARCHIVE_MEDIA_TYPE = "application/vnd.b10x.substrate-wire.bundle.tar"
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

# packaging.json § archive: the tar is ustar, uncompressed, ownerless.
ARCHIVE_FORMAT = tarfile.USTAR_FORMAT
ARCHIVE_UID = 0
ARCHIVE_GID = 0
ARCHIVE_OWNER_NAME = ""
ARCHIVE_GROUP_NAME = ""


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


def bundle_tree(bundle: Path) -> list[tuple[str, bool]]:
    """Every directory and regular file, bundle-relative, sorted UTF-8 bytewise.

    The flag is True for a directory. Bytewise order over the slash-free path
    puts a directory immediately ahead of everything inside it, so this one order
    serves both the layer list and the tar (`packaging.json.archive.path_order`).
    """
    entries: list[tuple[str, bool]] = []
    for path in bundle.rglob("*"):
        relative = path.relative_to(bundle).as_posix()
        if path.is_symlink():
            raise Refusal(f"refusing to package {relative}: symlink")
        if path.is_dir():
            entries.append((relative, True))
        elif path.is_file():
            entries.append((relative, False))
        else:
            raise Refusal(
                f"refusing to package {relative}: not a regular file or directory"
            )
    return sorted(entries, key=lambda entry: entry[0].encode("utf-8"))


def bundle_files(bundle: Path) -> list[str]:
    """Every regular file under the bundle, bundle-relative, sorted bytewise."""
    return [path for path, is_directory in bundle_tree(bundle) if not is_directory]


def source_date_epoch(bundle: Path, override: int | None) -> int:
    """SOURCE_DATE_EPOCH: the override, else the bundle's source commit.

    `packaging.json.archive.source_date_epoch` is
    `source-commit-author-seconds`: the author time of the last commit that
    touched the bundle directory. No commit and no override is a refusal — the
    clock is never an input.
    """
    if override is not None:
        if override < 0:
            raise Refusal(f"--source-date-epoch must not be negative: {override}")
        return override
    command = ["git", "-C", str(bundle), "log", "-1", "--format=%at", "--", "."]
    try:
        result = subprocess.run(command, capture_output=True, text=True, check=False)
    except OSError as error:
        raise Refusal(
            f"cannot run git to date {bundle} ({error}); "
            "pass --source-date-epoch <int>"
        ) from error
    seconds = result.stdout.strip()
    if result.returncode != 0 or not seconds:
        detail = result.stderr.strip() or "no commit touches this directory"
        raise Refusal(
            f"no source commit dates {bundle} ({detail}); "
            "pass --source-date-epoch <int>"
        )
    try:
        return int(seconds)
    except ValueError as error:
        raise Refusal(f"git returned a non-integer author time {seconds!r}") from error


def build_archive(bundle: Path, entries: list[tuple[str, bool]], epoch: int) -> bytes:
    """The `posix-tar` source archive `packaging.json` declares, as bytes."""
    buffer = io.BytesIO()
    try:
        with tarfile.open(fileobj=buffer, mode="w", format=ARCHIVE_FORMAT) as archive:
            for path, is_directory in entries:
                info = tarfile.TarInfo(path)
                info.uid = ARCHIVE_UID
                info.gid = ARCHIVE_GID
                info.uname = ARCHIVE_OWNER_NAME
                info.gname = ARCHIVE_GROUP_NAME
                info.mtime = epoch
                if is_directory:
                    info.type = tarfile.DIRTYPE
                    info.mode = DIRECTORY_MODE
                    info.size = 0
                    archive.addfile(info)
                    continue
                data = (bundle / path).read_bytes()
                info.type = tarfile.REGTYPE
                info.mode = FILE_MODE
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
    except ValueError as error:
        raise Refusal(
            f"cannot write a ustar archive of {bundle}: {error} "
            "(packaging.json declares format posix-tar; a path that no longer "
            "fits a ustar header is a bundle decision, not a format fallback)"
        ) from error
    return buffer.getvalue()


def write_blob(out: Path, data: bytes) -> str:
    digest = sha256_hex(data)
    blob = out / "blobs" / "sha256" / digest
    blob.parent.mkdir(parents=True, exist_ok=True)
    blob.parent.chmod(DIRECTORY_MODE)
    (blob.parent.parent).chmod(DIRECTORY_MODE)
    blob.write_bytes(data)
    blob.chmod(FILE_MODE)
    return f"sha256:{digest}"


def package(
    version: str,
    contracts_root: Path,
    out: Path,
    force: bool,
    epoch_override: int | None = None,
) -> dict[str, object]:
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

    tree = bundle_tree(bundle)
    present = [
        path
        for path, is_directory in tree
        if not is_directory and path != "bundle.json"
    ]
    missing = sorted(set(listed) - set(present))
    extra = sorted(set(present) - set(listed))
    if missing or extra:
        raise Refusal(
            f"{manifest_json} does not describe {bundle}: "
            f"listed-but-absent={missing or '[]'} present-but-unlisted={extra or '[]'}"
        )

    # Resolved before anything is written: no SOURCE_DATE_EPOCH is a refusal, and
    # a refusal must leave --force's target directory as it found it.
    epoch = source_date_epoch(bundle, epoch_override)
    archive_bytes = build_archive(bundle, tree, epoch)

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

    # The declared source archive, last: appending it leaves every per-file layer
    # descriptor above exactly where it was.
    archive_digest = write_blob(out, archive_bytes)
    layers.append(
        {
            "annotations": {TITLE_ANNOTATION: f"{version}.tar"},
            "digest": archive_digest,
            "mediaType": ARCHIVE_MEDIA_TYPE,
            "size": len(archive_bytes),
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
    return {
        "manifest": manifest_digest,
        "archive": archive_digest,
        "archive_bytes": len(archive_bytes),
        "source_date_epoch": epoch,
    }


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
    parser.add_argument(
        "--source-date-epoch",
        type=int,
        default=None,
        metavar="SECONDS",
        help="every archive mtime, in seconds since the epoch (default: the "
        "author time of the last commit touching the bundle directory; without "
        "either the script refuses rather than reading the clock)",
    )
    args = parser.parse_args(argv)

    try:
        contracts_root = Path(args.contracts_root).expanduser().resolve()
        out = resolve_out(args.out, [contracts_root, DEFAULT_CONTRACTS.resolve()])
        result = package(
            args.version, contracts_root, out, args.force, args.source_date_epoch
        )
    except Refusal as refusal:
        print(f"package-contract-bundle: {refusal}", file=sys.stderr)
        return 2
    print(
        f"{result['manifest']} archive={result['archive']} "
        f"archive_bytes={result['archive_bytes']} "
        f"source_date_epoch={result['source_date_epoch']}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
