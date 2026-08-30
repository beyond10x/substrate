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

# JSON authority classification is `cargo xtask check-json`, a gate step of its own: it was
# shared live machinery rather than part of this bundle's reproducibility proof, so it moved
# with the rest of the tooling (atlas AGENTS.md, section "Language").


ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "contracts" / "substrate-wire" / "0.3.0"
PREDECESSOR_BUNDLE = ROOT / "contracts" / "substrate-wire" / "0.1.0"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
VECTOR_ID = re.compile(r"^[a-z][a-z0-9-]+$")
MEDIA_TYPES = {".json": "application/json", ".md": "text/markdown"}
RUNTIME_EXECUTED_VECTORS = {
    "vectors/driver/crash-after-dispatch.json",
    "vectors/driver/crash-before-dispatch.json",
    "vectors/driver/event-push-pull-identity.json",
    "vectors/driver/event-retention-gap.json",
    "vectors/driver/event-stream-backpressure.json",
    "vectors/driver/restart-no-redispatch.json",
    "vectors/driver/snapshot-concurrent-mutation.json",
    "vectors/http/event-cross-scope-cursor.json",
    "vectors/http/exec-capacity.json",
    "vectors/http/exec-start.json",
    "vectors/http/input-body-limit.json",
    "vectors/http/ledger-capacity.json",
    "vectors/http/machinery-failure.json",
    "vectors/http/reconciliation-snapshot-create.json",
    "vectors/http/reconciliation-snapshot-empty.json",
    "vectors/http/reconciliation-snapshot-get.json",
    "vectors/http/replay-conflict.json",
    "vectors/http/workspace-capacity.json",
    "vectors/http/write-limit.json",
    "vectors/http/pipe-session-start.json",
    "vectors/http/pipe-session-missing-lease.json",
}
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
    ("exec.retire", "DELETE", "/v1/execs/{exec_id}"),
    ("session.capabilities", "GET", "/v1/pipe-sessions"),
    ("session.start", "POST", "/v1/pipe-sessions"),
    ("session.get", "GET", "/v1/pipe-sessions/{session_id}"),
    ("session.attach", "GET", "/v1/pipe-sessions/{session_id}/attach"),
    ("session.signal", "POST", "/v1/pipe-sessions/{session_id}/signal"),
    ("session.retire", "DELETE", "/v1/pipe-sessions/{session_id}"),
    ("session.lease.renew", "POST", "/v1/pipe-sessions/{session_id}/lease/renew"),
    ("operation.get", "GET", "/v1/ops/{operation_id}"),
    ("event.list", "GET", "/v1/events"),
    ("event.stream", "GET", "/v1/events/stream"),
    ("reconciliation.snapshot.create", "POST", "/v1/reconciliation-snapshots"),
    ("reconciliation.snapshot.get", "GET", "/v1/reconciliation-snapshots/{snapshot_id}"),
    ("workspace.lease.renew", "POST", "/v1/workspaces/{workspace_id}/lease/renew"),
    ("exec.lease.renew", "POST", "/v1/execs/{exec_id}/lease/renew"),
]
REQUIRED_COVERAGE = {
    'behavior.nonzero-exit-observation',
    'error.conflict',
    'error.exhausted',
    'error.failed',
    'error.refused',
    'error.unserved',
    'events.generation-reset',
    'events.pull-push-identity',
    'events.retention-gap',
    'events.source-scope',
    'events.stream-backpressure',
    'hash.different-input-conflict',
    'hash.ledger-scope',
    'hash.query-binding',
    'hash.rejected-number-binding',
    'hash.transport-exclusions',
    'lease.authorizing-operation',
    'lease.cleanup',
    'lease.clock-continuity',
    'lease.expiry',
    'lease.renewal',
    'lifecycle.crash-after-dispatch',
    'lifecycle.crash-before-dispatch',
    'lifecycle.different-input-conflict',
    'lifecycle.lost-answer-reconciliation',
    'lifecycle.post-action-observation',
    'lifecycle.restart-no-redispatch',
    'lifecycle.stable-replay',
    'lifecycle.subject-operation-isolation',
    'lifecycle.unknown-preserved',
    'ledger.capacity',
    'ledger.snapshot-independent',
    'route.event.list',
    'route.event.stream',
    'route.exec.get',
    'route.exec.lease.renew',
    'route.exec.output.get',
    'route.exec.retire',
    'route.exec.signal',
    'route.exec.start',
    'route.session.capabilities',
    'route.session.start',
    'route.session.get',
    'route.session.attach',
    'route.session.signal',
    'route.session.retire',
    'route.session.lease.renew',
    'session.distinct-identity',
    'session.profile-binding',
    'session.lease-authority',
    'session.single-attachment',
    'session.attachment-loss-containment',
    'session.protocol-failure-containment',
    'session.restart-no-redispatch',
    'session.exact-terminal-evidence',
    'route.machine.get',
    'route.operation.get',
    'route.reconciliation.snapshot.create',
    'route.reconciliation.snapshot.get',
    'route.workspace.create',
    'route.workspace.destroy',
    'route.workspace.file.delete',
    'route.workspace.file.read',
    'route.workspace.file.write',
    'route.workspace.get',
    'route.workspace.lease.renew',
    'schema.strict-request',
    'security.atomic-replacement',
    'security.bounds.delete',
    'security.bounds.input',
    'security.bounds.list',
    'security.bounds.read',
    'security.bounds.resource',
    'security.bounds.write',
    'security.daemon-credential',
    'security.daemon-environment',
    'security.daemon-fd',
    'security.git.helper',
    'security.git.hook',
    'security.git.lfs',
    'security.git.proxy',
    'security.git.rebinding',
    'security.git.redirect',
    'security.git.submodule',
    'security.no-egress',
    'security.output-draining',
    'security.path.absolute',
    'security.path.dangling-link',
    'security.path.lexical',
    'security.path.magic-link',
    'security.path.mount',
    'security.path.symlink',
    'security.post-action-observation',
    'security.process-tree-cancellation',
    'security.sandbox-unavailable',
    'security.stale-capability',
    'security.subject-resource-isolation',
    'security.timeout',
    'security.unauthenticated-reachable-startup',
    'snapshot.barrier',
    'snapshot.incomplete',
    'snapshot.empty',
    'snapshot.limit-rollback',
    'snapshot.resource-capacity',
    'snapshot.stable-pagination',
    'snapshot.terminal-exec-retirement',
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
        max_depth = contract.get("x-b10x-max-depth")
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
        raise TypeError("phase-3 operation inputs may not contain floating point values")
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


def structural_json(value: object) -> bytes:
    if value is None:
        return b"null"
    if value is True:
        return b"true"
    if value is False:
        return b"false"
    if isinstance(value, int):
        return str(value).encode("ascii")
    if isinstance(value, float):
        return json.dumps(value, allow_nan=False, separators=(",", ":")).encode("ascii")
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if isinstance(value, list):
        return b"[" + b",".join(structural_json(item) for item in value) + b"]"
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("structural JSON object keys must be strings")
        keys = sorted(value, key=lambda key: key.encode("utf-16be"))
        return b"{" + b",".join(structural_json(key) + b":" + structural_json(value[key]) for key in keys) + b"}"
    raise TypeError(f"unsupported structural JSON value {type(value).__name__}")


def canonical_input(value: object) -> tuple[str, bytes]:
    try:
        return "rfc8785-jcs", jcs(value)
    except TypeError:
        return "rejected-number-json", b"rejected-number-json:" + structural_json(value)


def decode_form_component(raw: bytes) -> str:
    decoded = bytearray()
    index = 0
    while index < len(raw):
        octet = raw[index]
        if octet == 0x2B:
            decoded.append(0x20)
            index += 1
        elif octet == 0x25:
            if index + 2 >= len(raw):
                raise ValueError("truncated percent escape")
            pair = raw[index + 1 : index + 3]
            if any(chr(value) not in "0123456789ABCDEFabcdef" for value in pair):
                raise ValueError("non-hex percent escape")
            decoded.append(int(pair, 16))
            index += 3
        else:
            decoded.append(octet)
            index += 1
    return bytes(decoded).decode("utf-8", errors="strict")


def canonical_query(raw: bytes) -> tuple[str, list[list[str]] | None, bytes]:
    try:
        pairs: list[list[str]] = []
        if raw:
            for field in raw.split(b"&"):
                name, separator, value = field.partition(b"=")
                pairs.append(
                    [decode_form_component(name), decode_form_component(value if separator else b"")]
                )
        pairs.sort(key=lambda pair: (pair[0].encode("utf-8"), pair[1].encode("utf-8")))
    except (UnicodeDecodeError, ValueError):
        tagged = b"malformed-raw\x00" + raw.hex().encode("ascii")
        return "malformed-raw", None, tagged
    tagged = b"pairs\x00" + jcs(pairs)
    return "pairs", pairs, tagged


def canonical_tuple(method: str, address: str, raw_input: object, raw_query: bytes) -> tuple[bytes, str, bytes, str, list[list[str]] | None, bytes]:
    query_mode, query_pairs, query_bytes = canonical_query(raw_query)
    input_mode, input_bytes = canonical_input(raw_input)
    fields = [b"2", method.encode("ascii"), address.encode("utf-8"), input_bytes, query_bytes]
    return b"".join(struct.pack(">I", len(field)) + field for field in fields), input_mode, input_bytes, query_mode, query_pairs, query_bytes


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
        elif name == "snapshot_id":
            expression = r"snap_[A-Za-z0-9]+"
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
    if base64.b64encode(decoded).decode("ascii") != content["data"]:
        failures.append(f"{location}: file content is not canonical padded RFC 4648 base64")
    if len(decoded) > 1048576:
        failures.append(f"{location}: decoded file content exceeds the schema ceiling")


def check_content_slice(value: object, failures: list[str], location: str) -> None:
    if not isinstance(value, dict) or value.get("kind") not in {"file", None}:
        return
    content = value.get("content")
    if not isinstance(content, dict) or content.get("encoding") != "base64" or not isinstance(content.get("data"), str):
        return
    try:
        decoded = base64.b64decode(content["data"], validate=True)
    except (binascii.Error, ValueError):
        return
    if base64.b64encode(decoded).decode("ascii") != content["data"]:
        failures.append(f"{location}: response content is not canonical padded RFC 4648 base64")
    returned = value.get("returned_bytes")
    offset = value.get("offset")
    next_offset = value.get("next_offset")
    if returned != len(decoded):
        failures.append(f"{location}: returned_bytes differs from decoded content length")
    if isinstance(offset, int) and isinstance(returned, int) and next_offset != offset + returned:
        failures.append(f"{location}: next_offset differs from offset plus returned_bytes")


def parsed_timestamp(value: object) -> dt.datetime | None:
    if not isinstance(value, str):
        return None
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def check_temporal_relations(value: object, failures: list[str], location: str) -> None:
    if not isinstance(value, dict):
        return
    accepted = parsed_timestamp(value.get("accepted_at"))
    terminal = parsed_timestamp(value.get("terminal_at"))
    if accepted is not None and terminal is not None and terminal < accepted:
        failures.append(f"{location}: operation terminal_at precedes accepted_at")
    probed = parsed_timestamp(value.get("probed_at"))
    valid_until = parsed_timestamp(value.get("valid_until"))
    if probed is not None and valid_until is not None and valid_until <= probed:
        failures.append(f"{location}: capability valid_until must follow probed_at")
    observed = parsed_timestamp(value.get("observed_at"))
    lease = value.get("lease")
    if observed is not None and isinstance(lease, dict):
        renew_by = parsed_timestamp(lease.get("renew_by"))
        if lease.get("state") == "active" and renew_by is not None and renew_by <= observed:
            failures.append(f"{location}: active lease renew_by must follow observed_at")


def check_manifest(
    documents: Documents, failures: list[str]
) -> tuple[set[str], set[str], set[str]]:
    manifest_path = BUNDLE / "bundle.json"
    manifest = documents.load(manifest_path)
    if not isinstance(manifest, dict):
        return set(), set(), set()
    expected_identity = {
        "$schema": "schemas/bundle.json",
        "api_version": "v1",
        "bundle_format": "b10x.contract-bundle.v1",
        "name": "substrate-wire",
        "origin": "b10x",
        "status": "development",
        "version": "0.3.0",
    }
    for key, expected in expected_identity.items():
        if manifest.get(key) != expected:
            failures.append(f"bundle.json: {key} must be {expected!r}")
    if manifest.get("source_base_commit") is not None:
        failures.append("bundle.json: development source_base_commit must remain null until release materialization")
    generator = manifest.get("generator")
    renderer = ROOT / "scripts" / "render-contract-bundle-0.3.0.py"
    if not isinstance(generator, dict) or generator.get("name") != "scripts/render-contract-bundle-0.3.0.py":
        failures.append("bundle.json: generator must name scripts/render-contract-bundle-0.3.0.py")
    elif generator.get("digest") != hashlib.sha256(renderer.read_bytes()).hexdigest():
        failures.append("bundle.json: generator digest does not match scripts/render-contract-bundle.py")
    conformance = manifest.get("conformance")
    executable = conformance.get("executable_vectors") if isinstance(conformance, dict) else None
    design = conformance.get("design_vectors") if isinstance(conformance, dict) else None
    actual_vectors = sorted(
        path.relative_to(BUNDLE).as_posix()
        for path in (BUNDLE / "vectors").glob("*/*.json")
    )
    if executable != sorted(RUNTIME_EXECUTED_VECTORS):
        failures.append(
            "bundle.json: executable_vectors must match the exact 0.3 vectors executed "
            "by crates/substrate-daemon/tests/contract_vectors.rs"
        )
    expected_design = sorted(set(actual_vectors) - RUNTIME_EXECUTED_VECTORS)
    if design != expected_design:
        failures.append(
            "bundle.json: design_vectors must select every bundled vector not executed "
            "by the runtime contract test"
        )
    if isinstance(executable, list) and isinstance(design, list):
        overlap = set(executable) & set(design)
        if overlap:
            failures.append(
                f"bundle.json: executable/design vector classifications overlap: {sorted(overlap)}"
            )
        if set(executable) | set(design) != set(actual_vectors):
            failures.append(
                "bundle.json: executable/design vector classifications must exhaust the bundle"
            )

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
    executable_set = set(executable) if isinstance(executable, list) else set()
    design_set = set(design) if isinstance(design, list) else set()
    return listed, executable_set, design_set


def check_renderer_reproducibility(failures: list[str]) -> None:
    renderer = ROOT / "scripts" / "render-contract-bundle-0.3.0.py"
    with tempfile.TemporaryDirectory(prefix="substrate-contract-render-") as temporary:
        clean_root = Path(temporary) / "substrate"
        clean_bundle = clean_root / "contracts" / "substrate-wire" / "0.3.0"
        clean_predecessor = clean_root / "contracts" / "substrate-wire" / "0.1.0"
        clean_scripts = clean_root / "scripts"
        clean_bundle.parent.mkdir(parents=True)
        clean_scripts.mkdir(parents=True)
        shutil.copytree(BUNDLE, clean_bundle)
        shutil.copytree(PREDECESSOR_BUNDLE, clean_predecessor)
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


def schemas_at_input_pointer(
    contract: object, schema_path: Path, raw_pointer: str, documents: Documents
) -> list[tuple[object, Path]]:
    parts = raw_pointer.removeprefix("/").split("/") if raw_pointer.startswith("/") else []

    def descend(value: object, path: Path, remaining: list[str]) -> list[tuple[object, Path]]:
        if not isinstance(value, dict):
            return []
        reference = value.get("$ref")
        if isinstance(reference, str):
            try:
                resolved, target_path = resolve_ref(reference, path, documents)
            except (KeyError, ValueError):
                return []
            return descend(resolved, target_path, remaining)
        alternatives: list[tuple[object, Path]] = []
        for keyword in ("oneOf", "anyOf", "allOf"):
            branches = value.get(keyword)
            if isinstance(branches, list):
                for branch in branches:
                    alternatives.extend(descend(branch, path, remaining))
        if not remaining:
            return [(value, path), *alternatives]
        properties = value.get("properties")
        if isinstance(properties, dict) and remaining[0] in properties:
            alternatives.extend(descend(properties[remaining[0]], path, remaining[1:]))
        return alternatives

    unique: dict[str, tuple[object, Path]] = {}
    for value, path in descend(contract, schema_path, parts):
        key = f"{path}:{json.dumps(value, sort_keys=True)}"
        unique[key] = (value, path)
    return list(unique.values())


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
        failures.append(f"operations.json: route inventory differs from exact phase-4 set: {observed!r}")
    by_id: dict[str, dict[str, object]] = {}
    capability_path = BUNDLE / "schemas" / "capability.json"
    capability = documents.load(capability_path)
    fact_schemas: dict[str, object] = {}
    if isinstance(capability, dict):
        properties = capability.get("properties")
        facts = properties.get("facts") if isinstance(properties, dict) else None
        fact_schemas = facts.get("properties", {}) if isinstance(facts, dict) and isinstance(facts.get("properties"), dict) else {}
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
            continue
        input_schema_path = BUNDLE / str(entry.get("input_schema"))
        input_schema = documents.load(input_schema_path)
        for predicate_index, predicate in enumerate(predicates):
            location = f"operations.json: {operation_id} predicate[{predicate_index}]"
            if not isinstance(predicate, dict):
                failures.append(f"{location} is not an object")
                continue
            fact = predicate.get("fact")
            fact_schema = fact_schemas.get(fact) if isinstance(fact, str) else None
            if fact_schema is None:
                failures.append(f"{location} references undeclared capability fact {fact!r}")
                continue
            operator = predicate.get("op")
            if "value" in predicate:
                if operator != "eq":
                    failures.append(f"{location} literal comparison must use eq")
                failures.extend(
                    f"{location} literal is incompatible with fact: {error}"
                    for error in validate(predicate.get("value"), fact_schema, capability_path, documents)
                )
            pointer_value = predicate.get("input_pointer")
            if isinstance(pointer_value, str) and input_schema is not None:
                input_targets = schemas_at_input_pointer(input_schema, input_schema_path, pointer_value, documents)
                if not input_targets:
                    failures.append(f"{location} input pointer {pointer_value!r} does not resolve")
                fact_type = fact_schema.get("type") if isinstance(fact_schema, dict) else None
                if fact_type is None and isinstance(fact_schema, dict) and isinstance(fact_schema.get("const"), int):
                    fact_type = "integer"
                if operator == "gte" and fact_type != "integer":
                    failures.append(f"{location} gte requires an integer capability fact")
                if operator == "one_of" and not (isinstance(fact_schema, dict) and isinstance(fact_schema.get("items"), dict)):
                    failures.append(f"{location} one_of requires an array capability fact")
                transform = predicate.get("transform")
                if transform == "base64-decoded-length":
                    if not any(isinstance(target, dict) and (target.get("type") == "string" or "$ref" in target) for target, _ in input_targets):
                        failures.append(f"{location} base64 transform requires a string input")
                elif operator == "gte" and not any(isinstance(target, dict) and target.get("type") == "integer" for target, _ in input_targets):
                    failures.append(f"{location} gte input must resolve to an integer")
            when = predicate.get("when")
            if isinstance(when, dict) and input_schema is not None:
                when_pointer = when.get("input_pointer")
                if not isinstance(when_pointer, str) or not schemas_at_input_pointer(input_schema, input_schema_path, when_pointer, documents):
                    failures.append(f"{location} when input pointer does not resolve")
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
    if hashing.get("format") != "b10x.substrate-request-hash.v2":
        failures.append("hashing.json: only request-hash v2 is authoritative in 0.2")
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
            raw_query_value = case["raw_query"]
            if not isinstance(raw_query_value, dict) or raw_query_value.get("encoding") != "hex":
                raise TypeError("raw_query must use the closed hex representation")
            raw_query = bytes.fromhex(raw_query_value["data"])
            tuple_bytes, input_mode, input_bytes, query_mode, query_pairs, query_bytes = canonical_tuple(
                case["method"], case["normalized_address"], case["raw_input"], raw_query
            )
        except (KeyError, TypeError, UnicodeError, ValueError) as error:
            failures.append(f"hash fixture {case_id}: cannot canonicalize: {error}")
            continue
        if case.get("hash_version") != 2:
            failures.append(f"hash fixture {case_id}: hash version is not 2")
        if not normalized_address(case["normalized_address"]):
            failures.append(f"hash fixture {case_id}: address is not normalized")
        if case.get("input_mode") != input_mode:
            failures.append(f"hash fixture {case_id}: canonical input mode differs")
        if case.get("canonical_input_hex") != input_bytes.hex():
            failures.append(f"hash fixture {case_id}: canonical input bytes differ")
        if case.get("query_mode") != query_mode or not same_json(case.get("canonical_query"), query_pairs):
            failures.append(f"hash fixture {case_id}: canonical query classification differs")
        if case.get("canonical_query_hex") != query_bytes.hex():
            failures.append(f"hash fixture {case_id}: canonical query bytes differ")
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
    required_query_cases = {
        "query-duplicates-base",
        "query-duplicates-reordered",
        "query-malformed-percent",
        "query-malformed-utf8",
    }
    if not required_query_cases.issubset(cases):
        failures.append("fixtures/canonical-hash.json: required duplicate/order/malformed query cases are absent")
    required_rejected_number_cases = {"rejected-float-base", "rejected-float-conflict"}
    if not required_rejected_number_cases.issubset(cases):
        failures.append("fixtures/canonical-hash.json: required rejected-number binding cases are absent")
    elif any(cases[case_id].get("input_mode") != "rejected-number-json" for case_id in required_rejected_number_cases):
        failures.append("fixtures/canonical-hash.json: rejected-number cases do not use repository fallback")
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
            check_temporal_relations(instance, failures, f"{display(path)}: valid[{index}]")
        for index, case in enumerate(value.get("invalid", [])):
            if not isinstance(case, dict):
                continue
            errors = validate(case.get("instance"), contract, target_path, documents)
            if not errors:
                failures.append(f"{display(path)}: invalid[{index}] unexpectedly validates")


def check_event_page(page: dict[str, object], location: str, failures: list[str]) -> None:
    generation = page.get("generation")
    through = page.get("through_seq")
    items = page.get("items")
    source_scope = page.get("source_scope")
    if isinstance(generation, int) and isinstance(through, int) and isinstance(source_scope, str):
        if page.get("next_cursor") != f"ev2.{source_scope}.{generation}.{through}":
            failures.append(f"{location}: event next_cursor is not bound to source_scope/generation/through_seq")
    if not isinstance(items, list):
        return
    sequences: list[int] = []
    for index, event in enumerate(items):
        if not isinstance(event, dict):
            continue
        if event.get("generation") != generation:
            failures.append(f"{location}: event item {index} generation differs from page")
        sequence = event.get("seq")
        if isinstance(sequence, int):
            sequences.append(sequence)
        cause = event.get("cause")
        if not isinstance(cause, dict) or cause.get("kind") not in {"operation", "control"}:
            failures.append(f"{location}: event item {index} has no typed cause")
    if sequences != sorted(sequences) or len(sequences) != len(set(sequences)):
        failures.append(f"{location}: event page sequences are not strictly ordered")
    if sequences and sequences[-1] > through:
        failures.append(f"{location}: event page contains sequence after through_seq")
    first = page.get("first_retained_seq")
    if isinstance(first, int) and sequences and sequences[0] < first:
        failures.append(f"{location}: event page contains item before first_retained_seq")


def check_snapshot_metadata(metadata: dict[str, object], location: str, failures: list[str]) -> None:
    partitions = metadata.get("partitions")
    history = metadata.get("history")
    item_count = metadata.get("item_count")
    if isinstance(partitions, dict) and isinstance(item_count, int):
        counts = [partitions.get(name) for name in ("workspaces", "execs", "provenance_events")]
        if all(isinstance(value, int) for value in counts) and sum(counts) != item_count:
            failures.append(f"{location}: snapshot partition counts do not sum to item_count")
        if item_count > 4096:
            failures.append(f"{location}: snapshot exceeds total 4096-item authority")
    if isinstance(history, dict) and isinstance(partitions, dict):
        if history.get("item_count") != partitions.get("provenance_events"):
            failures.append(f"{location}: snapshot history count differs from provenance_events partition")
        history_count = history.get("item_count")
        first = history.get("first_seq")
        history_through = history.get("through_seq")
        if history_count == 0 and (first is not None or history_through != 0):
            failures.append(f"{location}: empty snapshot history must have null first_seq and through_seq zero")
        if isinstance(history_count, int) and history_count > 0:
            if not isinstance(first, int) or not isinstance(history_through, int) or first > history_through:
                failures.append(f"{location}: non-empty snapshot history has incoherent sequence bounds")
        barrier = metadata.get("through_seq")
        if (
            isinstance(history_count, int)
            and history_count > 0
            and isinstance(history_through, int)
            and isinstance(barrier, int)
            and history_through >= barrier
        ):
            failures.append(
                f"{location}: snapshot provenance must precede the control barrier"
            )
    generation = metadata.get("generation")
    through = metadata.get("through_seq")
    source_scope = metadata.get("source_scope")
    if isinstance(generation, int) and isinstance(through, int) and isinstance(source_scope, str) and metadata.get("resume_cursor") != f"ev2.{source_scope}.{generation}.{through}":
        failures.append(f"{location}: snapshot resume_cursor is not bound to source_scope/generation/through_seq")


def check_snapshot_page(page: dict[str, object], location: str, failures: list[str]) -> None:
    items = page.get("items")
    if not isinstance(items, list):
        return
    ordinals: list[int] = []
    for index, item in enumerate(items):
        if not isinstance(item, dict):
            continue
        ordinal = item.get("ordinal")
        if isinstance(ordinal, int):
            ordinals.append(ordinal)
        kind = item.get("kind")
        identifier = item.get("id")
        value = item.get("value")
        if kind == "workspace" and isinstance(value, dict) and identifier != f"workspace:{value.get('id')}":
            failures.append(f"{location}: workspace snapshot item {index} id/value mismatch")
        elif kind == "exec" and isinstance(value, dict) and identifier != f"exec:{value.get('id')}":
            failures.append(f"{location}: exec snapshot item {index} id/value mismatch")
        elif kind == "provenance-event" and isinstance(value, dict):
            if identifier != f"event:{value.get('generation')}:{value.get('seq')}":
                failures.append(f"{location}: provenance-event snapshot item {index} id/value mismatch")
            if value.get("generation") != page.get("generation") or not isinstance(value.get("seq"), int) or value["seq"] > page.get("through_seq", -1):
                failures.append(f"{location}: provenance-event snapshot item {index} is outside snapshot barrier")
    if ordinals != sorted(ordinals) or len(ordinals) != len(set(ordinals)):
        failures.append(f"{location}: snapshot ordinals are not strictly ordered")
    next_cursor = page.get("next_cursor")
    if next_cursor is not None and ordinals and next_cursor != f"sp2.{page.get('snapshot')}.{ordinals[-1]}":
        failures.append(f"{location}: snapshot next_cursor does not encode snapshot/last ordinal")
    if page.get("complete") is True and next_cursor is not None:
        failures.append(f"{location}: complete snapshot page must not expose next_cursor")
    if page.get("complete") is False and next_cursor is None:
        failures.append(f"{location}: incomplete snapshot page must expose next_cursor")


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
        elif binding == "body":
            operation_input = request.get("body")
            if operation.get("idempotency") != "none" or operation_input != {}:
                failures.append(f"{relative}: control body must be exact empty JSON object and non-keyed")
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
            check_content_slice(result, failures, relative)
            check_temporal_relations(result, failures, relative)
            expected_success = {
                "workspace.create": {201},
                "exec.start": {200, 202},
                "session.start": {202},
                "reconciliation.snapshot.create": {201},
            }.get(str(operation_id), {200})
            if status not in expected_success:
                failures.append(f"{relative}: success status {status} is not allowed for {operation_id}")
            if operation_id == "reconciliation.snapshot.create" and isinstance(result, dict):
                check_snapshot_metadata(result, relative, failures)
            if operation_id == "reconciliation.snapshot.get" and isinstance(result, dict):
                check_snapshot_page(result, relative, failures)
            if operation_id == "event.list" and isinstance(result, dict):
                check_event_page(result, relative, failures)
            if isinstance(result, dict) and result.get("kind") == "exec" and isinstance(result.get("applied"), dict):
                requested = result.get("requested")
                applied_value = result["applied"]
                if not isinstance(requested, dict) or requested.get("profile") != applied_value.get("profile") or requested.get("network") != applied_value.get("network"):
                    failures.append(f"{relative}: applied confinement weakens or differs from requested confinement")
        elif error_schema is not None:
            failures.extend(f"{relative}: error rejected: {error}" for error in validate(body, error_schema, error_schema_path, documents))
            detail = body.get("error")
            allowed_error_status = {
                "refused": {400, 404, 422},
                "conflict": {409},
                "unserved": {501},
                "exhausted": {413, 429, 507},
                "failed": {500, 502, 503},
            }
            if isinstance(detail, dict) and status not in allowed_error_status.get(detail.get("class"), set()):
                failures.append(f"{relative}: error class {detail.get('class')!r} is incompatible with status {status!r}")
            if operation.get("idempotency") != "keyed" and isinstance(detail, dict) and "operation" in detail:
                failures.append(f"{relative}: non-keyed route error must not claim an operation id")
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


def check_predecessor_errata(documents: Documents, failures: list[str]) -> None:
    compatibility = documents.load(BUNDLE / "compatibility.json")
    if not isinstance(compatibility, dict):
        return
    expected_additive_keys = {
        "$schema",
        "contract",
        "development_constraints",
        "request_policy",
        "response_policy",
        "status",
        "supported_api_majors",
        "version",
    }
    if set(compatibility) != expected_additive_keys:
        failures.append("compatibility.json: closed additive root fields differ")
    return
    expected_root_keys = {
        "$schema",
        "contract",
        "development_constraints",
        "errata_from",
        "request_policy",
        "response_policy",
        "status",
        "supported_api_majors",
        "version",
    }
    if set(compatibility) != expected_root_keys:
        failures.append("compatibility.json: closed root fields differ")
    errata = compatibility.get("errata_from")
    if not isinstance(errata, dict) or set(errata) != {"records", "version"}:
        failures.append("compatibility.json: errata_from must be a closed authority")
        return
    if errata.get("version") != "0.1.0":
        failures.append("compatibility.json: errata predecessor must be 0.1.0")
    records = errata.get("records")
    if not isinstance(records, list):
        failures.append("compatibility.json: errata records must be an array")
        return
    expected = {
        "vectors/http/machinery-failure.json": (
            "response.body.error.retriable=true",
            "response.body.error.retriable=false",
        ),
        "vectors/driver/crash-before-dispatch.json": (
            "outcome.operation_state_after_restart=accepted",
            "outcome.operation_state_after_restart=unknown",
        ),
        "vectors/http/input-body-limit.json": (
            "setup.body-limit.limit_bytes=1048576 and request.body.repeat.count=1048577",
            "setup.body-limit.limit_bytes=2097152 and request.body.repeat.count=2097153",
        ),
        "vectors/http/write-limit.json": (
            "response.body.error.retriable=true",
            "response.body.error.retriable=false",
        ),
    }
    seen: set[str] = set()
    record_keys = {
        "compatibility_impact",
        "corrected_expectation",
        "erroneous_expectation",
        "predecessor_path",
        "predecessor_sha256",
        "reason",
        "replacement_path",
        "replacement_sha256",
    }
    for index, record in enumerate(records):
        if not isinstance(record, dict) or set(record) != record_keys:
            failures.append(f"compatibility.json: errata record {index} fields differ")
            continue
        predecessor_path = record.get("predecessor_path")
        replacement_path = record.get("replacement_path")
        if not isinstance(predecessor_path, str) or predecessor_path not in expected:
            failures.append(f"compatibility.json: unexpected predecessor path {predecessor_path!r}")
            continue
        seen.add(predecessor_path)
        if replacement_path != predecessor_path:
            failures.append(f"compatibility.json: {predecessor_path} replacement path changed")
            continue
        erroneous, corrected = expected[predecessor_path]
        if record.get("erroneous_expectation") != erroneous:
            failures.append(f"compatibility.json: {predecessor_path} erroneous expectation changed")
        if record.get("corrected_expectation") != corrected:
            failures.append(f"compatibility.json: {predecessor_path} corrected expectation changed")
        for root, digest_field in (
            (PREDECESSOR_BUNDLE, "predecessor_sha256"),
            (BUNDLE, "replacement_sha256"),
        ):
            digest = record.get(digest_field)
            target = root / predecessor_path
            if not isinstance(digest, str) or SHA256.fullmatch(digest) is None:
                failures.append(f"compatibility.json: {predecessor_path} invalid {digest_field}")
            elif not target.is_file() or hashlib.sha256(target.read_bytes()).hexdigest() != digest:
                failures.append(f"compatibility.json: {predecessor_path} stale {digest_field}")
        impact = record.get("compatibility_impact")
        if not isinstance(impact, str) or "must select 0.2" not in impact:
            failures.append(f"compatibility.json: {predecessor_path} hides consumer impact")
    if seen != set(expected):
        failures.append("compatibility.json: exact three-entry predecessor errata inventory differs")


def main() -> int:
    failures: list[str] = []
    documents = Documents(failures)
    listed, executable_vectors, design_vectors = check_manifest(documents, failures)
    check_renderer_reproducibility(failures)
    check_schema_references(documents, listed, failures)
    operations, _ = check_registry(documents, failures)
    hash_ids = check_hashes(documents, failures)
    check_schema_fixtures(documents, failures)
    vector_ids, vector_evidence = check_vectors(documents, operations, failures)
    check_coverage(documents, vector_ids, vector_evidence, hash_ids, failures)
    check_runner(documents, failures)
    check_predecessor_errata(documents, failures)
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(
        f"substrate-wire 0.3.0: {len(listed)} files, "
        f"{len(operations)} closed operations, "
        f"{len(executable_vectors)} executable vectors, {len(design_vectors)} design vectors, "
        f"{len(REQUIRED_COVERAGE)} requirements, and "
        f"{len(hash_ids)} exact hash fixtures verified"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
