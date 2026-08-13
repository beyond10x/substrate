#!/usr/bin/env python3
"""Verify the development bundle inventory, source format, and content hashes."""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "contracts" / "substrate-wire" / "0.1.0"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
CODE = re.compile(r"^[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)+$")
VECTOR_ID = re.compile(r"^[a-z][a-z0-9-]+$")
MEDIA_TYPES = {
    ".json": "application/json",
    ".md": "text/markdown",
}


def reject_duplicate_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate object key {key!r}")
        value[key] = item
    return value


def load(path: Path, failures: list[str]) -> object | None:
    try:
        text = path.read_text(encoding="utf-8")
        value = json.loads(text, object_pairs_hook=reject_duplicate_pairs)
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
        failures.append(f"{path.relative_to(ROOT)}: invalid JSON: {error}")
        return None
    rendered = json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    if text != rendered:
        failures.append(f"{path.relative_to(ROOT)}: JSON is not in deterministic source form")
    return value


def main() -> int:
    failures: list[str] = []
    manifest_path = BUNDLE / "bundle.json"
    manifest = load(manifest_path, failures)
    if not isinstance(manifest, dict):
        print("\n".join(failures), file=sys.stderr)
        return 1

    expected_identity = {
        "api_version": "v1",
        "bundle_format": "daemonloom.contract-bundle.v1",
        "name": "substrate-wire",
        "origin": "daemonloom",
        "status": "development",
        "version": "0.1.0",
    }
    for key, expected in expected_identity.items():
        if manifest.get(key) != expected:
            failures.append(f"bundle.json: {key} must be {expected!r}")

    entries = manifest.get("files")
    if not isinstance(entries, list):
        failures.append("bundle.json: files must be an array")
        entries = []

    actual = {
        path.relative_to(BUNDLE).as_posix()
        for path in BUNDLE.rglob("*")
        if path.is_file() and path != manifest_path
    }
    listed: set[str] = set()
    vector_ids: set[str] = set()

    for index, entry in enumerate(entries):
        if not isinstance(entry, dict) or set(entry) != {
            "byte_length",
            "media_type",
            "path",
            "sha256",
        }:
            failures.append(
                f"bundle.json: files[{index}] must contain byte_length, media_type, path, and sha256"
            )
            continue
        relative = entry.get("path")
        digest = entry.get("sha256")
        if not isinstance(relative, str):
            failures.append(f"bundle.json: files[{index}].path must be text")
            continue
        pure = PurePosixPath(relative)
        if pure.is_absolute() or ".." in pure.parts or relative != pure.as_posix():
            failures.append(f"bundle.json: unsafe or non-canonical path {relative!r}")
            continue
        if relative in listed:
            failures.append(f"bundle.json: duplicate file {relative}")
            continue
        listed.add(relative)
        path = BUNDLE / relative
        if not path.is_file():
            failures.append(f"bundle.json: missing file {relative}")
            continue
        expected_media_type = MEDIA_TYPES.get(path.suffix)
        if entry.get("media_type") != expected_media_type:
            failures.append(
                f"bundle.json: media_type for {relative} must be {expected_media_type!r}"
            )
        byte_length = len(path.read_bytes())
        if entry.get("byte_length") != byte_length:
            failures.append(
                f"bundle.json: byte_length mismatch for {relative}: "
                f"stated {entry.get('byte_length')!r}, computed {byte_length}"
            )
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            failures.append(f"bundle.json: invalid sha256 for {relative}")
        else:
            computed = hashlib.sha256(path.read_bytes()).hexdigest()
            if digest != computed:
                failures.append(
                    f"bundle.json: digest mismatch for {relative}: stated {digest}, computed {computed}"
                )

        value = load(path, failures) if path.suffix == ".json" else None
        if relative.startswith("vectors/") and isinstance(value, dict):
            vector_id = value.get("id")
            if not isinstance(vector_id, str) or not VECTOR_ID.fullmatch(vector_id):
                failures.append(f"{relative}: invalid vector id")
            elif vector_id in vector_ids:
                failures.append(f"{relative}: duplicate vector id {vector_id}")
            else:
                vector_ids.add(vector_id)
            required = {
                "$schema",
                "assertions",
                "context",
                "expected",
                "id",
                "layer",
                "phase",
                "preconditions",
                "request",
            }
            if set(value) != required:
                failures.append(f"{relative}: vector keys differ from the closed vector schema")
            if value.get("$schema") != "../../schemas/vector.json":
                failures.append(f"{relative}: $schema must name the bundled vector schema")
            if value.get("phase") != 2:
                failures.append(f"{relative}: phase must be 2")
            expected_layer = relative.split("/", 2)[1]
            if value.get("layer") != expected_layer:
                failures.append(f"{relative}: layer must be {expected_layer}")
            context = value.get("context")
            if not isinstance(context, dict):
                failures.append(f"{relative}: context must be an object")
            elif expected_layer == "http":
                if set(context) != {"authenticated_subject"} or not isinstance(
                    context.get("authenticated_subject"), str
                ):
                    failures.append(
                        f"{relative}: HTTP context must contain only authenticated_subject"
                    )
            elif context != {"fixture_authority": "driver-harness"}:
                failures.append(
                    f"{relative}: driver context must identify the trusted driver harness"
                )
            for key in ("assertions", "preconditions"):
                items = value.get(key)
                minimum = 1 if key == "assertions" else 0
                if (
                    not isinstance(items, list)
                    or len(items) < minimum
                    or any(not isinstance(item, str) or not item for item in items)
                ):
                    failures.append(f"{relative}: {key} must be an array of non-empty strings")
            if not isinstance(value.get("request"), dict):
                failures.append(f"{relative}: request must be an object")
            expected = value.get("expected")
            expected_keys = {"class", "code", "http_status", "outcome"}
            outcomes = {"success", "refused", "conflict", "unserved", "exhausted", "failed"}
            failure_classes = outcomes - {"success"}
            if not isinstance(expected, dict) or not set(expected).issubset(expected_keys):
                failures.append(f"{relative}: expected has unknown fields or is not an object")
            else:
                outcome = expected.get("outcome")
                if outcome not in outcomes:
                    failures.append(f"{relative}: invalid expected outcome")
                error_class = expected.get("class")
                if error_class is not None and error_class not in failure_classes:
                    failures.append(f"{relative}: invalid expected class")
                if outcome in failure_classes and error_class != outcome:
                    failures.append(f"{relative}: failure outcome and class must match")
                if outcome == "success" and error_class is not None:
                    failures.append(f"{relative}: successful outcome cannot carry an error class")
                code = expected.get("code")
                if code is not None and (not isinstance(code, str) or not CODE.fullmatch(code)):
                    failures.append(f"{relative}: invalid expected code")
                status = expected.get("http_status")
                if status is not None and (
                    isinstance(status, bool) or not isinstance(status, int) or not 200 <= status <= 599
                ):
                    failures.append(f"{relative}: invalid expected HTTP status")

    if listed != actual:
        for relative in sorted(actual - listed):
            failures.append(f"bundle.json: unmanifested file {relative}")
        for relative in sorted(listed - actual):
            failures.append(f"bundle.json: listed absent file {relative}")
    paths = [entry.get("path") for entry in entries if isinstance(entry, dict)]
    if paths != sorted(paths):
        failures.append("bundle.json: files must be sorted by path")

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"substrate-wire 0.1.0: {len(listed)} files and {len(vector_ids)} vectors verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
