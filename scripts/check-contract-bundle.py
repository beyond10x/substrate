#!/usr/bin/env python3
"""Verify the substrate-wire bundle without third-party dependencies."""

from __future__ import annotations

import base64
import binascii
import datetime as dt
import hashlib
import json
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath
from urllib.parse import unquote_to_bytes

from contract_json_gate import check_json_authority


ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "contracts" / "substrate-wire" / "0.1.0"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
VECTOR_ID = re.compile(r"^[a-z][a-z0-9-]+$")
MEDIA_TYPES = {".json": "application/json", ".md": "text/markdown"}
EXPECTED_ROUTES = [
    ("machine.get", "GET", "/v1/machine"),
    ("workspace.create", "POST", "/v1/workspaces"),
    ("workspace.get", "GET", "/v1/workspaces/{workspace_id}"),
    ("workspace.file.read", "GET", "/v1/workspaces/{workspace_id}/files/{path}"),
    ("workspace.file.write", "PUT", "/v1/workspaces/{workspace_id}/files/{path}"),
    ("workspace.file.delete", "DELETE", "/v1/workspaces/{workspace_id}/files/{path}"),
    ("workspace.destroy", "DELETE", "/v1/workspaces/{workspace_id}"),
    ("exec.start", "POST", "/v1/execs"),
    ("exec.get", "GET", "/v1/execs/{exec_id}"),
    ("exec.output.get", "GET", "/v1/execs/{exec_id}/output"),
    ("exec.signal", "POST", "/v1/execs/{exec_id}/signal"),
    ("operation.get", "GET", "/v1/ops/{operation_id}"),
]
REQUIRED_COVERAGE = {
    "behavior.nonzero-exit-observation",
    "error.conflict",
    "error.exhausted",
    "error.failed",
    "error.refused",
    "error.unserved",
    "hash.different-input-conflict",
    "hash.ledger-scope",
    "hash.transport-exclusions",
    "lifecycle.crash-after-dispatch",
    "lifecycle.crash-before-dispatch",
    "lifecycle.different-input-conflict",
    "lifecycle.lost-answer-reconciliation",
    "lifecycle.post-action-observation",
    "lifecycle.stable-replay",
    "lifecycle.subject-operation-isolation",
    "lifecycle.unknown-preserved",
    "route.exec.get",
    "route.exec.output.get",
    "route.exec.signal",
    "route.exec.start",
    "route.machine.get",
    "route.operation.get",
    "route.workspace.create",
    "route.workspace.destroy",
    "route.workspace.file.delete",
    "route.workspace.file.read",
    "route.workspace.file.write",
    "route.workspace.get",
    "schema.strict-request",
    "security.atomic-replacement",
    "security.bounds.delete",
    "security.bounds.input",
    "security.bounds.list",
    "security.bounds.read",
    "security.bounds.write",
    "security.bounds.resource",
    "security.daemon-credential",
    "security.daemon-environment",
    "security.daemon-fd",
    "security.git.helper",
    "security.git.hook",
    "security.git.lfs",
    "security.git.proxy",
    "security.git.rebinding",
    "security.git.redirect",
    "security.git.submodule",
    "security.no-egress",
    "security.output-draining",
    "security.path.absolute",
    "security.path.dangling-link",
    "security.path.lexical",
    "security.path.magic-link",
    "security.path.mount",
    "security.path.symlink",
    "security.post-action-observation",
    "security.process-tree-cancellation",
    "security.sandbox-unavailable",
    "security.stale-capability",
    "security.subject-resource-isolation",
    "security.timeout",
    "security.unauthenticated-reachable-startup",
}


def reject_duplicate_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate object key {key!r}")
        result[key] = value
    return result


class Documents:
    def __init__(self, failures: list[str]) -> None:
        self.failures = failures
        self.cache: dict[Path, object] = {}

    def load(self, path: Path) -> object | None:
        path = path.resolve()
        if path in self.cache:
            return self.cache[path]
        try:
            text = path.read_text(encoding="utf-8")
            value = json.loads(text, object_pairs_hook=reject_duplicate_pairs)
        except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
            self.failures.append(f"{display(path)}: invalid JSON: {error}")
            return None
        rendered = json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
        if text != rendered:
            self.failures.append(f"{display(path)}: JSON is not in deterministic source form")
        self.cache[path] = value
        return value


