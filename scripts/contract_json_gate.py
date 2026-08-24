#!/usr/bin/env python3
"""Closed JSON classification and offline Draft 2020-12 schema checks."""

from __future__ import annotations

import json
import subprocess
from copy import deepcopy
from pathlib import Path
from typing import Callable


DRAFT_2020_12 = "https://json-schema.org/draft/2020-12/schema"


def closed(properties: dict[str, object], required: list[str] | None = None) -> dict[str, object]:
    return {
        "additionalProperties": False,
        "properties": properties,
        "required": required if required is not None else list(properties),
        "type": "object",
    }


STRING = {"type": "string", "minLength": 1}
STRING_LIST = {"type": "array", "items": STRING}


def authority_schemas(version: str) -> dict[str, object]:
    file_entry = closed(
        {
            "byte_length": {"type": "integer", "minimum": 0},
            "media_type": {"enum": ["application/json", "text/markdown"]},
            "path": STRING,
            "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
        }
    )
    bundle_properties: dict[str, object] = {
        "api_version": {"const": "v1"},
        "bundle_format": {"const": "b10x.contract-bundle.v1"},
        "files": {"type": "array", "items": file_entry, "uniqueItems": True},
        "generator": closed(
            {
                "digest": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                "name": STRING,
                "version": STRING,
            }
        ),
        "name": {"const": "substrate-wire"},
        "origin": {"const": "b10x"},
        "source_base_commit": {"type": ["null", "string"]},
        "status": {"const": "development"},
        "version": {"const": version},
    }
    if version == "0.2.0":
        bundle_properties["compatibility"] = closed(
            {
                "adds_routes": {"const": 7},
                "kind": {"const": "additive-v1"},
                "predecessor": {"const": "0.1.0"},
                "preserves_routes": {"const": 12},
            }
        )
    elif version in {"0.3.0", "0.4.0"}:
        bundle_properties["compatibility"] = closed(
            {
                "adds_routes": {"const": 7},
                "kind": {"const": "additive-v1"},
                "predecessor": {"const": "0.2.0"},
                "preserves_routes": {"const": 19},
            }
        )

    compatibility_properties: dict[str, object] = {
        "contract": {"const": "substrate-wire"},
        "development_constraints": STRING_LIST,
        "request_policy": {"const": "closed"},
        "response_policy": STRING,
        "status": {"const": "development"},
        "supported_api_majors": {"const": [1]},
        "version": {"const": version},
    }
    if version == "0.2.0":
        erratum = closed(
            {
                "compatibility_impact": STRING,
                "corrected_expectation": STRING,
                "erroneous_expectation": STRING,
                "predecessor_path": STRING,
                "predecessor_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                "reason": STRING,
                "replacement_path": STRING,
                "replacement_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
            }
        )
        compatibility_properties["errata_from"] = closed(
            {
                "records": {"type": "array", "items": erratum, "minItems": 1},
                "version": {"const": "0.1.0"},
            }
        )

    origin_input = closed(
        {
            "digest": {"type": ["null", "string"]},
            "name": STRING,
            "origin": STRING,
            "release_blocker": STRING,
            "role": STRING,
            "uri": STRING,
            "version": STRING,
        },
        ["name", "origin", "role", "version"],
    )
    origins = closed(
        {
            "bundle": {"const": f"substrate-wire@{version}"},
            "inputs": {"type": "array", "items": origin_input, "minItems": 1},
            "origin": {"const": "b10x"},
        }
    )
    archive = closed(
        {
            "compression": STRING,
            "format": STRING,
            "gid": {"type": "integer", "minimum": 0},
            "group_name": {"type": "string"},
            "mode": STRING,
            "owner_name": {"type": "string"},
            "path_order": STRING,
            "source_date_epoch": STRING,
            "uid": {"type": "integer", "minimum": 0},
        }
    )
    packaging = closed(
        {
            "archive": archive,
            "json_authority": STRING,
            "release_blockers": STRING_LIST,
            "status": {"const": "development"},
        }
    )
    normalization = closed(
        {
            "dot_segments": STRING,
            "encoded_separator": STRING,
            "path_parameters": STRING,
            "percent_encoding": STRING,
            "query": STRING,
            "repeated_separator": STRING,
            "trailing_separator": STRING,
        }
    )
    tuple_schema = closed(
        {"encoding": STRING, "fields": STRING_LIST, "length_unit": STRING}
    )
    hashing = closed(
        {
            "address_normalization": normalization,
            "algorithm": {"const": "sha256"},
            "canonical_input": STRING,
            "excluded": STRING_LIST,
            "fixtures": {"const": "fixtures/canonical-hash.json"},
            "format": {"const": "b10x.substrate-request-hash.v1"},
            "ledger_key": {"const": ["deployment", "subject", "operation"]},
            "tuple": tuple_schema,
        }
    )
    result = {
        "bundle.json": closed(bundle_properties),
        "compatibility.json": closed(compatibility_properties),
        "hashing.json": hashing,
        "origins.json": origins,
        "packaging.json": packaging,
    }
    for schema in result.values():
        assert isinstance(schema, dict)
        schema["$schema"] = DRAFT_2020_12
    return result


