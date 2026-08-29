#!/usr/bin/env python3
"""Tests for the deterministic contract-bundle OCI packager.

Run: python3 scripts/test_package_contract_bundle.py

Every test that mutates bundle bytes works on a copy under $TMPDIR; nothing here
writes into contracts/ (AGENTS.md invariant 6: a released bundle directory is
immutable, and the packager reads the bundle rather than teaching it about the
packager).

`ArchiveLayerTests` holds the artifact to `packaging.json`: the layout ships the
declared `posix-tar` source archive, its headers carry what `packaging.json.archive`
declares, extracting it reproduces the bundle directory byte for byte, and
SOURCE_DATE_EPOCH moves the archive and the manifest and nothing else. A copied
bundle has no git history to date it, so those tests pin `--source-date-epoch`.
"""

from __future__ import annotations

import filecmp
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
ROOT = SCRIPTS.parent
PACKAGER = SCRIPTS / "package-contract-bundle.py"
VERSION = "0.4.0"
CONTRACTS = ROOT / "contracts"
BUNDLE = CONTRACTS / "substrate-wire" / VERSION

MANIFEST_MEDIA_TYPE = "application/vnd.oci.image.manifest.v1+json"
CONFIG_MEDIA_TYPE = "application/vnd.b10x.substrate-wire.bundle.v1+json"
ARCHIVE_MEDIA_TYPE = "application/vnd.b10x.substrate-wire.bundle.tar"
TITLE_ANNOTATION = "org.opencontainers.image.title"

# packaging.json § archive, the shape the source tar must have.
ARCHIVE_UID = 0
ARCHIVE_GID = 0
ARCHIVE_OWNER_NAME = ""
ARCHIVE_GROUP_NAME = ""
ARCHIVE_FILE_MODE = 0o644
ARCHIVE_DIRECTORY_MODE = 0o755

# A bundle copied under $TMPDIR carries no git history, so tests that read a
# scratch contracts root pin SOURCE_DATE_EPOCH instead of deriving it.
SCRATCH_EPOCH = 1700000000


def scratch_base() -> Path:
    """A scratch root that is never /tmp."""
    base = Path(os.environ.get("TMPDIR") or (Path.home() / ".cache" / "claude-tmp"))
    base.mkdir(parents=True, exist_ok=True)
    return base


