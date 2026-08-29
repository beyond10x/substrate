#!/usr/bin/env python3
"""Tests for the deterministic contract-bundle OCI packager.

Run: python3 scripts/test_package_contract_bundle.py

Every test that mutates bundle bytes works on a copy under $TMPDIR; nothing here
writes into contracts/ (AGENTS.md invariant 6: a released bundle directory is
immutable, and the packager reads the bundle rather than teaching it about the
packager).
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
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
    ) -> subprocess.CompletedProcess:
        argv = [sys.executable, str(PACKAGER), version, "--out", str(out)]
        if contracts_root is not None:
            argv += ["--contracts-root", str(contracts_root)]
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
        self.assertEqual(self.stdout.strip(), entry["digest"])

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
        layers = self.manifest["layers"]
        titles = [layer["annotations"]["org.opencontainers.image.title"] for layer in layers]
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