def standards_schema_errors(
    resources: list[dict[str, object]], records: list[dict[str, object]]
) -> list[str]:
    """Run pinned jsonschema Draft 2020-12 meta and instance validation offline."""
    root = Path(__file__).resolve().parent.parent
    command = [
        "cargo",
        "run",
        "--locked",
        "--quiet",
        "--manifest-path",
        str(root / "Cargo.toml"),
        "-p",
        "substrate-contract-check",
        "--",
    ]
    completed = subprocess.run(
        command,
        cwd=root,
        input=json.dumps({"records": records, "resources": resources}, separators=(",", ":")),
        capture_output=True,
        text=True,
        check=False,
    )
    try:
        result = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return [
            "standards meta-schema validator failed without JSON output: "
            f"exit={completed.returncode} stderr={completed.stderr.strip()}"
        ]
    if completed.returncode not in (0, 1) or not isinstance(result, dict):
        return [f"standards meta-schema validator protocol failure: {result!r}"]
    reported = result.get("failures")
    if not isinstance(reported, list) or any(not isinstance(item, str) for item in reported):
        return [f"standards meta-schema validator returned invalid failures: {result!r}"]
    return reported


def schema_pointer(document: object, fragment: str) -> object:
    if fragment in ("", "#"):
        return document
    if not fragment.startswith("#/"):
        raise ValueError(f"unsupported schema fragment {fragment!r}")
    current = document
    for raw in fragment[2:].split("/"):
        part = raw.replace("~1", "/").replace("~0", "~")
        if isinstance(current, dict):
            current = current[part]
        elif isinstance(current, list):
            current = current[int(part)]
        else:
            raise KeyError(part)
    return current


def dereference_schema(
    schema: object,
    schema_path: Path,
    bundle: Path,
    documents: object,
    stack: tuple[tuple[Path, str], ...] = (),
) -> object:
    """Inline exact bundle-relative refs so immutable rootless URN `$id` values cannot rebase them."""
    if isinstance(schema, list):
        return [dereference_schema(item, schema_path, bundle, documents, stack) for item in schema]
    if not isinstance(schema, dict):
        return deepcopy(schema)
    reference = schema.get("$ref")
    if isinstance(reference, str):
        target_text, separator, fragment = reference.partition("#")
        target_path = (schema_path.parent / target_text).resolve() if target_text else schema_path.resolve()
        target_path.relative_to(bundle.resolve())
        identity = (target_path, fragment)
        if identity in stack:
            raise ValueError(f"cyclic schema reference {reference!r}")
        target_document = documents.load(target_path)
        if target_document is None:
            raise ValueError(f"unavailable schema reference {reference!r}")
        target = schema_pointer(target_document, f"#{fragment}" if separator else "")
        resolved = dereference_schema(target, target_path, bundle, documents, (*stack, identity))
        siblings = {
            key: dereference_schema(value, schema_path, bundle, documents, stack)
            for key, value in schema.items()
            if key != "$ref"
        }
        return resolved if not siblings else {"allOf": [resolved], **siblings}
    return {
        key: dereference_schema(value, schema_path, bundle, documents, stack)
        for key, value in schema.items()
    }