def display(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return str(path)


def same_json(left: object, right: object) -> bool:
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(same_json(left[key], right[key]) for key in left)  # type: ignore[index]
    if isinstance(left, list):
        return len(left) == len(right) and all(same_json(a, b) for a, b in zip(left, right))  # type: ignore[arg-type]
    return left == right


def instance_type(instance: object, expected: str) -> bool:
    if expected == "null":
        return instance is None
    if expected == "boolean":
        return isinstance(instance, bool)
    if expected == "integer":
        return isinstance(instance, int) and not isinstance(instance, bool)
    if expected == "number":
        return isinstance(instance, (int, float)) and not isinstance(instance, bool)
    if expected == "string":
        return isinstance(instance, str)
    if expected == "array":
        return isinstance(instance, list)
    if expected == "object":
        return isinstance(instance, dict)
    return False


def pointer(document: object, fragment: str) -> object:
    if fragment in ("", "#"):
        return document
    if not fragment.startswith("#/"):
        raise ValueError(f"unsupported JSON pointer fragment {fragment!r}")
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


def resolve_ref(ref: str, schema_path: Path, documents: Documents) -> tuple[object, Path]:
    target, separator, fragment = ref.partition("#")
    if target:
        target_path = (schema_path.parent / target).resolve()
        try:
            target_path.relative_to(BUNDLE.resolve())
        except ValueError as error:
            raise ValueError(f"reference escapes bundle: {ref}") from error
        document = documents.load(target_path)
        if document is None:
            raise ValueError(f"cannot load reference {ref}")
    else:
        target_path = schema_path
        document = documents.load(schema_path)
        if document is None:
            raise ValueError(f"cannot load local reference {ref}")
    return pointer(document, f"#{fragment}" if separator else ""), target_path


def validate(
    instance: object,
    contract: object,
    schema_path: Path,
    documents: Documents,
    location: str = "$",
) -> list[str]:
    if contract is True:
        return []
    if contract is False:
        return [f"{location}: false schema rejects instance"]
    if not isinstance(contract, dict):
        return [f"{location}: schema is not an object or boolean"]
    errors: list[str] = []

    ref = contract.get("$ref")
    if isinstance(ref, str):
        try:
            resolved, resolved_path = resolve_ref(ref, schema_path, documents)
            errors.extend(validate(instance, resolved, resolved_path, documents, location))
        except (KeyError, ValueError) as error:
            errors.append(f"{location}: invalid $ref {ref!r}: {error}")

    if "const" in contract and not same_json(instance, contract["const"]):
        errors.append(f"{location}: expected const {contract['const']!r}")
    enum = contract.get("enum")
    if isinstance(enum, list) and not any(same_json(instance, candidate) for candidate in enum):
        errors.append(f"{location}: value is outside enum")

    type_value = contract.get("type")
    if isinstance(type_value, str):
        accepted_types = [type_value]
    elif isinstance(type_value, list):
        accepted_types = [item for item in type_value if isinstance(item, str)]
    else:
        accepted_types = []
    if accepted_types and not any(instance_type(instance, expected) for expected in accepted_types):
        errors.append(f"{location}: expected type {'|'.join(accepted_types)}, got {type(instance).__name__}")
        return errors

    all_of = contract.get("allOf")
    if isinstance(all_of, list):
        for index, branch in enumerate(all_of):
            errors.extend(validate(instance, branch, schema_path, documents, f"{location}[allOf:{index}]"))
    any_of = contract.get("anyOf")
    if isinstance(any_of, list):
        matches = [not validate(instance, branch, schema_path, documents, location) for branch in any_of]
        if not any(matches):
            errors.append(f"{location}: matches no anyOf branch")
    one_of = contract.get("oneOf")
    if isinstance(one_of, list):
        matches = [not validate(instance, branch, schema_path, documents, location) for branch in one_of]
        if sum(matches) != 1:
            errors.append(f"{location}: matches {sum(matches)} oneOf branches, expected exactly one")
    not_schema = contract.get("not")
    if isinstance(not_schema, (dict, bool)) and not validate(instance, not_schema, schema_path, documents, location):
        errors.append(f"{location}: matches forbidden schema")

    condition = contract.get("if")
    if isinstance(condition, (dict, bool)):
        matched = not validate(instance, condition, schema_path, documents, location)
        selected = contract.get("then") if matched else contract.get("else")
        if isinstance(selected, (dict, bool)):
            errors.extend(validate(instance, selected, schema_path, documents, location))

    if isinstance(instance, dict):
        required = contract.get("required")
        if isinstance(required, list):
            for key in required:
                if key not in instance:
                    errors.append(f"{location}: missing required property {key!r}")
        properties = contract.get("properties")
        known = set(properties) if isinstance(properties, dict) else set()
        if isinstance(properties, dict):
            for key, child_schema in properties.items():
                if key in instance:
                    errors.extend(validate(instance[key], child_schema, schema_path, documents, f"{location}/{key}"))
        additional = contract.get("additionalProperties")
        for key, value in instance.items():
            if key in known:
                continue
            if additional is False:
                errors.append(f"{location}: additional property {key!r} is forbidden")
            elif isinstance(additional, (dict, bool)):
                errors.extend(validate(value, additional, schema_path, documents, f"{location}/{key}"))
        property_names = contract.get("propertyNames")
        if isinstance(property_names, (dict, bool)):
            for key in instance:
                errors.extend(validate(key, property_names, schema_path, documents, f"{location}/<property:{key}>"))
        minimum = contract.get("minProperties")
        maximum = contract.get("maxProperties")
        if isinstance(minimum, int) and len(instance) < minimum:
            errors.append(f"{location}: fewer than {minimum} properties")
        if isinstance(maximum, int) and len(instance) > maximum:
            errors.append(f"{location}: more than {maximum} properties")

    if isinstance(instance, list):
        items = contract.get("items")
        if isinstance(items, (dict, bool)):
            for index, item in enumerate(instance):
                errors.extend(validate(item, items, schema_path, documents, f"{location}/{index}"))
        minimum = contract.get("minItems")
        maximum = contract.get("maxItems")
        if isinstance(minimum, int) and len(instance) < minimum:
            errors.append(f"{location}: fewer than {minimum} items")
        if isinstance(maximum, int) and len(instance) > maximum:
            errors.append(f"{location}: more than {maximum} items")
        if contract.get("uniqueItems") is True:
            for index, item in enumerate(instance):
                if any(same_json(item, prior) for prior in instance[:index]):
                    errors.append(f"{location}/{index}: duplicate item")

    if isinstance(instance, str):
        minimum = contract.get("minLength")
        maximum = contract.get("maxLength")
        if isinstance(minimum, int) and len(instance) < minimum:
            errors.append(f"{location}: shorter than {minimum} characters")
        if isinstance(maximum, int) and len(instance) > maximum:
            errors.append(f"{location}: longer than {maximum} characters")
        pattern_value = contract.get("pattern")
        if isinstance(pattern_value, str):
            try:
                if re.search(pattern_value, instance) is None:
                    errors.append(f"{location}: does not match {pattern_value!r}")
            except re.error as error:
                errors.append(f"{location}: invalid schema regex {pattern_value!r}: {error}")
        if contract.get("format") == "date-time":
            try:
                dt.datetime.fromisoformat(instance.replace("Z", "+00:00"))
            except ValueError:
                errors.append(f"{location}: invalid date-time")
        max_depth = contract.get("x-daemonloom-max-depth")
        if isinstance(max_depth, int) and len(instance.split("/")) > max_depth:
            errors.append(f"{location}: path has more than {max_depth} components")

    if isinstance(instance, (int, float)) and not isinstance(instance, bool):
        minimum = contract.get("minimum")
        maximum = contract.get("maximum")
        if isinstance(minimum, (int, float)) and instance < minimum:
            errors.append(f"{location}: below minimum {minimum}")
        if isinstance(maximum, (int, float)) and instance > maximum:
            errors.append(f"{location}: above maximum {maximum}")
    return errors


def jcs(value: object) -> bytes:
    if value is None:
        return b"null"
    if value is True:
        return b"true"
    if value is False:
        return b"false"
    if isinstance(value, int):
        return str(value).encode("ascii")
    if isinstance(value, float):
        raise TypeError("phase-2 operation inputs may not contain floating point values")
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if isinstance(value, list):
        return b"[" + b",".join(jcs(item) for item in value) + b"]"
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("JCS object keys must be strings")
        keys = sorted(value, key=lambda key: key.encode("utf-16be"))
        return b"{" + b",".join(jcs(key) + b":" + jcs(value[key]) for key in keys) + b"}"
    raise TypeError(f"unsupported canonical value {type(value).__name__}")


def canonical_tuple(api_major: str, method: str, address: str, operation_input: object) -> bytes:
    fields = [api_major.encode("ascii"), method.encode("ascii"), address.encode("utf-8"), jcs(operation_input)]
    return b"".join(struct.pack(">I", len(field)) + field for field in fields)


def normalized_address(address: str) -> bool:
    if not address.startswith("/v1/") or "//" in address or address.endswith("/"):
        return False
    if re.search(r"%(?![0-9A-F]{2})", address) or re.search(r"%[0-9A-F][a-f]", address):
        return False
    for encoded in re.findall(r"%([0-9A-F]{2})", address):
        octet = int(encoded, 16)
        if chr(octet) in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~":
            return False
    if re.search(r"[^A-Za-z0-9/_.~%:-]", address):
        return False
    try:
        raw = unquote_to_bytes(address)
        decoded = raw.decode("utf-8")
    except (UnicodeDecodeError, ValueError):
        return False
    segments = decoded.split("/")
    if any(segment in (".", "..") or not segment for segment in segments[1:]):
        return False
    return "%2f" not in address.lower() and "%5c" not in address.lower()


def route_pattern(template: str) -> re.Pattern[str]:
    pieces: list[str] = []
    cursor = 0
    for match in re.finditer(r"\{([a-z_]+)\}", template):
        pieces.append(re.escape(template[cursor : match.start()]))
        name = match.group(1)
        if name == "workspace_id":
            expression = r"ws_[A-Za-z0-9]+"
        elif name == "exec_id":
            expression = r"ex_[A-Za-z0-9]+"
        elif name == "operation_id":
            expression = r"[A-Za-z0-9_-]{16,128}"
        elif name == "path":
            expression = r".+"
        else:
            raise ValueError(f"unknown route parameter {name}")
        pieces.append(f"(?P<{name}>{expression})")
        cursor = match.end()
    pieces.append(re.escape(template[cursor:]))
    return re.compile("^" + "".join(pieces) + "$")


def route_address(template: str, request_path: str) -> dict[str, str] | None:
    match = route_pattern(template).fullmatch(request_path)
    if match is None or not normalized_address(request_path):
        return None
    result: dict[str, str] = {}
    for name, raw in match.groupdict().items():
        if name != "path":
            result[name] = raw
            continue
        if "%2f" in raw.lower() or "%5c" in raw.lower():
            return None
        try:
            result[name] = unquote_to_bytes(raw).decode("utf-8")
        except UnicodeDecodeError:
            return None
    return result


def find_embedded(value: object, target: str) -> bool:
    if isinstance(value, str):
        return value == target
    if isinstance(value, list):
        return any(find_embedded(item, target) for item in value)
    if isinstance(value, dict):
        return any(key in {"fixture_id", "vector_id"} or find_embedded(item, target) for key, item in value.items())
    return False


def check_base64_content(operation_id: str, operation_input: object, failures: list[str], location: str) -> None:
    if operation_id != "workspace.file.write" or not isinstance(operation_input, dict):
        return
    content = operation_input.get("content")
    if not isinstance(content, dict) or not isinstance(content.get("data"), str):
        return
    try:
        decoded = base64.b64decode(content["data"], validate=True)
    except (binascii.Error, ValueError) as error:
        failures.append(f"{location}: invalid base64 content: {error}")
        return
    if len(decoded) > 1048576:
        failures.append(f"{location}: decoded file content exceeds the schema ceiling")


def check_manifest(documents: Documents, failures: list[str]) -> set[str]:
    manifest_path = BUNDLE / "bundle.json"
    manifest = documents.load(manifest_path)
    if not isinstance(manifest, dict):
        return set()
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
    if manifest.get("source_base_commit") is not None:
        failures.append("bundle.json: development source_base_commit must remain null until release materialization")
    generator = manifest.get("generator")
    renderer = ROOT / "scripts" / "render-contract-bundle.py"
    if not isinstance(generator, dict) or generator.get("name") != "scripts/render-contract-bundle.py":
        failures.append("bundle.json: generator must name scripts/render-contract-bundle.py")
    elif generator.get("digest") != hashlib.sha256(renderer.read_bytes()).hexdigest():
        failures.append("bundle.json: generator digest does not match scripts/render-contract-bundle.py")

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
    paths: list[str] = []
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict) or set(entry) != {"byte_length", "media_type", "path", "sha256"}:
            failures.append(f"bundle.json: files[{index}] has the wrong closed shape")
            continue
        relative = entry.get("path")
        if not isinstance(relative, str):
            failures.append(f"bundle.json: files[{index}].path must be text")
            continue
        paths.append(relative)
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
        data = path.read_bytes()
        expected_media_type = MEDIA_TYPES.get(path.suffix)
        if entry.get("media_type") != expected_media_type:
            failures.append(f"bundle.json: wrong media type for {relative}")
        if entry.get("byte_length") != len(data):
            failures.append(f"bundle.json: byte length mismatch for {relative}")
        digest = entry.get("sha256")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            failures.append(f"bundle.json: invalid digest for {relative}")
        elif digest != hashlib.sha256(data).hexdigest():
            failures.append(f"bundle.json: digest mismatch for {relative}")
        if path.suffix == ".json":
            documents.load(path)
    for relative in sorted(actual - listed):
        failures.append(f"bundle.json: unmanifested file {relative}")
    for relative in sorted(listed - actual):
        failures.append(f"bundle.json: listed absent file {relative}")
    if paths != sorted(paths):
        failures.append("bundle.json: files must be sorted by path")
    return listed