def digest_of(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def blob_path(layout: Path, digest: str) -> Path:
    algorithm, _, hex_digest = digest.partition(":")
    return layout / "blobs" / algorithm / hex_digest


def read_index(layout: Path) -> dict:
    return json.loads((layout / "index.json").read_text(encoding="utf-8"))


def manifest_digest(layout: Path) -> str:
    manifests = read_index(layout)["manifests"]
    assert len(manifests) == 1, manifests
    return manifests[0]["digest"]


def read_manifest(layout: Path) -> dict:
    return json.loads(blob_path(layout, manifest_digest(layout)).read_bytes())


def archive_layers(manifest: dict) -> list[dict]:
    """Every layer that claims to be the declared posix-tar source archive."""
    return [
        layer
        for layer in manifest["layers"]
        if layer["mediaType"] == ARCHIVE_MEDIA_TYPE
    ]


def file_layers(manifest: dict) -> list[dict]:
    """The per-file layers: everything that is not the source archive."""
    return [
        layer
        for layer in manifest["layers"]
        if layer["mediaType"] != ARCHIVE_MEDIA_TYPE
    ]


def stdout_fields(stdout: str) -> dict[str, str]:
    """The packager's one stdout line: a bare digest then `key=value` fields."""
    fields = {}
    for token in stdout.split()[1:]:
        key, _, value = token.partition("=")
        fields[key] = value
    return fields


def tree_of(root: Path) -> dict[str, bytes | None]:
    """Every path under `root`, mapped to its bytes (None for a directory)."""
    return {
        path.relative_to(root).as_posix(): None if path.is_dir() else path.read_bytes()
        for path in root.rglob("*")
    }


class PackagerTestCase(unittest.TestCase):
    def scratch(self, prefix: str) -> Path:
        directory = Path(tempfile.mkdtemp(prefix=prefix, dir=scratch_base()))
        self.addCleanup(shutil.rmtree, directory, ignore_errors=True)
        return directory

    def package(
        self,
        out: Path,
        *,
        contracts_root: Path | None = None,
        version: str = VERSION,
        extra: tuple[str, ...] = (),
        expect_exit: int = 0,
        pin_epoch: bool = True,
    ) -> subprocess.CompletedProcess:
        argv = [sys.executable, str(PACKAGER), version, "--out", str(out)]
        if contracts_root is not None:
            argv += ["--contracts-root", str(contracts_root)]
            if pin_epoch and "--source-date-epoch" not in extra:
                # A scratch copy has no git history to date it, and the packager
                # refuses to reach for the clock, so pin the epoch explicitly.
                argv += ["--source-date-epoch", str(SCRATCH_EPOCH)]
        argv += list(extra)
        proc = subprocess.run(argv, capture_output=True, text=True, cwd=str(ROOT))
        self.assertEqual(
            proc.returncode,
            expect_exit,
            f"argv={argv}\nstdout={proc.stdout}\nstderr={proc.stderr}",
        )
        return proc

    def copy_bundle(self, into: Path) -> Path:
        """Copy contracts/substrate-wire/<version> under a scratch contracts root."""
        contracts_root = into / "contracts"
        target = contracts_root / "substrate-wire" / VERSION
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(BUNDLE, target)
        return contracts_root


class DeterminismTests(PackagerTestCase):
    def test_two_runs_are_byte_identical(self) -> None:
        work = self.scratch("pkg-bundle-determinism-")
        first, second = work / "first", work / "second"
        proc_one = self.package(first)
        proc_two = self.package(second)

        self.assertEqual(
            (first / "index.json").read_bytes(),
            (second / "index.json").read_bytes(),
            "index.json differs between two runs",
        )
        self.assertEqual(
            (first / "oci-layout").read_bytes(),
            (second / "oci-layout").read_bytes(),
        )
        self.assertEqual(proc_one.stdout, proc_two.stdout)
        self.assertIn(manifest_digest(first), proc_one.stdout)

        def blob_names(layout: Path) -> set[str]:
            return {
                str(path.relative_to(layout))
                for path in (layout / "blobs").rglob("*")
                if path.is_file()
            }

        self.assertEqual(blob_names(first), blob_names(second), "blob sets differ")
        for name in sorted(blob_names(first)):
            self.assertEqual(
                (first / name).read_bytes(),
                (second / name).read_bytes(),
                f"blob {name} differs between runs",
            )

    def test_one_byte_change_changes_manifest_digest(self) -> None:
        work = self.scratch("pkg-bundle-onebyte-")
        pristine_root = self.copy_bundle(work / "pristine")
        mutated_root = self.copy_bundle(work / "mutated")

        first = work / "out-pristine-1"
        again = work / "out-pristine-2"
        mutated_out = work / "out-mutated"

        self.package(first, contracts_root=pristine_root)
        self.package(again, contracts_root=pristine_root)
        baseline = manifest_digest(first)
        self.assertEqual(
            manifest_digest(again),
            baseline,
            "two runs over identical bytes must yield the same manifest digest; "
            "a digest that moves on its own cannot attribute a change to the bundle",
        )

        target = mutated_root / "substrate-wire" / VERSION / "runner.json"
        original = target.read_bytes()
        self.assertEqual(original[-1:], b"\n")
        mutated = original[:-2] + bytes([original[-2] ^ 0x20]) + original[-1:]
        self.assertEqual(len(mutated), len(original))
        self.assertEqual(sum(a != b for a, b in zip(original, mutated)), 1)
        target.write_bytes(mutated)

        self.package(mutated_out, contracts_root=mutated_root)
        self.assertNotEqual(
            manifest_digest(mutated_out),
            baseline,
            "a one-byte change in a bundle file must change the manifest digest",
        )
        self.assertEqual(
            BUNDLE.joinpath("runner.json").read_bytes(),
            original,
            "the checked-in bundle must be untouched",
        )


class ArtifactShapeTests(PackagerTestCase):
    def setUp(self) -> None:
        self.layout = self.scratch("pkg-bundle-shape-") / "layout"
        self.stdout = self.package(self.layout).stdout
        self.manifest = read_manifest(self.layout)

    def test_layout_marker_and_index(self) -> None:
        self.assertEqual(
            json.loads((self.layout / "oci-layout").read_text(encoding="utf-8")),
            {"imageLayoutVersion": "1.0.0"},
        )
        index = read_index(self.layout)
        self.assertEqual(index["schemaVersion"], 2)
        entry = index["manifests"][0]
        self.assertEqual(entry["mediaType"], MANIFEST_MEDIA_TYPE)
        self.assertEqual(
            entry["annotations"]["org.opencontainers.image.version"], VERSION
        )
        self.assertEqual(entry["annotations"]["dev.b10x.contract.status"], "development")
        manifest_blob = blob_path(self.layout, entry["digest"]).read_bytes()
        self.assertEqual(entry["size"], len(manifest_blob))
        self.assertEqual(entry["digest"], "sha256:" + digest_of(manifest_blob))
        self.assertEqual(
            self.stdout.split()[0],
            entry["digest"],
            "the manifest digest must stay the first field on stdout",
        )

    def test_config_is_the_bundle_json_verbatim(self) -> None:
        config = self.manifest["config"]
        self.assertEqual(config["mediaType"], CONFIG_MEDIA_TYPE)
        blob = blob_path(self.layout, config["digest"]).read_bytes()
        self.assertEqual(
            blob,
            (BUNDLE / "bundle.json").read_bytes(),
            "the artifact's bundle.json must be the bundle's own bytes",
        )
        self.assertEqual(config["size"], len(blob))
        self.assertEqual(config["digest"], "sha256:" + digest_of(blob))

    def test_every_bundle_json_entry_matches_the_bytes(self) -> None:
        bundle_json = json.loads(
            blob_path(self.layout, self.manifest["config"]["digest"]).read_bytes()
        )
        entries = bundle_json["files"]
        self.assertGreater(len(entries), 0)
        for entry in entries:
            with self.subTest(path=entry["path"]):
                blob = blob_path(self.layout, "sha256:" + entry["sha256"])
                self.assertTrue(blob.is_file(), f"no blob for {entry['path']}")
                data = blob.read_bytes()
                self.assertEqual(len(data), entry["byte_length"])
                self.assertEqual(digest_of(data), entry["sha256"])

    def test_layers_are_one_per_bundle_path_in_sorted_order(self) -> None:
        bundle_json = json.loads(
            blob_path(self.layout, self.manifest["config"]["digest"]).read_bytes()
        )
        entries = {entry["path"]: entry for entry in bundle_json["files"]}
        layers = file_layers(self.manifest)
        self.assertEqual(
            layers,
            self.manifest["layers"][: len(layers)],
            "the per-file layers must stay first; the archive layer is appended",
        )
        titles = [layer["annotations"][TITLE_ANNOTATION] for layer in layers]
        self.assertEqual(titles, sorted(titles), "layer order is not path-sorted")
        self.assertEqual(set(titles), set(entries), "layers do not cover bundle.json's files")
        self.assertNotIn("bundle.json", titles, "bundle.json is the config, not a layer")
        for layer, title in zip(layers, titles):
            with self.subTest(path=title):
                entry = entries[title]
                self.assertEqual(layer["digest"], "sha256:" + entry["sha256"])
                self.assertEqual(layer["size"], entry["byte_length"])
                self.assertEqual(layer["mediaType"], entry["media_type"])

    def test_blob_set_is_exactly_manifest_config_and_layers(self) -> None:
        present = {
            f"sha256:{path.name}"
            for path in (self.layout / "blobs" / "sha256").iterdir()
            if path.is_file()
        }
        expected = {manifest_digest(self.layout), self.manifest["config"]["digest"]}
        expected |= {layer["digest"] for layer in self.manifest["layers"]}
        self.assertEqual(present, expected, "the layout carries unreferenced blobs")

    def test_no_absolute_path_or_timestamp_leaks_into_the_metadata(self) -> None:
        for name in ("index.json", "oci-layout"):
            text = (self.layout / name).read_text(encoding="utf-8")
            self.assertNotIn(str(ROOT), text)
            self.assertNotIn(str(self.layout), text)
        manifest_text = blob_path(self.layout, manifest_digest(self.layout)).read_text(
            encoding="utf-8"
        )
        self.assertNotIn(str(ROOT), manifest_text)
        self.assertNotIn("created", manifest_text)


class ArchiveLayerTests(PackagerTestCase):
    """`packaging.json` declares a posix-tar source archive; it must be shipped."""

    def archive_of(self, layout: Path) -> tuple[dict, bytes]:
        """The single source-archive layer descriptor and its blob bytes."""
        layers = archive_layers(read_manifest(layout))
        self.assertEqual(
            len(layers),
            1,
            f"{layout} carries no single {ARCHIVE_MEDIA_TYPE} layer; "
            "packaging.json declares a posix-tar source archive and the "
            "artifact must ship it",
        )
        return layers[0], blob_path(layout, layers[0]["digest"]).read_bytes()

    def test_two_runs_produce_a_byte_identical_archive(self) -> None:
        work = self.scratch("pkg-bundle-archive-determinism-")
        first, second = work / "first", work / "second"
        self.package(first)
        self.package(second)

        _, one = self.archive_of(first)
        _, two = self.archive_of(second)
        self.assertEqual(one, two, "the source archive is not reproducible")
        self.assertEqual(
            manifest_digest(first),
            manifest_digest(second),
            "adding the archive must not make the manifest digest move on its own",
        )

    def test_archive_descriptor_and_stdout_agree_with_the_blob(self) -> None:
        layout = self.scratch("pkg-bundle-archive-descriptor-") / "layout"
        stdout = self.package(layout).stdout
        layer, blob = self.archive_of(layout)
        reported = stdout_fields(stdout)

        self.assertEqual(layer["annotations"][TITLE_ANNOTATION], f"{VERSION}.tar")
        self.assertEqual(layer["size"], len(blob))
        self.assertEqual(layer["digest"], "sha256:" + digest_of(blob))
        self.assertEqual(reported["archive"], layer["digest"])
        self.assertEqual(reported["archive_bytes"], str(len(blob)))
        self.assertEqual(
            read_manifest(layout)["layers"][-1],
            layer,
            "the archive must be the last layer, so no per-file descriptor moves",
        )

    def test_archive_headers_match_packaging_json(self) -> None:
        layout = self.scratch("pkg-bundle-archive-headers-") / "layout"
        stdout = self.package(layout).stdout
        epoch = int(stdout_fields(stdout)["source_date_epoch"])
        _, blob = self.archive_of(layout)

        declared = json.loads((BUNDLE / "packaging.json").read_text(encoding="utf-8"))
        archive = declared["archive"]
        self.assertEqual(archive["format"], "posix-tar")
        self.assertEqual(archive["compression"], "none")
        self.assertEqual(archive["mode"], "files-0644-directories-0755")
        self.assertEqual(archive["path_order"], "utf8-bytewise")
        self.assertEqual(archive["source_date_epoch"], "source-commit-author-seconds")

        # compression "none": the bytes open as a plain tar, and the ustar magic
        # sits in the first header. pax/GNU records would show up as extra member
        # types below.
        self.assertEqual(blob[257:265], b"ustar\x0000", "not a ustar header")
        with tarfile.open(fileobj=io.BytesIO(blob), mode="r:") as tar:
            members = tar.getmembers()

        self.assertGreater(len(members), 0)
        for member in members:
            with self.subTest(name=member.name):
                self.assertIn(
                    member.type,
                    (tarfile.REGTYPE, tarfile.DIRTYPE),
                    "only files and directories belong in the archive",
                )
                self.assertEqual(member.uid, archive["uid"])
                self.assertEqual(member.gid, archive["gid"])
                self.assertEqual(member.uname, archive["owner_name"])
                self.assertEqual(member.gname, archive["group_name"])
                self.assertEqual(member.mtime, epoch)
                self.assertEqual(
                    member.mode,
                    ARCHIVE_DIRECTORY_MODE if member.isdir() else ARCHIVE_FILE_MODE,
                )

        names = [member.name.rstrip("/") for member in members]
        self.assertEqual(
            names,
            sorted(names, key=lambda name: name.encode("utf-8")),
            "archive entries are not in UTF-8 bytewise path order",
        )
        self.assertEqual(
            set(names),
            {
                path.relative_to(BUNDLE).as_posix()
                for path in BUNDLE.rglob("*")
                if path.is_file() or path.is_dir()
            },
            "the archive is not the whole bundle directory",
        )
        self.assertIn(
            "bundle.json",
            names,
            "the archive is the source form of the directory, bundle.json included",
        )
        self.assertTrue(
            any(member.isdir() for member in members),
            "packaging.json declares a directory mode, so directories are entries",
        )

    def test_archive_extracts_to_the_bundle_byte_for_byte(self) -> None:
        work = self.scratch("pkg-bundle-archive-extract-")
        layout = work / "layout"
        self.package(layout)
        _, blob = self.archive_of(layout)

        extracted = work / "extracted"
        extracted.mkdir()
        with tarfile.open(fileobj=io.BytesIO(blob), mode="r:") as tar:
            tar.extractall(extracted, filter="data")

        self.assertEqual(
            tree_of(extracted),
            tree_of(BUNDLE),
            "extracting the archive does not reproduce the bundle byte for byte",
        )
        comparison = filecmp.dircmp(str(BUNDLE), str(extracted))
        self.assertEqual(comparison.left_only, [])
        self.assertEqual(comparison.right_only, [])
        self.assertEqual(comparison.funny_files, [])

    def test_source_date_epoch_moves_only_the_archive_and_the_manifest(self) -> None:
        work = self.scratch("pkg-bundle-archive-epoch-")
        early, late = work / "early", work / "late"
        self.package(early, extra=("--source-date-epoch", "1000000000"))
        self.package(late, extra=("--source-date-epoch", "2000000000"))

        early_layer, early_blob = self.archive_of(early)
        late_layer, late_blob = self.archive_of(late)
        self.assertNotEqual(early_blob, late_blob)
        self.assertNotEqual(
            early_layer["digest"],
            late_layer["digest"],
            "a different SOURCE_DATE_EPOCH must change the archive digest",
        )
        self.assertNotEqual(
            manifest_digest(early),
            manifest_digest(late),
            "the manifest pins the archive, so its digest must move too",
        )
        self.assertEqual(
            file_layers(read_manifest(early)),
            file_layers(read_manifest(late)),
            "no per-file layer may depend on SOURCE_DATE_EPOCH",
        )
        for layout, seconds in ((early, 1000000000), (late, 2000000000)):
            with self.subTest(epoch=seconds):
                _, blob = self.archive_of(layout)
                with tarfile.open(fileobj=io.BytesIO(blob), mode="r:") as tar:
                    self.assertEqual(
                        {member.mtime for member in tar.getmembers()}, {seconds}
                    )

    def test_default_epoch_is_the_bundle_source_commit(self) -> None:
        expected = subprocess.run(
            ["git", "log", "-1", "--format=%at", "--", str(BUNDLE)],
            capture_output=True,
            text=True,
            cwd=str(ROOT),
            check=False,
        )
        if expected.returncode != 0 or not expected.stdout.strip():
            self.skipTest("no git history dates the bundle in this tree")
        layout = self.scratch("pkg-bundle-archive-git-epoch-") / "layout"
        reported = stdout_fields(self.package(layout).stdout)
        self.assertEqual(
            reported["source_date_epoch"],
            expected.stdout.strip(),
            "SOURCE_DATE_EPOCH must be the source commit's author seconds "
            "(packaging.json: source-commit-author-seconds)",
        )

    def test_a_tree_without_git_and_without_an_epoch_is_refused(self) -> None:
        work = self.scratch("pkg-bundle-archive-noepoch-")
        contracts_root = self.copy_bundle(work)
        out = work / "layout"
        refused = self.package(
            out, contracts_root=contracts_root, pin_epoch=False, expect_exit=2
        )
        self.assertIn("--source-date-epoch", refused.stderr)
        self.assertFalse(out.exists(), "a refusal must write nothing")
        self.package(out, contracts_root=contracts_root)


class RefusalTests(PackagerTestCase):
    def test_out_inside_contracts_is_refused(self) -> None:
        target = CONTRACTS / "substrate-wire" / VERSION / "oci"
        proc = self.package(target, expect_exit=2)
        self.assertIn("contracts/", proc.stderr)
        self.assertFalse(target.exists(), "the packager created a path under contracts/")

    def test_contracts_root_itself_is_refused_as_out(self) -> None:
        proc = self.package(CONTRACTS, expect_exit=2)
        self.assertIn("contracts/", proc.stderr)

    def test_out_inside_a_scratch_contracts_root_is_refused(self) -> None:
        work = self.scratch("pkg-bundle-refusal-")
        contracts_root = self.copy_bundle(work)
        target = contracts_root / "oci"
        proc = self.package(target, contracts_root=contracts_root, expect_exit=2)
        self.assertIn("contracts", proc.stderr)
        self.assertFalse(target.exists())

    def test_non_empty_out_requires_force(self) -> None:
        work = self.scratch("pkg-bundle-force-")
        out = work / "layout"
        first = self.package(out).stdout.strip()
        refused = self.package(out, expect_exit=2)
        self.assertIn("--force", refused.stderr)
        again = self.package(out, extra=("--force",)).stdout.strip()
        self.assertEqual(first, again)

    def test_force_refuses_a_directory_that_is_not_a_layout(self) -> None:
        work = self.scratch("pkg-bundle-foreign-")
        out = work / "not-a-layout"
        out.mkdir()
        (out / "precious.txt").write_text("do not delete me\n", encoding="utf-8")
        proc = self.package(out, extra=("--force",), expect_exit=2)
        self.assertIn("precious.txt", proc.stderr)
        self.assertTrue((out / "precious.txt").is_file())

    def test_unknown_version_is_refused(self) -> None:
        work = self.scratch("pkg-bundle-version-")
        proc = self.package(work / "layout", version="9.9.9", expect_exit=2)
        self.assertIn("9.9.9", proc.stderr)


if __name__ == "__main__":
    unittest.main()