def check_json_authority(
    bundle: Path,
    version: str,
    documents: object,
    validate: Callable[[object, object, Path, object, str], list[str]],
    failures: list[str],
) -> int:
    """Require every bundle JSON document to have one exact schema classification."""
    # 0.1 is immutable and predates bundled schemas for its five fixed root
    # authorities.  Development 0.2 has no exception: every JSON instance,
    # including the bundle manifest, declares one schema under schemas/.
    #
    # Immutability exception, 2026-08-24.  The brand rename rewrote the frozen
    # bundles 0.1.0, 0.2.0 and 0.3.0 in place: the former brand name appears in
    # their bytes, and removing it from the repository is what the rename is.
    # No successor bundle can undo that -- cutting a 0.5.0 would add a bundle
    # while leaving three rewritten frozen ones unrecorded -- so the rewrite is
    # recorded here instead of hidden.  This is a one-time identifier rename
    # with no semantic wire change: every bundle was re-rendered by its own
    # renderer, so each remains a reproducible fixed point of that renderer.
    # Immutability applies again from this commit forward.
    embedded = authority_schemas(version) if version == "0.1.0" else {}
    count = 0
    standards_records: list[dict[str, object]] = []
    standards_resources: list[dict[str, object]] = []
    standards_resource_uris: dict[str, str] = {}
    for path in sorted(bundle.rglob("*.json")):
        count += 1
        relative = path.relative_to(bundle).as_posix()
        document = documents.load(path)
        if document is None:
            continue
        if relative.startswith("schemas/"):
            if not isinstance(document, dict) or document.get("$schema") != DRAFT_2020_12:
                failures.append(
                    f"{relative}: schema authority must declare the pinned Draft 2020-12 meta-schema"
                )
            standards_records.append({"kind": "meta", "label": relative, "schema": document})
            continue
        contract = embedded.get(relative)
        schema_path = bundle / f".embedded/{relative}.schema"
        if contract is not None:
            standards_records.append(
                {"kind": "meta", "label": f"fixed:{relative}", "schema": contract}
            )
        if contract is None:
            declaration = document.get("$schema") if isinstance(document, dict) else None
            if not isinstance(declaration, str):
                failures.append(f"{relative}: unclassified JSON authority (missing exact schema mapping)")
                continue
            target = (path.parent / declaration).resolve()
            try:
                target.relative_to(bundle.resolve())
            except ValueError:
                failures.append(f"{relative}: declared schema escapes bundle: {declaration}")
                continue
            try:
                target.relative_to((bundle / "schemas").resolve())
            except ValueError:
                failures.append(
                    f"{relative}: declared schema is not under schemas/: {declaration}"
                )
                continue
            contract = documents.load(target)
            schema_path = target
            if contract is None:
                failures.append(f"{relative}: declared schema is unavailable: {declaration}")
                continue
            if not isinstance(contract, dict) or contract.get("$schema") != DRAFT_2020_12:
                failures.append(
                    f"{relative}: declared target is not a Draft 2020-12 schema authority: {declaration}"
                )
                continue
        failures.extend(
            f"{relative}: classified schema validation: {error}"
            for error in validate(document, contract, schema_path, documents, "$")
        )
        schema_key = f"{schema_path.resolve()}#{json.dumps(contract, sort_keys=True)}"
        schema_uri = standards_resource_uris.get(schema_key, "")
        if not schema_uri:
            schema_uri = (
                f"https://b10x.invalid/substrate-wire/{version}/classified/"
                f"{len(standards_resource_uris)}"
            )
            try:
                resolved_contract = dereference_schema(
                    contract, schema_path, bundle, documents
                )
            except (KeyError, ValueError) as error:
                failures.append(f"{relative}: standards schema resolution failed: {error}")
                continue
            standards_resource_uris[schema_key] = schema_uri
            standards_resources.append({"uri": schema_uri, "schema": resolved_contract})
        standards_records.append(
            {
                "kind": "instance",
                "label": relative,
                "schema_uri": schema_uri,
                "instance": document,
            }
        )
    failures.extend(standards_schema_errors(standards_resources, standards_records))
    return count