def check_renderer_reproducibility(failures: list[str]) -> None:
    renderer = ROOT / "scripts" / "render-contract-bundle.py"
    with tempfile.TemporaryDirectory(prefix="substrate-contract-render-") as temporary:
        clean_root = Path(temporary) / "substrate"
        clean_bundle = clean_root / "contracts" / "substrate-wire" / "0.1.0"
        clean_scripts = clean_root / "scripts"
        clean_bundle.parent.mkdir(parents=True)
        clean_scripts.mkdir(parents=True)
        shutil.copytree(BUNDLE, clean_bundle)
        shutil.copy2(renderer, clean_scripts / renderer.name)
        result = subprocess.run(
            [sys.executable, str(clean_scripts / renderer.name)],
            cwd=clean_root,
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            failures.append(f"contract renderer failed in isolation: {result.stderr.strip()}")
            return
        current_paths = {path.relative_to(BUNDLE).as_posix() for path in BUNDLE.rglob("*") if path.is_file()}
        rendered_paths = {path.relative_to(clean_bundle).as_posix() for path in clean_bundle.rglob("*") if path.is_file()}
        if current_paths != rendered_paths:
            failures.append("isolated renderer changes the bundle file inventory")
            return
        for relative in sorted(current_paths):
            if (BUNDLE / relative).read_bytes() != (clean_bundle / relative).read_bytes():
                failures.append(f"isolated renderer changes {relative}")


def check_schema_references(documents: Documents, listed: set[str], failures: list[str]) -> None:
    def walk(value: object, path: Path) -> None:
        if isinstance(value, dict):
            ref = value.get("$ref")
            if isinstance(ref, str):
                try:
                    resolve_ref(ref, path, documents)
                except (KeyError, ValueError) as error:
                    failures.append(f"{display(path)}: invalid reference {ref!r}: {error}")
            for child in value.values():
                walk(child, path)
        elif isinstance(value, list):
            for child in value:
                walk(child, path)

    for relative in sorted(listed):
        if relative.startswith("schemas/") and relative.endswith(".json"):
            path = BUNDLE / relative
            document = documents.load(path)
            if document is not None:
                walk(document, path)


def check_registry(documents: Documents, failures: list[str]) -> tuple[dict[str, dict[str, object]], Path]:
    path = BUNDLE / "operations.json"
    registry = documents.load(path)
    registry_schema_path = BUNDLE / "schemas" / "operation-registry.json"
    registry_schema = documents.load(registry_schema_path)
    if registry is not None and registry_schema is not None:
        failures.extend(f"operations.json: {error}" for error in validate(registry, registry_schema, registry_schema_path, documents))
    if not isinstance(registry, dict) or not isinstance(registry.get("operations"), list):
        return {}, path
    operations = registry["operations"]
    observed = [
        (entry.get("id"), entry.get("method"), entry.get("path"))
        for entry in operations
        if isinstance(entry, dict)
    ]
    if observed != EXPECTED_ROUTES:
        failures.append(f"operations.json: route inventory differs from exact phase-2 set: {observed!r}")
    by_id: dict[str, dict[str, object]] = {}
    for index, entry in enumerate(operations):
        if not isinstance(entry, dict) or not isinstance(entry.get("id"), str):
            continue
        operation_id = entry["id"]
        if operation_id in by_id:
            failures.append(f"operations.json: duplicate operation id {operation_id}")
        by_id[operation_id] = entry
        for field in ("address_schema", "input_schema", "result_schema"):
            relative = entry.get(field)
            if not isinstance(relative, str) or not (BUNDLE / relative).is_file():
                failures.append(f"operations.json: operations[{index}].{field} does not name a bundled schema")
        predicates = entry.get("capability_predicates")
        if not isinstance(predicates, list):
            failures.append(f"operations.json: {operation_id} has no capability predicate list")
    return by_id, path


def check_hashes(documents: Documents, failures: list[str]) -> set[str]:
    hashing = documents.load(BUNDLE / "hashing.json")
    fixtures_path = BUNDLE / "fixtures" / "canonical-hash.json"
    fixtures = documents.load(fixtures_path)
    schema_path = BUNDLE / "schemas" / "hash-fixtures.json"
    fixture_schema = documents.load(schema_path)
    if not isinstance(hashing, dict):
        return set()
    if hashing.get("excluded") != ["operation", "request_id", "headers", "authorization", "bearer", "subject", "principal", "deployment"]:
        failures.append("hashing.json: exclusion inventory changed")
    if hashing.get("ledger_key") != ["deployment", "subject", "operation"]:
        failures.append("hashing.json: ledger key must be deployment, subject, operation")
    if fixtures is not None and fixture_schema is not None:
        failures.extend(f"fixtures/canonical-hash.json: {error}" for error in validate(fixtures, fixture_schema, schema_path, documents))
    if not isinstance(fixtures, dict) or not isinstance(fixtures.get("cases"), list):
        return set()
    cases: dict[str, dict[str, object]] = {}
    for case in fixtures["cases"]:
        if not isinstance(case, dict) or not isinstance(case.get("id"), str):
            continue
        case_id = case["id"]
        if case_id in cases:
            failures.append(f"fixtures/canonical-hash.json: duplicate case {case_id}")
        cases[case_id] = case
        try:
            canonical = jcs(case["input"])
            tuple_bytes = canonical_tuple(case["api_major"], case["method"], case["normalized_address"], case["input"])
        except (KeyError, TypeError, UnicodeError) as error:
            failures.append(f"hash fixture {case_id}: cannot canonicalize: {error}")
            continue
        if not normalized_address(case["normalized_address"]):
            failures.append(f"hash fixture {case_id}: address is not normalized")
        if case.get("jcs_input_hex") != canonical.hex():
            failures.append(f"hash fixture {case_id}: JCS bytes differ")
        if case.get("tuple_hex") != tuple_bytes.hex():
            failures.append(f"hash fixture {case_id}: length-delimited tuple differs")
        if case.get("sha256") != hashlib.sha256(tuple_bytes).hexdigest():
            failures.append(f"hash fixture {case_id}: SHA-256 differs")
        excluded = case.get("excluded")
        ledger = case.get("ledger_key")
        if isinstance(excluded, dict) and isinstance(ledger, dict):
            for key in ("deployment", "subject", "operation"):
                if excluded.get(key) != ledger.get(key):
                    failures.append(f"hash fixture {case_id}: excluded {key} differs from ledger key")
    for case in cases.values():
        relation = case.get("relation")
        if not isinstance(relation, dict):
            continue
        other = cases.get(relation.get("case"))
        if other is None:
            failures.append(f"hash fixture {case['id']}: relation target is absent")
            continue
        kind = relation.get("kind")
        if kind == "same-request-hash-different-ledger-key":
            if case.get("sha256") != other.get("sha256") or same_json(case.get("ledger_key"), other.get("ledger_key")):
                failures.append(f"hash fixture {case['id']}: transport/scope exclusion relation is false")
            case_excluded = case.get("excluded")
            other_excluded = other.get("excluded")
            if isinstance(case_excluded, dict) and isinstance(other_excluded, dict):
                unchanged = sorted(key for key in case_excluded if same_json(case_excluded[key], other_excluded.get(key)))
                if unchanged:
                    failures.append(f"hash fixture {case['id']}: excluded fields were not varied: {unchanged}")
        elif kind == "same-request-hash-same-ledger-key":
            if case.get("sha256") != other.get("sha256") or not same_json(case.get("ledger_key"), other.get("ledger_key")):
                failures.append(f"hash fixture {case['id']}: stable replay relation is false")
        elif kind == "different-request-hash-same-ledger-key":
            if case.get("sha256") == other.get("sha256") or not same_json(case.get("ledger_key"), other.get("ledger_key")):
                failures.append(f"hash fixture {case['id']}: replay-conflict relation is false")
    return set(cases)


def check_schema_fixtures(documents: Documents, failures: list[str]) -> None:
    wrapper_schema_path = BUNDLE / "schemas" / "schema-fixtures.json"
    wrapper_schema = documents.load(wrapper_schema_path)
    fixture_paths = [
        BUNDLE / "fixtures" / "operation-states.json",
        BUNDLE / "fixtures" / "resource-invariants.json",
        BUNDLE / "fixtures" / "runner-results.json",
    ]
    for path in fixture_paths:
        value = documents.load(path)
        if value is None or wrapper_schema is None:
            continue
        failures.extend(f"{display(path)}: {error}" for error in validate(value, wrapper_schema, wrapper_schema_path, documents))
        if not isinstance(value, dict) or not isinstance(value.get("schema"), str):
            continue
        schema_ref = value["schema"]
        target, separator, fragment = schema_ref.partition("#")
        target_path = BUNDLE / target
        contract = documents.load(target_path)
        if contract is None:
            continue
        try:
            contract = pointer(contract, f"#{fragment}" if separator else "")
        except (KeyError, ValueError) as error:
            failures.append(f"{display(path)}: invalid target schema pointer: {error}")
            continue
        for index, instance in enumerate(value.get("valid", [])):
            errors = validate(instance, contract, target_path, documents)
            failures.extend(f"{display(path)}: valid[{index}] rejected: {error}" for error in errors)
        for index, case in enumerate(value.get("invalid", [])):
            if not isinstance(case, dict):
                continue
            errors = validate(case.get("instance"), contract, target_path, documents)
            if not errors:
                failures.append(f"{display(path)}: invalid[{index}] unexpectedly validates")


def check_vectors(
    documents: Documents,
    operations: dict[str, dict[str, object]],
    failures: list[str],
) -> tuple[set[str], dict[str, set[str]]]:
    vector_schema_path = BUNDLE / "schemas" / "vector.json"
    vector_schema = documents.load(vector_schema_path)
    error_schema_path = BUNDLE / "schemas" / "error.json"
    error_schema = documents.load(error_schema_path)
    request_union_path = BUNDLE / "schemas" / "request.json"
    request_union = documents.load(request_union_path)
    response_union_path = BUNDLE / "schemas" / "response.json"
    response_union = documents.load(response_union_path)
    vector_ids: set[str] = set()
    evidence: dict[str, set[str]] = {}
    for path in sorted((BUNDLE / "vectors").glob("*/*.json")):
        vector = documents.load(path)
        relative = path.relative_to(BUNDLE).as_posix()
        if vector is None or vector_schema is None:
            continue
        failures.extend(f"{relative}: {error}" for error in validate(vector, vector_schema, vector_schema_path, documents))
        if not isinstance(vector, dict):
            continue
        vector_id = vector.get("id")
        if not isinstance(vector_id, str) or not VECTOR_ID.fullmatch(vector_id):
            failures.append(f"{relative}: invalid vector id")
            continue
        if vector_id in vector_ids:
            failures.append(f"{relative}: duplicate vector id {vector_id}")
        vector_ids.add(vector_id)
        expected_layer = path.parent.name
        if vector.get("layer") != expected_layer:
            failures.append(f"{relative}: layer differs from directory")
        for requirement in vector.get("covers", []):
            if isinstance(requirement, str):
                evidence.setdefault(requirement, set()).add(vector_id)
        action = vector.get("action")
        if not isinstance(action, dict):
            continue
        if action.get("kind") == "driver" and find_embedded(action.get("command"), vector_id):
            failures.append(f"{relative}: vector identity appears in driver command data")
        if action.get("kind") == "http-sequence":
            operation = operations.get(action.get("operation"))
            steps = action.get("steps")
            expected_sequence = vector.get("expected")
            responses = expected_sequence.get("responses") if isinstance(expected_sequence, dict) else None
            if operation is None or not isinstance(steps, list) or not isinstance(responses, list) or len(steps) != len(responses):
                failures.append(f"{relative}: malformed HTTP sequence")
                continue
            prior_input: object = None
            prior_operation: object = None
            prior_result: object = None
            for index, (step, response) in enumerate(zip(steps, responses)):
                if not isinstance(step, dict) or not isinstance(step.get("request"), dict) or not isinstance(response, dict):
                    failures.append(f"{relative}: sequence step {index} has wrong shape")
                    continue
                request = step["request"]
                if find_embedded(request, vector_id):
                    failures.append(f"{relative}: vector identity appears in sequence request {index}")
                address = route_address(str(operation.get("path")), request.get("path")) if isinstance(request.get("path"), str) else None
                if request.get("method") != operation.get("method") or address is None:
                    failures.append(f"{relative}: sequence step {index} differs from registry route")
                body = request.get("body")
                if not isinstance(body, dict) or set(body) != {"op", "input"}:
                    failures.append(f"{relative}: sequence step {index} has no closed mutation body")
                    continue
                input_schema_path = BUNDLE / str(operation.get("input_schema"))
                input_schema = documents.load(input_schema_path)
                if input_schema is not None:
                    failures.extend(
                        f"{relative}: sequence input {index} rejected: {error}"
                        for error in validate(body.get("input"), input_schema, input_schema_path, documents)
                    )
                if request_union is not None:
                    failures.extend(
                        f"{relative}: sequence request union {index} rejected: {error}"
                        for error in validate(body, request_union, request_union_path, documents)
                    )
                if index == 0:
                    prior_input = body.get("input")
                    prior_operation = body.get("op")
                elif not same_json(prior_input, body.get("input")) or not same_json(prior_operation, body.get("op")):
                    failures.append(f"{relative}: replay sequence changes operation id or input")
                response_body = response.get("body")
                if not isinstance(response_body, dict):
                    continue
                if response_union is not None:
                    failures.extend(
                        f"{relative}: sequence response {index} rejected: {error}"
                        for error in validate(response_body, response_union, response_union_path, documents)
                    )
                result_schema_path = BUNDLE / str(operation.get("result_schema"))
                result_schema = documents.load(result_schema_path)
                if result_schema is not None:
                    failures.extend(
                        f"{relative}: sequence result {index} rejected: {error}"
                        for error in validate(response_body.get("result"), result_schema, result_schema_path, documents)
                    )
                if index == 0:
                    prior_result = response_body.get("result")
                elif not same_json(prior_result, response_body.get("result")):
                    failures.append(f"{relative}: replay sequence did not preserve logical result")
            continue
        if action.get("kind") == "raw-http":
            operation = operations.get(action.get("operation"))
            request = action.get("request")
            if operation is None or not isinstance(request, dict):
                failures.append(f"{relative}: raw HTTP action names no registry operation")
                continue
            if find_embedded(request, vector_id):
                failures.append(f"{relative}: vector identity appears in raw request data")
            if request.get("method") != operation.get("method") or not isinstance(request.get("path"), str) or route_address(str(operation.get("path")), request["path"]) is None:
                failures.append(f"{relative}: raw HTTP action does not match registry route")
            body_recipe = request.get("body")
            if isinstance(body_recipe, dict) and isinstance(body_recipe.get("repeat"), dict):
                repeat = body_recipe["repeat"]
                try:
                    raw = bytes.fromhex(repeat["octet_hex"]) * repeat["count"]
                except (KeyError, TypeError, ValueError) as error:
                    failures.append(f"{relative}: invalid raw body recipe: {error}")
                else:
                    if body_recipe.get("sha256") != hashlib.sha256(raw).hexdigest():
                        failures.append(f"{relative}: raw body digest differs from exact recipe")
            expected = vector.get("expected")
            if isinstance(expected, dict) and isinstance(expected.get("response"), dict) and error_schema is not None:
                response_body = expected["response"].get("body")
                failures.extend(f"{relative}: error rejected: {error}" for error in validate(response_body, error_schema, error_schema_path, documents))
            continue
        if action.get("kind") != "http":
            continue
        operation_id = action.get("operation")
        operation = operations.get(operation_id)
        if operation is None:
            failures.append(f"{relative}: unknown operation {operation_id!r}")
            continue
        request = action.get("request")
        if not isinstance(request, dict):
            continue
        if find_embedded(request, vector_id):
            failures.append(f"{relative}: vector identity appears in request data")
        method = request.get("method")
        request_path = request.get("path")
        if method != operation.get("method"):
            failures.append(f"{relative}: method differs from registry")
        address = route_address(str(operation["path"]), request_path) if isinstance(request_path, str) else None
        address_errors: list[str] = []
        if address is not None and isinstance(operation.get("address_schema"), str):
            address_schema_path = BUNDLE / operation["address_schema"]
            address_schema = documents.load(address_schema_path)
            if address_schema is not None:
                address_errors = validate(address, address_schema, address_schema_path, documents)
        path_matches = address is not None and not address_errors
        if action.get("valid_address") is True:
            if address is None:
                failures.append(f"{relative}: declared-valid address does not match normalized registry route")
            else:
                failures.extend(f"{relative}: declared-valid route address rejected: {error}" for error in address_errors)
        elif action.get("valid_address") is False and path_matches:
            failures.append(f"{relative}: declared-invalid route address unexpectedly validates")
        binding = operation.get("input_binding")
        if binding == "query":
            operation_input = request.get("query")
            if "body" in request:
                failures.append(f"{relative}: read request unexpectedly has a body")
        else:
            body = request.get("body")
            if not isinstance(body, dict) or set(body) != {"op", "input"}:
                failures.append(f"{relative}: mutation body must contain exactly op and input")
                operation_input = None
            else:
                operation_input = body.get("input")
                if request_union is not None and action.get("valid_input") is True:
                    failures.extend(
                        f"{relative}: mutation request union rejected: {error}"
                        for error in validate(body, request_union, request_union_path, documents)
                    )
        input_schema_path = BUNDLE / str(operation.get("input_schema"))
        input_schema = documents.load(input_schema_path)
        input_errors = validate(operation_input, input_schema, input_schema_path, documents) if input_schema is not None else ["input schema unavailable"]
        if action.get("valid_input") is True and input_errors:
            failures.extend(f"{relative}: declared-valid input rejected: {error}" for error in input_errors)
        if action.get("valid_input") is False and not input_errors and path_matches:
            failures.append(f"{relative}: declared-invalid request unexpectedly matches route input schema")
        check_base64_content(str(operation_id), operation_input, failures, relative)

        expected = vector.get("expected")
        if not isinstance(expected, dict) or expected.get("kind") != "http-response":
            continue
        response = expected.get("response")
        if not isinstance(response, dict) or not isinstance(response.get("body"), dict):
            continue
        status = response.get("status")
        body = response["body"]
        if isinstance(status, int) and 200 <= status < 300:
            expected_keys = {"api_version", "request_id", "result"}
            if operation.get("idempotency") == "keyed":
                expected_keys.add("operation")
            if set(body) != expected_keys:
                failures.append(f"{relative}: success envelope keys differ from route-selected shape")
            if response_union is not None:
                failures.extend(
                    f"{relative}: success response union rejected: {error}"
                    for error in validate(body, response_union, response_union_path, documents)
                )
            result_schema_path = BUNDLE / str(operation.get("result_schema"))
            result_schema = documents.load(result_schema_path)
            if result_schema is not None:
                failures.extend(f"{relative}: result rejected: {error}" for error in validate(body.get("result"), result_schema, result_schema_path, documents))
            result = body.get("result")
            if isinstance(result, dict) and result.get("kind") == "exec" and isinstance(result.get("applied"), dict):
                requested = result.get("requested")
                applied_value = result["applied"]
                if not isinstance(requested, dict) or requested.get("profile") != applied_value.get("profile") or requested.get("network") != applied_value.get("network"):
                    failures.append(f"{relative}: applied confinement weakens or differs from requested confinement")
        elif error_schema is not None:
            failures.extend(f"{relative}: error rejected: {error}" for error in validate(body, error_schema, error_schema_path, documents))
    return vector_ids, evidence


def check_coverage(
    documents: Documents,
    vector_ids: set[str],
    vector_evidence: dict[str, set[str]],
    hash_ids: set[str],
    failures: list[str],
) -> None:
    path = BUNDLE / "coverage.json"
    coverage = documents.load(path)
    schema_path = BUNDLE / "schemas" / "coverage.json"
    coverage_schema = documents.load(schema_path)
    if coverage is not None and coverage_schema is not None:
        failures.extend(f"coverage.json: {error}" for error in validate(coverage, coverage_schema, schema_path, documents))
    if not isinstance(coverage, dict) or not isinstance(coverage.get("requirements"), list):
        return
    requirements: dict[str, list[dict[str, object]]] = {}
    for entry in coverage["requirements"]:
        if not isinstance(entry, dict) or not isinstance(entry.get("id"), str):
            continue
        requirement = entry["id"]
        if requirement in requirements:
            failures.append(f"coverage.json: duplicate requirement {requirement}")
        evidence = entry.get("evidence")
        requirements[requirement] = evidence if isinstance(evidence, list) else []
    if set(requirements) != REQUIRED_COVERAGE:
        failures.append(
            "coverage.json: exact requirement inventory differs; missing "
            f"{sorted(REQUIRED_COVERAGE - set(requirements))}, extra {sorted(set(requirements) - REQUIRED_COVERAGE)}"
        )
    for requirement, entries in requirements.items():
        if not entries:
            failures.append(f"coverage.json: {requirement} has no evidence")
        for evidence in entries:
            if not isinstance(evidence, dict):
                continue
            kind = evidence.get("kind")
            evidence_id = evidence.get("id")
            if kind == "vector":
                if evidence_id not in vector_ids:
                    failures.append(f"coverage.json: {requirement} names absent vector {evidence_id}")
                elif evidence_id not in vector_evidence.get(requirement, set()):
                    failures.append(f"coverage.json: vector {evidence_id} does not self-declare {requirement}")
            elif kind == "hash-fixture" and evidence_id not in hash_ids:
                failures.append(f"coverage.json: {requirement} names absent hash fixture {evidence_id}")


def check_runner(documents: Documents, failures: list[str]) -> None:
    path = BUNDLE / "runner.json"
    runner = documents.load(path)
    schema_path = BUNDLE / "schemas" / "runner.json"
    runner_schema = documents.load(schema_path)
    if runner is not None and runner_schema is not None:
        failures.extend(f"runner.json: {error}" for error in validate(runner, runner_schema, schema_path, documents))
    if isinstance(runner, dict):
        invocation = runner.get("invocation")
        if not isinstance(invocation, dict) or invocation.get("argv") != [
            "<runner>",
            "--bundle",
            "<bundle-dir>",
            "--vector",
            "<bundle-relative-vector-path>",
            "--output",
            "<runner-result.json>",
        ]:
            failures.append("runner.json: clean-room CLI invocation changed")
        selection = runner.get("selection")
        if not isinstance(selection, dict) or selection.get("fixture_identity") != "out-of-band-only-never-request-data":
            failures.append("runner.json: fixture identity must remain out-of-band")
        result_schema = BUNDLE / "schemas" / "runner-result.json"
        if not result_schema.is_file():
            failures.append("runner.json: result schema is absent")


def main() -> int:
    failures: list[str] = []
    documents = Documents(failures)
    listed = check_manifest(documents, failures)
    json_documents = check_json_authority(BUNDLE, "0.1.0", documents, validate, failures)
    check_renderer_reproducibility(failures)
    check_schema_references(documents, listed, failures)
    operations, _ = check_registry(documents, failures)
    hash_ids = check_hashes(documents, failures)
    check_schema_fixtures(documents, failures)
    vector_ids, vector_evidence = check_vectors(documents, operations, failures)
    check_coverage(documents, vector_ids, vector_evidence, hash_ids, failures)
    check_runner(documents, failures)
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(
        f"substrate-wire 0.1.0: {len(listed)} files, {json_documents} classified JSON documents, "
        f"{len(operations)} closed operations, "
        f"{len(vector_ids)} executable vectors, {len(REQUIRED_COVERAGE)} requirements, and "
        f"{len(hash_ids)} exact hash fixtures verified"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
