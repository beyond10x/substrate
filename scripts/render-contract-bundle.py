#!/usr/bin/env python3
"""Render the hand-authored substrate-wire development authority.

This is build tooling for the contract bundle, not substrate runtime code.  The
checked-in JSON remains the review and distribution surface; this renderer keeps
the deliberately repetitive closed route schemas and fixtures deterministic.
"""

from __future__ import annotations

import hashlib
import json
import struct
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / "contracts" / "substrate-wire" / "0.1.0"
SCHEMA_URI = "https://json-schema.org/draft/2020-12/schema"
FIXED_TIME = "2026-08-13T12:00:00Z"
CAPABILITY_SNAPSHOT = "sha256:" + "7" * 64
SUBJECT = "local:1000"
DEPLOYMENT = "dep_vector"


def write_json(relative: str, value: object) -> None:
    path = BUNDLE / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def schema(title: str, *, schema_id: str | None = None, **values: object) -> dict[str, object]:
    result: dict[str, object] = {
        "$schema": SCHEMA_URI,
        "title": title,
    }
    if schema_id is not None:
        result["$id"] = f"urn:daemonloom:substrate-wire:0.1.0:{schema_id}"
    result.update(values)
    return result


def closed_object(
    properties: dict[str, object],
    required: list[str] | None = None,
    **values: object,
) -> dict[str, object]:
    result: dict[str, object] = {
        "additionalProperties": False,
        "properties": properties,
        "type": "object",
    }
    if required is not None:
        result["required"] = required
    result.update(values)
    return result


COMMON = schema(
    "Substrate phase-2 scalar and envelope definitions",
    schema_id="common",
    **{
        "$defs": {
            "api-version": {"const": "v1"},
            "operation-id": {
                "maxLength": 128,
                "minLength": 16,
                "pattern": "^[A-Za-z0-9_-]+$",
                "type": "string",
            },
            "request-id": {
                "maxLength": 128,
                "minLength": 8,
                "pattern": "^[A-Za-z0-9_-]+$",
                "type": "string",
            },
            "workspace-id": {"pattern": "^ws_[A-Za-z0-9]+$", "type": "string"},
            "exec-id": {"pattern": "^ex_[A-Za-z0-9]+$", "type": "string"},
            "timestamp": {"format": "date-time", "type": "string"},
            "relative-path": {
                "maxLength": 4096,
                "minLength": 1,
                "pattern": "^(?!/)(?!.*(?:^|/)\\.\\.(?:/|$))(?!.*\\u0000).+$",
                "type": "string",
                "x-daemonloom-max-depth": 64,
            },
            "labels": {
                "additionalProperties": {"maxLength": 256, "type": "string"},
                "maxProperties": 64,
                "propertyNames": {
                    "maxLength": 128,
                    "pattern": "^[A-Za-z0-9_.-]+$",
                },
                "type": "object",
            },
            "empty-input": closed_object({}, []),
        }
    },
)


CAPABILITY = schema(
    "Phase-2 machine capability snapshot",
    schema_id="capability",
    **closed_object(
        {
            "config_generation": {"minimum": 1, "type": "integer"},
            "driver": {"const": "host"},
            "driver_version": {"minLength": 1, "type": "string"},
            "facts": closed_object(
                {
                    "exec.argv-only": {"const": True},
                    "exec.cgroup-kill": {"const": True},
                    "exec.cgroup-limits": closed_object(
                        {
                            "cpu": {"const": True},
                            "memory": {"const": True},
                            "processes": {"const": True},
                        },
                        ["processes", "memory", "cpu"],
                    ),
                    "exec.namespaces": closed_object(
                        {
                            name: {"const": True}
                            for name in ("ipc", "mount", "network", "pid", "user", "uts")
                        },
                        ["user", "mount", "pid", "ipc", "uts", "network"],
                    ),
                    "exec.no-egress": {"const": True},
                    "exec.output-limit-bytes": {"minimum": 1, "type": "integer"},
                    "exec.signals": {
                        "items": {"enum": ["INT", "TERM", "KILL"]},
                        "maxItems": 3,
                        "minItems": 1,
                        "type": "array",
                        "uniqueItems": True,
                    },
                    "workspace.atomic-replace": {"const": True},
                    "workspace.guarded-io": {"const": True},
                    "workspace.list-limit-items": {"minimum": 1, "type": "integer"},
                    "workspace.max-file-bytes": {"minimum": 1, "type": "integer"},
                    "workspace.openat2-beneath": {"const": True},
                    "workspace.read-limit-bytes": {"minimum": 1, "type": "integer"},
                },
                [],
            ),
            "probed_at": {"$ref": "common.json#/$defs/timestamp"},
            "snapshot": {"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"},
            "valid_until": {"$ref": "common.json#/$defs/timestamp"},
        },
        ["snapshot", "driver", "driver_version", "config_generation", "probed_at", "facts"],
    ),
)


ERROR = schema(
    "Substrate answered failure",
    schema_id="error",
    **{
        "$defs": {
            "detail": closed_object(
                {
                    "address": {"maxLength": 512, "type": "string"},
                    "class": {
                        "enum": ["refused", "conflict", "unserved", "exhausted", "failed"]
                    },
                    "code": {
                        "pattern": "^[a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)+$",
                        "type": "string",
                    },
                    "message": {"maxLength": 1024, "minLength": 1, "type": "string"},
                    "operation": {"$ref": "common.json#/$defs/operation-id"},
                    "retriable": {"type": "boolean"},
                },
                ["class", "code", "message", "retriable"],
            )
        },
        **closed_object(
            {
                "api_version": {"$ref": "common.json#/$defs/api-version"},
                "error": {"$ref": "#/$defs/detail"},
                "request_id": {"$ref": "common.json#/$defs/request-id"},
            },
            ["api_version", "request_id", "error"],
        ),
    },
)


CONFINEMENT_REQUEST = closed_object(
    {
        "capability_snapshot": {"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"},
        "network": {"enum": ["none", "aperture"]},
        "profile": {"const": "workspace"},
        "require": {"const": True},
    },
    ["capability_snapshot", "profile", "network", "require"],
)

CONFINEMENT_APPLIED = closed_object(
    {
        "capability_snapshot": {"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"},
        "cgroup": {"maxLength": 512, "minLength": 1, "type": "string"},
        "filesystem": {"const": "workspace-rw-system-ro"},
        "network": {"const": "none"},
        "profile": {"const": "workspace"},
    },
    ["capability_snapshot", "profile", "filesystem", "network", "cgroup"],
)

CONFINEMENT_REQUEST_APPLIED = closed_object(
    {
        "capability_snapshot": {"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"},
        "network": {"const": "none"},
        "profile": {"const": "workspace"},
        "require": {"const": True},
    },
    ["capability_snapshot", "profile", "network", "require"],
)

EXIT = closed_object(
    {
        "code": {"maximum": 255, "minimum": 0, "type": ["integer", "null"]},
        "signal": {"enum": ["INT", "TERM", "KILL", None]},
    },
    ["code", "signal"],
    oneOf=[
        {"properties": {"code": {"type": "integer"}, "signal": {"type": "null"}}},
        {"properties": {"code": {"type": "null"}, "signal": {"enum": ["INT", "TERM", "KILL"]}}},
    ],
)

WORKSPACE = closed_object(
    {
        "id": {"$ref": "common.json#/$defs/workspace-id"},
        "kind": {"const": "workspace"},
        "labels": {"$ref": "common.json#/$defs/labels"},
        "observed_at": {"$ref": "common.json#/$defs/timestamp"},
        "state": {"enum": ["ready", "destroying", "destroyed", "unknown"]},
    },
    ["kind", "id", "state", "observed_at", "labels"],
)

EXEC_BASE_PROPERTIES = {
    "applied": {"oneOf": [CONFINEMENT_APPLIED, {"type": "null"}]},
    "exit": {"oneOf": [EXIT, {"type": "null"}]},
    "id": {"$ref": "common.json#/$defs/exec-id"},
    "kind": {"const": "exec"},
    "observed_at": {"$ref": "common.json#/$defs/timestamp"},
    "requested": CONFINEMENT_REQUEST,
    "state": {"enum": ["accepted", "running", "exited", "cancelled", "unknown"]},
    "workspace": {"$ref": "common.json#/$defs/workspace-id"},
}

EXEC = {
    "oneOf": [
        closed_object(
            {**EXEC_BASE_PROPERTIES, "applied": {"type": "null"}, "exit": {"type": "null"}, "state": {"const": "accepted"}},
            ["kind", "id", "workspace", "state", "observed_at", "requested", "applied", "exit"],
        ),
        closed_object(
            {**EXEC_BASE_PROPERTIES, "applied": CONFINEMENT_APPLIED, "exit": {"type": "null"}, "requested": CONFINEMENT_REQUEST_APPLIED, "state": {"const": "running"}},
            ["kind", "id", "workspace", "state", "observed_at", "requested", "applied", "exit"],
        ),
        closed_object(
            {**EXEC_BASE_PROPERTIES, "applied": CONFINEMENT_APPLIED, "exit": EXIT, "requested": CONFINEMENT_REQUEST_APPLIED, "state": {"enum": ["exited", "cancelled"]}},
            ["kind", "id", "workspace", "state", "observed_at", "requested", "applied", "exit"],
        ),
        closed_object(
            {**EXEC_BASE_PROPERTIES, "state": {"const": "unknown"}},
            ["kind", "id", "workspace", "state", "observed_at", "requested", "applied", "exit"],
        ),
    ]
}

RESOURCE = schema(
    "Phase-2 observed resource",
    schema_id="resource",
    **{
        "$defs": {
            "confinement-request": CONFINEMENT_REQUEST,
            "confinement-applied": CONFINEMENT_APPLIED,
            "exit": EXIT,
            "workspace": WORKSPACE,
            "exec": EXEC,
        },
        "oneOf": [{"$ref": "#/$defs/workspace"}, {"$ref": "#/$defs/exec"}],
    },
)


INPUT_SCHEMAS: dict[str, dict[str, object]] = {}
RESULT_SCHEMAS: dict[str, dict[str, object]] = {}
ADDRESS_SCHEMAS: dict[str, dict[str, object]] = {}


def add_input(name: str, value: dict[str, object]) -> str:
    path = f"schemas/inputs/{name}.json"
    INPUT_SCHEMAS[path] = schema(f"{name} operation input", schema_id=f"input:{name}", **value)
    return path


def add_result(name: str, value: dict[str, object]) -> str:
    path = f"schemas/results/{name}.json"
    RESULT_SCHEMAS[path] = schema(f"{name} operation result", schema_id=f"result:{name}", **value)
    return path


def add_address(name: str, value: dict[str, object]) -> str:
    path = f"schemas/addresses/{name}.json"
    ADDRESS_SCHEMAS[path] = schema(f"{name} normalized route address", schema_id=f"address:{name}", **value)
    return path


EMPTY = closed_object({}, [])
machine_input = add_input("machine-get", EMPTY)
workspace_create_input = add_input(
    "workspace-create",
    closed_object(
        {
            "labels": {"$ref": "../common.json#/$defs/labels"},
            "source": {
                "oneOf": [
                    {"const": "empty"},
                    closed_object(
                        {
                            "git": closed_object(
                                {
                                    "depth": {"maximum": 1000, "minimum": 1, "type": "integer"},
                                    "ref": {"maxLength": 512, "minLength": 1, "type": "string"},
                                    "source": {"maxLength": 128, "minLength": 1, "type": "string"},
                                },
                                ["source", "ref", "depth"],
                            )
                        },
                        ["git"],
                    ),
                ]
            },
        },
        ["source", "labels"],
    ),
)
workspace_get_input = add_input("workspace-get", EMPTY)
workspace_file_read_input = add_input(
    "workspace-file-read",
    {
        "oneOf": [
            closed_object(
                {
                    "limit_bytes": {"maximum": 1048576, "minimum": 1, "type": "integer"},
                    "mode": {"const": "file"},
                    "offset": {"minimum": 0, "type": "integer"},
                },
                ["mode", "offset", "limit_bytes"],
            ),
            closed_object(
                {
                    "cursor": {"type": ["string", "null"]},
                    "limit_items": {"maximum": 1000, "minimum": 1, "type": "integer"},
                    "mode": {"const": "directory"},
                },
                ["mode", "cursor", "limit_items"],
            ),
        ]
    },
)
workspace_file_write_input = add_input(
    "workspace-file-write",
    closed_object(
        {
            "content": closed_object(
                {
                    "data": {"maxLength": 1398104, "pattern": "^[A-Za-z0-9+/]*={0,2}$", "type": "string"},
                    "encoding": {"const": "base64"},
                },
                ["encoding", "data"],
            ),
        },
        ["content"],
    ),
)
workspace_file_delete_input = add_input("workspace-file-delete", EMPTY)
workspace_destroy_input = add_input("workspace-destroy", EMPTY)

EXEC_START_INPUT = closed_object(
    {
        "argv": {
            "items": {"maxLength": 4096, "minLength": 1, "pattern": "^[^\\u0000]+$", "type": "string"},
            "maxItems": 256,
            "minItems": 1,
            "type": "array",
        },
        "env": closed_object(
            {
                "allow": {
                    "items": {"enum": ["LANG", "LC_ALL", "PATH", "TERM", "TZ"]},
                    "maxItems": 5,
                    "type": "array",
                    "uniqueItems": True,
                },
                "set": {
                    "additionalProperties": {"maxLength": 4096, "type": "string"},
                    "maxProperties": 64,
                    "propertyNames": {
                        "allOf": [
                            {"pattern": "^[A-Z][A-Z0-9_]{0,127}$"},
                            {
                                "not": {
                                    "pattern": "(?i)(authorization|bearer|credential|password|secret|token|proxy)"
                                }
                            },
                        ]
                    },
                    "type": "object",
                },
            },
            ["allow", "set"],
        ),
        "limits": closed_object(
            {
                "cpu_millis": {"maximum": 86400000, "minimum": 1, "type": "integer"},
                "memory_bytes": {"maximum": 1099511627776, "minimum": 1048576, "type": "integer"},
                "output_bytes": {"maximum": 1048576, "minimum": 1, "type": "integer"},
                "processes": {"maximum": 4096, "minimum": 1, "type": "integer"},
                "timeout_ms": {"maximum": 86400000, "minimum": 1, "type": "integer"},
            },
            ["timeout_ms", "output_bytes", "processes", "memory_bytes", "cpu_millis"],
        ),
        "sandbox": CONFINEMENT_REQUEST,
        "wait": {"type": "boolean"},
        "workspace": {"$ref": "../common.json#/$defs/workspace-id"},
    },
    ["workspace", "argv", "env", "sandbox", "limits", "wait"],
)
exec_start_input = add_input("exec-start", EXEC_START_INPUT)
exec_get_input = add_input("exec-get", EMPTY)
exec_output_input = add_input(
    "exec-output-get",
    closed_object(
        {
            "limit_bytes": {"maximum": 1048576, "minimum": 1, "type": "integer"},
            "offset": {"minimum": 0, "type": "integer"},
            "stream": {"enum": ["stdout", "stderr"]},
        },
        ["stream", "offset", "limit_bytes"],
    ),
)
exec_signal_input = add_input(
    "exec-signal",
    closed_object(
        {
            "grace_ms": {"maximum": 30000, "minimum": 0, "type": "integer"},
            "signal": {"enum": ["INT", "TERM", "KILL"]},
        },
        ["signal", "grace_ms"],
    ),
)
operation_get_input = add_input("operation-get", EMPTY)

ADDRESS_BY_OPERATION = {
    "machine.get": add_address("machine-get", EMPTY),
    "workspace.create": add_address("workspace-create", EMPTY),
    "workspace.get": add_address(
        "workspace-get",
        closed_object({"workspace_id": {"$ref": "../common.json#/$defs/workspace-id"}}, ["workspace_id"]),
    ),
    "workspace.file.read": add_address(
        "workspace-file-read",
        closed_object(
            {
                "path": {"$ref": "../common.json#/$defs/relative-path"},
                "workspace_id": {"$ref": "../common.json#/$defs/workspace-id"},
            },
            ["workspace_id", "path"],
        ),
    ),
    "workspace.file.write": add_address(
        "workspace-file-write",
        closed_object(
            {
                "path": {"$ref": "../common.json#/$defs/relative-path"},
                "workspace_id": {"$ref": "../common.json#/$defs/workspace-id"},
            },
            ["workspace_id", "path"],
        ),
    ),
    "workspace.file.delete": add_address(
        "workspace-file-delete",
        closed_object(
            {
                "path": {"$ref": "../common.json#/$defs/relative-path"},
                "workspace_id": {"$ref": "../common.json#/$defs/workspace-id"},
            },
            ["workspace_id", "path"],
        ),
    ),
    "workspace.destroy": add_address(
        "workspace-destroy",
        closed_object({"workspace_id": {"$ref": "../common.json#/$defs/workspace-id"}}, ["workspace_id"]),
    ),
    "exec.start": add_address("exec-start", EMPTY),
    "exec.get": add_address(
        "exec-get", closed_object({"exec_id": {"$ref": "../common.json#/$defs/exec-id"}}, ["exec_id"])
    ),
    "exec.output.get": add_address(
        "exec-output-get", closed_object({"exec_id": {"$ref": "../common.json#/$defs/exec-id"}}, ["exec_id"])
    ),
    "exec.signal": add_address(
        "exec-signal", closed_object({"exec_id": {"$ref": "../common.json#/$defs/exec-id"}}, ["exec_id"])
    ),
    "operation.get": add_address(
        "operation-get",
        closed_object({"operation_id": {"$ref": "../common.json#/$defs/operation-id"}}, ["operation_id"]),
    ),
}


machine_result = add_result("machine-get", {"$ref": "../capability.json"})
workspace_create_result = add_result(
    "workspace-create",
    {"allOf": [{"$ref": "../resource.json#/$defs/workspace"}, {"properties": {"state": {"const": "ready"}}}]},
)
workspace_get_result = add_result("workspace-get", {"$ref": "../resource.json#/$defs/workspace"})

FILE_SLICE = closed_object(
    {
        "content": closed_object(
            {
                "data": {"pattern": "^[A-Za-z0-9+/]*={0,2}$", "type": "string"},
                "encoding": {"const": "base64"},
            },
            ["encoding", "data"],
        ),
        "eof": {"type": "boolean"},
        "kind": {"const": "file"},
        "next_offset": {"minimum": 0, "type": "integer"},
        "observed_at": {"$ref": "../common.json#/$defs/timestamp"},
        "offset": {"minimum": 0, "type": "integer"},
        "path": {"$ref": "../common.json#/$defs/relative-path"},
        "returned_bytes": {"minimum": 0, "type": "integer"},
        "workspace": {"$ref": "../common.json#/$defs/workspace-id"},
    },
    ["kind", "workspace", "path", "offset", "returned_bytes", "next_offset", "eof", "content", "observed_at"],
)
DIRECTORY_PAGE = closed_object(
    {
        "items": {
            "items": closed_object(
                {
                    "kind": {"enum": ["file", "directory", "symlink"]},
                    "name": {"maxLength": 255, "minLength": 1, "type": "string"},
                    "size": {"minimum": 0, "type": ["integer", "null"]},
                },
                ["name", "kind", "size"],
            ),
            "type": "array",
        },
        "kind": {"const": "directory"},
        "next_cursor": {"type": ["string", "null"]},
        "observed_at": {"$ref": "../common.json#/$defs/timestamp"},
        "path": {"$ref": "../common.json#/$defs/relative-path"},
        "workspace": {"$ref": "../common.json#/$defs/workspace-id"},
    },
    ["kind", "workspace", "path", "items", "next_cursor", "observed_at"],
)
workspace_file_read_result = add_result("workspace-file-read", {"oneOf": [FILE_SLICE, DIRECTORY_PAGE]})

FILE_OBSERVATION = closed_object(
    {
        "atomic_replacement": {"const": True},
        "kind": {"const": "file"},
        "observed_at": {"$ref": "../common.json#/$defs/timestamp"},
        "path": {"$ref": "../common.json#/$defs/relative-path"},
        "sha256": {"pattern": "^[0-9a-f]{64}$", "type": "string"},
        "size": {"minimum": 0, "type": "integer"},
        "workspace": {"$ref": "../common.json#/$defs/workspace-id"},
    },
    ["kind", "workspace", "path", "size", "sha256", "atomic_replacement", "observed_at"],
)
workspace_file_write_result = add_result("workspace-file-write", FILE_OBSERVATION)

ABSENCE = lambda kind, id_ref: closed_object(
    {
        "absent": {"const": True},
        "id": {"$ref": id_ref},
        "kind": {"const": kind},
        "observed_at": {"$ref": "../common.json#/$defs/timestamp"},
    },
    ["kind", "id", "absent", "observed_at"],
)
workspace_file_delete_result = add_result(
    "workspace-file-delete",
    closed_object(
        {
            "absent": {"const": True},
            "kind": {"const": "file"},
            "observed_at": {"$ref": "../common.json#/$defs/timestamp"},
            "path": {"$ref": "../common.json#/$defs/relative-path"},
            "workspace": {"$ref": "../common.json#/$defs/workspace-id"},
        },
        ["kind", "workspace", "path", "absent", "observed_at"],
    ),
)
workspace_destroy_result = add_result(
    "workspace-destroy", ABSENCE("workspace", "../common.json#/$defs/workspace-id")
)
exec_start_result = add_result("exec-start", {"$ref": "../resource.json#/$defs/exec"})
exec_get_result = add_result("exec-get", {"$ref": "../resource.json#/$defs/exec"})
exec_signal_result = add_result("exec-signal", {"$ref": "../resource.json#/$defs/exec"})

OUTPUT_SLICE = closed_object(
    {
        "content": closed_object(
            {
                "data": {"pattern": "^[A-Za-z0-9+/]*={0,2}$", "type": "string"},
                "encoding": {"const": "base64"},
            },
            ["encoding", "data"],
        ),
        "eof": {"type": "boolean"},
        "exec": {"$ref": "../common.json#/$defs/exec-id"},
        "next_offset": {"minimum": 0, "type": "integer"},
        "observed_at": {"$ref": "../common.json#/$defs/timestamp"},
        "offset": {"minimum": 0, "type": "integer"},
        "returned_bytes": {"minimum": 0, "type": "integer"},
        "stream": {"enum": ["stdout", "stderr"]},
        "truncated": {"type": "boolean"},
    },
    ["exec", "stream", "offset", "returned_bytes", "next_offset", "eof", "truncated", "content", "observed_at"],
)
exec_output_result = add_result("exec-output-get", OUTPUT_SLICE)
operation_get_result = add_result("operation-get", {"$ref": "../operation.json"})


def predicate(
    fact: str,
    op: str,
    *,
    value: object | None = None,
    input_pointer: str | None = None,
    transform: str | None = None,
    when: dict[str, object] | None = None,
) -> dict[str, object]:
    result: dict[str, object] = {"fact": fact, "op": op}
    if input_pointer is not None:
        result["input_pointer"] = input_pointer
    else:
        result["value"] = value
    if transform is not None:
        result["transform"] = transform
    if when is not None:
        result["when"] = when
    return result


ROUTES = [
    ("machine.get", "GET", "/v1/machine", "observe", "read", "idempotent", [], "callable", machine_input, machine_result, "query", []),
    ("workspace.create", "POST", "/v1/workspaces", "workspaces", "write", "keyed", ["filesystem:workspace", "network:egress"], "projected", workspace_create_input, workspace_create_result, "body.input", [predicate("workspace.guarded-io", "eq", value=True), predicate("workspace.openat2-beneath", "eq", value=True), predicate("workspace.atomic-replace", "eq", value=True), predicate("workspace.git", "eq", value=True, when={"input_pointer": "/source", "not_equals": "empty"})]),
    ("workspace.get", "GET", "/v1/workspaces/{workspace_id}", "workspaces", "read", "idempotent", [], "callable", workspace_get_input, workspace_get_result, "query", []),
    ("workspace.file.read", "GET", "/v1/workspaces/{workspace_id}/files/{path}", "workspaces", "read", "idempotent", [], "projected", workspace_file_read_input, workspace_file_read_result, "query", [predicate("workspace.guarded-io", "eq", value=True), predicate("workspace.read-limit-bytes", "gte", input_pointer="/limit_bytes", when={"input_pointer": "/mode", "equals": "file"}), predicate("workspace.list-limit-items", "gte", input_pointer="/limit_items", when={"input_pointer": "/mode", "equals": "directory"})]),
    ("workspace.file.write", "PUT", "/v1/workspaces/{workspace_id}/files/{path}", "workspaces", "write", "keyed", ["filesystem:workspace"], "projected", workspace_file_write_input, workspace_file_write_result, "body.input", [predicate("workspace.guarded-io", "eq", value=True), predicate("workspace.atomic-replace", "eq", value=True), predicate("workspace.max-file-bytes", "gte", input_pointer="/content/data", transform="base64-decoded-length")]),
    ("workspace.file.delete", "DELETE", "/v1/workspaces/{workspace_id}/files/{path}", "workspaces", "destructive", "keyed", ["filesystem:workspace"], "projected", workspace_file_delete_input, workspace_file_delete_result, "body.input", [predicate("workspace.guarded-io", "eq", value=True)]),
    ("workspace.destroy", "DELETE", "/v1/workspaces/{workspace_id}", "workspaces", "destructive", "keyed", ["filesystem:workspace"], "projected", workspace_destroy_input, workspace_destroy_result, "body.input", [predicate("workspace.guarded-io", "eq", value=True)]),
    ("exec.start", "POST", "/v1/execs", "exec", "write", "keyed", ["process", "filesystem:workspace", "network:egress"], "projected", exec_start_input, exec_start_result, "body.input", [predicate("exec.argv-only", "eq", value=True), predicate("exec.namespaces", "eq", value={"user": True, "mount": True, "pid": True, "ipc": True, "uts": True, "network": True}), predicate("exec.cgroup-limits", "eq", value={"processes": True, "memory": True, "cpu": True}), predicate("exec.cgroup-kill", "eq", value=True), predicate("exec.output-limit-bytes", "gte", input_pointer="/limits/output_bytes"), predicate("exec.no-egress", "eq", value=True, when={"input_pointer": "/sandbox/network", "equals": "none"}), predicate("exec.network-aperture", "eq", value=True, when={"input_pointer": "/sandbox/network", "equals": "aperture"})]),
    ("exec.get", "GET", "/v1/execs/{exec_id}", "exec", "read", "idempotent", [], "callable", exec_get_input, exec_get_result, "query", []),
    ("exec.output.get", "GET", "/v1/execs/{exec_id}/output", "exec", "read", "idempotent", [], "projected", exec_output_input, exec_output_result, "query", [predicate("exec.output-limit-bytes", "gte", input_pointer="/limit_bytes")]),
    ("exec.signal", "POST", "/v1/execs/{exec_id}/signal", "exec", "destructive", "keyed", ["process"], "projected", exec_signal_input, exec_signal_result, "body.input", [predicate("exec.cgroup-kill", "eq", value=True), predicate("exec.signals", "one_of", input_pointer="/signal")]),
    ("operation.get", "GET", "/v1/ops/{operation_id}", "observe", "read", "idempotent", [], "callable", operation_get_input, operation_get_result, "query", []),
]


OPERATION_REGISTRY = {
    "$schema": "schemas/operation-registry.json",
    "api_major": 1,
    "operations": [
        {
            "address_schema": ADDRESS_BY_OPERATION[operation_id],
            "capability_predicates": predicates,
            "direction": "outbound",
            "effects": effects,
            "exposure": exposure,
            "id": operation_id,
            "idempotency": idempotency,
            "input_binding": input_binding,
            "input_schema": input_schema,
            "method": method,
            "path": path,
            "required_scope": scope,
            "result_schema": result_schema,
            "risk": risk,
        }
        for operation_id, method, path, scope, risk, idempotency, effects, exposure, input_schema, result_schema, input_binding, predicates in ROUTES
    ],
    "registry_format": "daemonloom.substrate-operation-registry.v1",
}


PREDICATE_VALUE = {}
PREDICATE = closed_object(
    {
        "fact": {"pattern": "^[a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)+$", "type": "string"},
        "input_pointer": {"pattern": "^/", "type": "string"},
        "op": {"enum": ["eq", "one_of", "lte", "gte"]},
        "transform": {"enum": ["base64-decoded-length"]},
        "value": {},
        "when": closed_object(
            {
                "equals": {},
                "input_pointer": {"pattern": "^/", "type": "string"},
                "not_equals": {},
            },
            ["input_pointer"],
            oneOf=[{"required": ["equals"]}, {"required": ["not_equals"]}],
        ),
    },
    ["fact", "op"],
    oneOf=[{"required": ["value"]}, {"required": ["input_pointer"]}],
)

OPERATION_REGISTRY_SCHEMA = schema(
    "Phase-2 operation registry",
    schema_id="operation-registry",
    **closed_object(
        {
            "$schema": {"const": "schemas/operation-registry.json"},
            "api_major": {"const": 1},
            "operations": {
                "items": closed_object(
                    {
                        "capability_predicates": {"items": PREDICATE, "type": "array"},
                        "address_schema": {"pattern": "^schemas/addresses/[a-z0-9-]+\\.json$", "type": "string"},
                        "direction": {"const": "outbound"},
                        "effects": {
                            "items": {"enum": ["process", "filesystem:workspace", "filesystem:volume", "network:egress", "network:expose", "image", "workload"]},
                            "type": "array",
                            "uniqueItems": True,
                        },
                        "exposure": {"enum": ["callable", "callable-direct", "projected"]},
                        "id": {"pattern": "^[a-z][a-z0-9-]*(\\.[a-z][a-z0-9-]*)+$", "type": "string"},
                        "idempotency": {"enum": ["idempotent", "keyed", "none"]},
                        "input_binding": {"enum": ["query", "body.input"]},
                        "input_schema": {"pattern": "^schemas/inputs/[a-z0-9-]+\\.json$", "type": "string"},
                        "method": {"enum": ["GET", "POST", "PUT", "DELETE"]},
                        "path": {"pattern": "^/v1/", "type": "string"},
                        "required_scope": {"enum": ["observe", "workspaces", "exec"]},
                        "result_schema": {"pattern": "^schemas/results/[a-z0-9-]+\\.json$", "type": "string"},
                        "risk": {"enum": ["read", "write", "destructive"]},
                    },
                    ["id", "method", "path", "direction", "risk", "idempotency", "effects", "exposure", "required_scope", "input_binding", "address_schema", "input_schema", "result_schema", "capability_predicates"],
                ),
                "maxItems": 12,
                "minItems": 12,
                "type": "array",
            },
            "registry_format": {"const": "daemonloom.substrate-operation-registry.v1"},
        },
        ["$schema", "registry_format", "api_major", "operations"],
    ),
)


def jcs(value: object) -> bytes:
    """RFC 8785 for the deliberately integer-only phase-2 input domain."""

    if value is None:
        return b"null"
    if value is True:
        return b"true"
    if value is False:
        return b"false"
    if isinstance(value, int):
        return str(value).encode("ascii")
    if isinstance(value, float):
        raise TypeError("phase-2 canonical inputs forbid floating point numbers")
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if isinstance(value, list):
        return b"[" + b",".join(jcs(item) for item in value) + b"]"
    if isinstance(value, dict):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("JCS object keys must be strings")
        keys = sorted(value, key=lambda key: key.encode("utf-16be"))
        return b"{" + b",".join(jcs(key) + b":" + jcs(value[key]) for key in keys) + b"}"
    raise TypeError(f"unsupported canonical JSON value {type(value).__name__}")


def hash_case(
    case_id: str,
    method: str,
    normalized_address: str,
    operation_input: dict[str, object],
    operation_id: str,
    subject: str,
    deployment: str,
    relation: dict[str, str] | None = None,
) -> dict[str, object]:
    fields = [b"1", method.encode("ascii"), normalized_address.encode("utf-8"), jcs(operation_input)]
    tuple_bytes = b"".join(struct.pack(">I", len(field)) + field for field in fields)
    result: dict[str, object] = {
        "api_major": "1",
        "excluded": {
            "authorization": f"Bearer sbt_authorization_{case_id}",
            "bearer": f"sbt_raw_{case_id}",
            "deployment": deployment,
            "headers": {"traceparent": f"00-{case_id}"},
            "operation": operation_id,
            "principal": f"principal:{case_id}",
            "request_id": f"req_{case_id.replace('-', '_')}",
            "subject": subject,
        },
        "id": case_id,
        "input": operation_input,
        "jcs_input_hex": jcs(operation_input).hex(),
        "ledger_key": {
            "deployment": deployment,
            "operation": operation_id,
            "subject": subject,
        },
        "method": method,
        "normalized_address": normalized_address,
        "sha256": hashlib.sha256(tuple_bytes).hexdigest(),
        "tuple_hex": tuple_bytes.hex(),
    }
    if relation is not None:
        result["relation"] = relation
    return result


HASH_CASES = [
    hash_case(
        "workspace-create-base",
        "POST",
        "/v1/workspaces",
        {"source": "empty", "labels": {"z": "last", "a": "first"}},
        "01JHASHBASE00000000001",
        "local:1000",
        "dep_a",
    ),
    hash_case(
        "workspace-create-transport-variant",
        "POST",
        "/v1/workspaces",
        {"labels": {"a": "first", "z": "last"}, "source": "empty"},
        "01JHASHOTHER0000000002",
        "local:2000",
        "dep_b",
        {"case": "workspace-create-base", "kind": "same-request-hash-different-ledger-key"},
    ),
    hash_case(
        "workspace-create-replay",
        "POST",
        "/v1/workspaces",
        {"labels": {"a": "first", "z": "last"}, "source": "empty"},
        "01JHASHBASE00000000001",
        "local:1000",
        "dep_a",
        {"case": "workspace-create-base", "kind": "same-request-hash-same-ledger-key"},
    ),
    hash_case(
        "workspace-create-conflict",
        "POST",
        "/v1/workspaces",
        {"labels": {"a": "changed", "z": "last"}, "source": "empty"},
        "01JHASHBASE00000000001",
        "local:1000",
        "dep_a",
        {"case": "workspace-create-base", "kind": "different-request-hash-same-ledger-key"},
    ),
    hash_case(
        "workspace-file-address",
        "PUT",
        "/v1/workspaces/ws_vector/files/src/main.txt",
        {"content": {"encoding": "base64", "data": "aGVsbG8="}},
        "01JHASHFILE000000000003",
        "local:1000",
        "dep_a",
    ),
]

HASH_FIXTURES = {
    "$schema": "../schemas/hash-fixtures.json",
    "algorithm": "sha256",
    "cases": HASH_CASES,
    "format": "daemonloom.substrate-canonical-hash-fixtures.v1",
}

HASH_SCHEMA = schema(
    "Canonical request-hash fixtures",
    schema_id="hash-fixtures",
    **closed_object(
        {
            "$schema": {"const": "../schemas/hash-fixtures.json"},
            "algorithm": {"const": "sha256"},
            "cases": {
                "items": closed_object(
                    {
                        "api_major": {"const": "1"},
                        "excluded": closed_object(
                            {
                                "authorization": {"type": "string"},
                                "bearer": {"type": "string"},
                                "deployment": {"type": "string"},
                                "headers": {"type": "object"},
                                "operation": {"$ref": "common.json#/$defs/operation-id"},
                                "principal": {"type": "string"},
                                "request_id": {"$ref": "common.json#/$defs/request-id"},
                                "subject": {"type": "string"},
                            },
                            ["operation", "request_id", "authorization", "bearer", "headers", "subject", "deployment", "principal"],
                        ),
                        "id": {"pattern": "^[a-z][a-z0-9-]+$", "type": "string"},
                        "input": {"type": "object"},
                        "jcs_input_hex": {"pattern": "^(?:[0-9a-f]{2})*$", "type": "string"},
                        "ledger_key": closed_object(
                            {"deployment": {"type": "string"}, "operation": {"$ref": "common.json#/$defs/operation-id"}, "subject": {"type": "string"}},
                            ["deployment", "subject", "operation"],
                        ),
                        "method": {"enum": ["POST", "PUT", "DELETE"]},
                        "normalized_address": {"pattern": "^/v1/", "type": "string"},
                        "relation": closed_object(
                            {"case": {"type": "string"}, "kind": {"enum": ["same-request-hash-different-ledger-key", "same-request-hash-same-ledger-key", "different-request-hash-same-ledger-key"]}},
                            ["kind", "case"],
                        ),
                        "sha256": {"pattern": "^[0-9a-f]{64}$", "type": "string"},
                        "tuple_hex": {"pattern": "^(?:[0-9a-f]{2})+$", "type": "string"},
                    },
                    ["id", "api_major", "method", "normalized_address", "input", "jcs_input_hex", "tuple_hex", "sha256", "excluded", "ledger_key"],
                ),
                "minItems": 5,
                "type": "array",
            },
            "format": {"const": "daemonloom.substrate-canonical-hash-fixtures.v1"},
        },
        ["$schema", "format", "algorithm", "cases"],
    ),
)

HASHING = {
    "address_normalization": {
        "dot_segments": "reject",
        "encoded_separator": "reject",
        "path_parameters": "validate-and-substitute-into-registry-template",
        "percent_encoding": "decode-once-as-utf8-then-encode-non-unreserved-with-uppercase-hex",
        "query": "excluded; typed query values are operation input",
        "repeated_separator": "reject",
        "trailing_separator": "reject-except-root",
    },
    "algorithm": "sha256",
    "canonical_input": "RFC 8785 JCS; phase-2 schemas contain no floating-point numbers",
    "excluded": ["operation", "request_id", "headers", "authorization", "bearer", "subject", "principal", "deployment"],
    "fixtures": "fixtures/canonical-hash.json",
    "format": "daemonloom.substrate-request-hash.v1",
    "ledger_key": ["deployment", "subject", "operation"],
    "tuple": {
        "encoding": "concatenated-u32be-length-prefixed-fields",
        "fields": ["api-major-as-ascii-decimal", "uppercase-http-method", "normalized-address", "rfc8785-operation-input"],
        "length_unit": "octets",
    },
}


def success_body(request_id: str, result: object, operation: str | None = None) -> dict[str, object]:
    body: dict[str, object] = {"api_version": "v1", "request_id": request_id, "result": result}
    if operation is not None:
        body["operation"] = operation
    return body


def error_body(
    request_id: str,
    error_class: str,
    code: str,
    message: str,
    *,
    operation: str | None = None,
    address: str | None = None,
    retriable: bool = False,
) -> dict[str, object]:
    detail: dict[str, object] = {"class": error_class, "code": code, "message": message, "retriable": retriable}
    if operation is not None:
        detail["operation"] = operation
    if address is not None:
        detail["address"] = address
    return {"api_version": "v1", "request_id": request_id, "error": detail}


def requested(network: str = "none") -> dict[str, object]:
    return {"capability_snapshot": CAPABILITY_SNAPSHOT, "network": network, "profile": "workspace", "require": True}


def applied() -> dict[str, object]:
    return {"capability_snapshot": CAPABILITY_SNAPSHOT, "cgroup": "substrate/vector", "filesystem": "workspace-rw-system-ro", "network": "none", "profile": "workspace"}


def workspace(workspace_id: str = "ws_vector", state: str = "ready", labels: dict[str, str] | None = None) -> dict[str, object]:
    return {"id": workspace_id, "kind": "workspace", "labels": labels or {}, "observed_at": FIXED_TIME, "state": state}


def exec_resource(exec_id: str = "ex_vector", state: str = "running", *, exit_value: object = None, applied_value: object = ...) -> dict[str, object]:
    if applied_value is ...:
        applied_value = None if state == "accepted" else applied()
    return {"applied": applied_value, "exit": exit_value, "id": exec_id, "kind": "exec", "observed_at": FIXED_TIME, "requested": requested(), "state": state, "workspace": "ws_vector"}


def http_context(subject: str = SUBJECT, *, authenticated: bool = True, reachable: bool = False) -> dict[str, object]:
    return {
        "actor": "vector-client",
        "authority": "http-harness",
        "clock": FIXED_TIME,
        "deployment": DEPLOYMENT,
        "subject": subject,
        "transport": {"authenticated": authenticated, "kind": "tcp" if reachable else "unix", "reachable": reachable},
    }


def base_postconditions() -> list[dict[str, object]]:
    return [{"actual": "/fixture_identity_from_request", "expected": False, "operator": "equals"}]


def http_vector(
    vector_id: str,
    covers: list[str],
    operation: str,
    request: dict[str, object],
    status: int,
    body: dict[str, object],
    *,
    setup: list[dict[str, object]] | None = None,
    postconditions: list[dict[str, object]] | None = None,
    valid_address: bool = True,
    valid_input: bool = True,
    context: dict[str, object] | None = None,
) -> dict[str, object]:
    return {
        "$schema": "../../schemas/vector.json",
        "action": {
            "kind": "http",
            "operation": operation,
            "request": request,
            "valid_address": valid_address,
            "valid_input": valid_input,
        },
        "context": context or http_context(),
        "covers": covers,
        "expected": {"kind": "http-response", "response": {"body": body, "status": status}},
        "id": vector_id,
        "layer": "http",
        "phase": 2,
        "postconditions": base_postconditions() + (postconditions or []),
        "setup": setup or [],
    }


def driver_vector(
    vector_id: str,
    covers: list[str],
    port: str,
    command: dict[str, object],
    outcome: dict[str, object],
    *,
    setup: list[dict[str, object]] | None = None,
    postconditions: list[dict[str, object]] | None = None,
    phase: int = 2,
) -> dict[str, object]:
    return {
        "$schema": "../../schemas/vector.json",
        "action": {"command": command, "kind": "driver", "port": port},
        "context": {"authority": "driver-harness", "clock": FIXED_TIME, "deployment": DEPLOYMENT, "subject": SUBJECT},
        "covers": covers,
        "expected": {"kind": "driver-outcome", "outcome": outcome},
        "id": vector_id,
        "layer": "driver",
        "phase": phase,
        "postconditions": base_postconditions() + (postconditions or []),
        "setup": setup or [],
    }


def fixture(kind: str, name: str, state: object) -> dict[str, object]:
    return {"kind": kind, "name": name, "state": state}


def condition(actual: str, operator: str, expected: object) -> dict[str, object]:
    return {"actual": actual, "expected": expected, "operator": operator}


MACHINE = {
    "config_generation": 7,
    "driver": "host",
    "driver_version": "fixture-1",
    "facts": {
        "exec.argv-only": True,
        "exec.cgroup-kill": True,
        "exec.cgroup-limits": {"cpu": True, "memory": True, "processes": True},
        "exec.namespaces": {"ipc": True, "mount": True, "network": True, "pid": True, "user": True, "uts": True},
        "exec.no-egress": True,
        "exec.output-limit-bytes": 65536,
        "exec.signals": ["INT", "TERM", "KILL"],
        "workspace.atomic-replace": True,
        "workspace.guarded-io": True,
        "workspace.list-limit-items": 100,
        "workspace.max-file-bytes": 1048576,
        "workspace.openat2-beneath": True,
        "workspace.read-limit-bytes": 65536,
    },
    "probed_at": FIXED_TIME,
    "snapshot": CAPABILITY_SNAPSHOT,
}


def mutation_request(method: str, path: str, operation: str, operation_input: dict[str, object], request_id: str) -> dict[str, object]:
    return {"body": {"input": operation_input, "op": operation}, "headers": {"x-request-id": request_id}, "method": method, "path": path, "query": {}}


def read_request(method: str, path: str, query: dict[str, object], request_id: str) -> dict[str, object]:
    return {"headers": {"x-request-id": request_id}, "method": method, "path": path, "query": query}


VECTORS: dict[str, dict[str, object]] = {}


def add_vector(directory: str, filename: str, value: dict[str, object]) -> None:
    VECTORS[f"vectors/{directory}/{filename}.json"] = value


# Exact positive route fixtures.
add_vector("http", "machine-probe", http_vector(
    "machine-probe-is-observed", ["route.machine.get"], "machine.get",
    read_request("GET", "/v1/machine", {}, "req_machine_0001"), 200,
    success_body("req_machine_0001", MACHINE),
    postconditions=[condition("/probes/advertised_unprobed_fact_count", "equals", 0)],
))

create_op = "01JPHASE2WORKSPACECREATE"
add_vector("http", "workspace-create", http_vector(
    "empty-workspace-create-is-keyed", ["route.workspace.create"], "workspace.create",
    mutation_request("POST", "/v1/workspaces", create_op, {"labels": {"vector": "workspace-create"}, "source": "empty"}, "req_workspace_create"), 201,
    success_body("req_workspace_create", workspace("ws_created", labels={"vector": "workspace-create"}), create_op),
    setup=[fixture("machine", "minimum-host", MACHINE)],
    postconditions=[condition("/driver/dispatch_count", "equals", 1), condition("/replay/logical_resource", "equals", "ws_created")],
))

replay_request_one = mutation_request(
    "POST",
    "/v1/workspaces",
    create_op,
    {"labels": {"vector": "workspace-replay"}, "source": "empty"},
    "req_workspace_replay1",
)
replay_request_two = {
    **replay_request_one,
    "headers": {"traceparent": "00-second-attempt", "x-request-id": "req_workspace_replay2"},
}
replayed_workspace = workspace("ws_replayed", labels={"vector": "workspace-replay"})
add_vector("http", "workspace-replay", {
    "$schema": "../../schemas/vector.json",
    "action": {
        "kind": "http-sequence",
        "operation": "workspace.create",
        "steps": [
            {"request": replay_request_one, "valid_input": True},
            {"request": replay_request_two, "valid_input": True},
        ],
    },
    "context": http_context(),
    "covers": ["lifecycle.stable-replay"],
    "expected": {
        "kind": "http-sequence",
        "responses": [
            {"body": success_body("req_workspace_replay1", replayed_workspace, create_op), "status": 201},
            {"body": success_body("req_workspace_replay2", replayed_workspace, create_op), "status": 201},
        ],
    },
    "id": "same-operation-same-input-replays-original-outcome",
    "layer": "http",
    "phase": 2,
    "postconditions": base_postconditions()
    + [condition("/driver/dispatch_count", "equals", 1), condition("/probes/distinct_workspace_count", "equals", 1)],
    "setup": [fixture("machine", "minimum-host", MACHINE)],
})

add_vector("http", "workspace-get", http_vector(
    "owned-workspace-is-observed", ["route.workspace.get"], "workspace.get",
    read_request("GET", "/v1/workspaces/ws_vector", {}, "req_workspace_get"), 200,
    success_body("req_workspace_get", workspace()),
    setup=[fixture("workspace", "owned", {"deployment": DEPLOYMENT, "owner": SUBJECT, "resource": workspace()})],
))

file_result = {"content": {"data": "aGVsbG8=", "encoding": "base64"}, "eof": True, "kind": "file", "next_offset": 5, "observed_at": FIXED_TIME, "offset": 0, "path": "src/main.txt", "returned_bytes": 5, "workspace": "ws_vector"}
add_vector("http", "file-read", http_vector(
    "bounded-file-read-is-observed", ["route.workspace.file.read", "security.bounds.read"], "workspace.file.read",
    read_request("GET", "/v1/workspaces/ws_vector/files/src/main.txt", {"limit_bytes": 16, "mode": "file", "offset": 0}, "req_file_read_0001"), 200,
    success_body("req_file_read_0001", file_result),
    setup=[fixture("workspace", "owned-with-file", {"files": {"src/main.txt": "aGVsbG8="}, "owner": SUBJECT})],
    postconditions=[condition("/probes/read_bytes", "equals", 5)],
))

dir_result = {"items": [{"kind": "file", "name": "main.txt", "size": 5}], "kind": "directory", "next_cursor": None, "observed_at": FIXED_TIME, "path": "src", "workspace": "ws_vector"}
add_vector("http", "directory-list", http_vector(
    "bounded-directory-list-is-observed", ["security.bounds.list"], "workspace.file.read",
    read_request("GET", "/v1/workspaces/ws_vector/files/src", {"cursor": None, "limit_items": 10, "mode": "directory"}, "req_dir_list_0001"), 200,
    success_body("req_dir_list_0001", dir_result),
    setup=[fixture("workspace", "owned-with-file", {"files": {"src/main.txt": "aGVsbG8="}, "owner": SUBJECT})],
    postconditions=[condition("/probes/listed_items", "equals", 1)],
))

write_op = "01JPHASE2FILEWRITE00001"
write_result = {"atomic_replacement": True, "kind": "file", "observed_at": FIXED_TIME, "path": "src/main.txt", "sha256": hashlib.sha256(b"hello").hexdigest(), "size": 5, "workspace": "ws_vector"}
add_vector("http", "file-write", http_vector(
    "bounded-file-replacement-is-atomic", ["route.workspace.file.write", "security.bounds.write", "security.atomic-replacement", "lifecycle.post-action-observation"], "workspace.file.write",
    mutation_request("PUT", "/v1/workspaces/ws_vector/files/src/main.txt", write_op, {"content": {"data": "aGVsbG8=", "encoding": "base64"}}, "req_file_write_001"), 200,
    success_body("req_file_write_001", write_result, write_op),
    setup=[fixture("workspace", "owned-with-old-file", {"files": {"src/main.txt": "b2xk"}, "owner": SUBJECT})],
    postconditions=[condition("/probes/partial_target_visible", "equals", False), condition("/probes/reobserved_sha256", "equals", hashlib.sha256(b"hello").hexdigest())],
))

delete_file_op = "01JPHASE2FILEDELETE0001"
delete_file_result = {"absent": True, "kind": "file", "observed_at": FIXED_TIME, "path": "src/main.txt", "workspace": "ws_vector"}
add_vector("http", "file-delete", http_vector(
    "bounded-file-delete-observes-absence", ["route.workspace.file.delete", "security.bounds.delete"], "workspace.file.delete",
    mutation_request("DELETE", "/v1/workspaces/ws_vector/files/src/main.txt", delete_file_op, {}, "req_file_delete01"), 200,
    success_body("req_file_delete01", delete_file_result, delete_file_op),
    setup=[fixture("workspace", "owned-with-file", {"files": {"src/main.txt": "aGVsbG8="}, "owner": SUBJECT})],
    postconditions=[condition("/probes/target_exists", "equals", False), condition("/probes/deleted_entries", "equals", 1)],
))

deep_delete_path = "/v1/workspaces/ws_vector/files/" + "/".join(["a"] * 65)
add_vector("http", "file-delete-depth", http_vector(
    "delete-over-path-depth-is-refused", ["security.bounds.delete"], "workspace.file.delete",
    mutation_request("DELETE", deep_delete_path, "01JPHASE2DELETEDEPTH001", {}, "req_delete_depth1"), 422,
    error_body("req_delete_depth1", "refused", "workspace.path-depth", "Workspace path exceeds the configured component limit.", operation="01JPHASE2DELETEDEPTH001", address="path"),
    setup=[fixture("workspace", "owned", {"owner": SUBJECT})], valid_address=False,
    postconditions=[condition("/probes/deleted_entries", "equals", 0)],
))

destroy_op = "01JPHASE2DESTROY0000001"
destroy_result = {"absent": True, "id": "ws_vector", "kind": "workspace", "observed_at": FIXED_TIME}
add_vector("http", "workspace-destroy", http_vector(
    "workspace-destroy-observes-absence", ["route.workspace.destroy"], "workspace.destroy",
    mutation_request("DELETE", "/v1/workspaces/ws_vector", destroy_op, {}, "req_ws_destroy001"), 200,
    success_body("req_ws_destroy001", destroy_result, destroy_op),
    setup=[fixture("workspace", "owned-terminal", {"execs_terminal": True, "owner": SUBJECT})],
    postconditions=[condition("/probes/workspace_root_exists", "equals", False)],
))

start_op = "01JPHASE2EXECSTART000001"
start_input = {"argv": ["/usr/bin/true"], "env": {"allow": [], "set": {}}, "limits": {"cpu_millis": 1000, "memory_bytes": 67108864, "output_bytes": 65536, "processes": 16, "timeout_ms": 5000}, "sandbox": requested(), "wait": False, "workspace": "ws_vector"}
add_vector("http", "exec-start", http_vector(
    "bounded-exec-start-records-requested-and-applied", ["route.exec.start"], "exec.start",
    mutation_request("POST", "/v1/execs", start_op, start_input, "req_exec_start001"), 202,
    success_body("req_exec_start001", exec_resource(), start_op),
    setup=[fixture("machine", "minimum-host", MACHINE), fixture("workspace", "owned", {"owner": SUBJECT})],
    postconditions=[condition("/probes/child_environment_keys", "equals", []), condition("/probes/applied_network", "equals", "none")],
))

add_vector("http", "exec-get", http_vector(
    "owned-exec-is-reobserved", ["route.exec.get", "security.post-action-observation"], "exec.get",
    read_request("GET", "/v1/execs/ex_vector", {}, "req_exec_get_0001"), 200,
    success_body("req_exec_get_0001", exec_resource()),
    setup=[fixture("exec", "owned-running", {"owner": SUBJECT, "resource": exec_resource()})],
    postconditions=[condition("/probes/driver_observe_count", "equals", 1)],
))

output_result = {"content": {"data": "b2sK", "encoding": "base64"}, "eof": True, "exec": "ex_vector", "next_offset": 3, "observed_at": FIXED_TIME, "offset": 0, "returned_bytes": 3, "stream": "stdout", "truncated": False}
add_vector("http", "exec-output", http_vector(
    "bounded-exec-output-is-ranged", ["route.exec.output.get"], "exec.output.get",
    read_request("GET", "/v1/execs/ex_vector/output", {"limit_bytes": 16, "offset": 0, "stream": "stdout"}, "req_exec_output01"), 200,
    success_body("req_exec_output01", output_result),
    setup=[fixture("exec-output", "owned-output", {"owner": SUBJECT, "stderr": "", "stdout": "b2sK"})],
    postconditions=[condition("/probes/returned_bytes", "equals", 3)],
))

signal_op = "01JPHASE2EXECSIGNAL00001"
cancelled_exec = exec_resource(state="cancelled", exit_value={"code": None, "signal": "TERM"})
add_vector("http", "exec-signal", http_vector(
    "exec-signal-observes-cgroup-empty", ["route.exec.signal"], "exec.signal",
    mutation_request("POST", "/v1/execs/ex_vector/signal", signal_op, {"grace_ms": 1000, "signal": "TERM"}, "req_exec_signal01"), 200,
    success_body("req_exec_signal01", cancelled_exec, signal_op),
    setup=[fixture("exec", "owned-running", {"owner": SUBJECT, "resource": exec_resource()})],
    postconditions=[condition("/probes/cgroup_process_count", "equals", 0)],
))

terminal_operation = {
    "accepted_at": "2026-08-13T11:59:59Z",
    "actor": "vector-client",
    "capability_snapshot": CAPABILITY_SNAPSHOT,
    "operation": create_op,
    "operation_kind": "workspace.create",
    "outcome": {"kind": "success", "result": workspace("ws_created", labels={"vector": "workspace-create"})},
    "principal": None,
    "request_hash": HASH_CASES[0]["sha256"],
    "resource": "ws_created",
    "state": "terminal",
    "terminal_at": FIXED_TIME,
}
add_vector("http", "operation-get", http_vector(
    "operation-ledger-reconciles-terminal-answer", ["route.operation.get", "lifecycle.lost-answer-reconciliation"], "operation.get",
    read_request("GET", f"/v1/ops/{create_op}", {}, "req_operation_get"), 200,
    success_body("req_operation_get", terminal_operation),
    setup=[fixture("operation", "terminal", {"deployment": DEPLOYMENT, "owner": SUBJECT, "record": terminal_operation})],
))

# Refusal, isolation, bounds, and lifecycle fixtures.
add_vector("http", "path-escape", http_vector(
    "lexical-path-escape-is-refused", ["security.path.lexical", "error.refused"], "workspace.file.read",
    read_request("GET", "/v1/workspaces/ws_vector/files/%2e%2e%2fetc%2fpasswd", {"limit_bytes": 16, "mode": "file", "offset": 0}, "req_path_escape01"), 422,
    error_body("req_path_escape01", "refused", "workspace.path-escape", "Workspace path is outside the confined root.", address="path"),
    setup=[fixture("workspace", "owned", {"owner": SUBJECT})], valid_address=False, valid_input=True,
    postconditions=[condition("/probes/outside_access_count", "equals", 0)],
))

for filename, vector_id, coverage, shape, path in [
    ("absolute-path", "absolute-path-is-refused", "security.path.absolute", "absolute", "/etc/passwd"),
    ("symlink-escape", "symlink-escape-is-refused", "security.path.symlink", "symlink", "link/secret"),
    ("dangling-link", "dangling-link-is-refused", "security.path.dangling-link", "dangling-link", "dangling/secret"),
    ("magic-link", "magic-link-is-refused", "security.path.magic-link", "magic-link", "proc/self/fd/9"),
    ("mount-escape", "mount-escape-is-refused", "security.path.mount", "mount", "mounted/secret"),
]:
    add_vector("driver", filename, driver_vector(
        vector_id, [coverage], "workspace", {"operation": "workspace.read-file", "path": path, "workspace": "ws_vector"},
        {"code": "workspace.path-escape", "status": "refused"},
        setup=[fixture("path-shape", shape, {"outside_sha256": "1" * 64, "path": path})],
        postconditions=[condition("/probes/outside_sha256", "equals", "1" * 64), condition("/probes/outside_access_count", "equals", 0)],
    ))

add_vector("http", "read-limit", http_vector(
    "read-over-cap-is-exhausted", ["error.exhausted"], "workspace.file.read",
    read_request("GET", "/v1/workspaces/ws_vector/files/large.bin", {"limit_bytes": 1048576, "mode": "file", "offset": 0}, "req_read_limit001"), 429,
    error_body("req_read_limit001", "exhausted", "workspace.read-limit", "Requested read exceeds the probed limit.", address="limit", retriable=True),
    setup=[fixture("machine", "lower-read-limit", {**MACHINE, "facts": {**MACHINE["facts"], "workspace.read-limit-bytes": 65536}})],
    postconditions=[condition("/probes/read_bytes", "equals", 0)],
))

large_write_op = "01JPHASE2WRITEEXHAUST01"
add_vector("http", "write-limit", http_vector(
    "write-over-cap-is-exhausted", [], "workspace.file.write",
    mutation_request("PUT", "/v1/workspaces/ws_vector/files/large.bin", large_write_op, {"content": {"data": "aGVsbG8=", "encoding": "base64"}}, "req_write_limit01"), 429,
    error_body("req_write_limit01", "exhausted", "workspace.write-limit", "Requested replacement exceeds the probed limit.", operation=large_write_op, address="limit", retriable=True),
    setup=[fixture("machine", "lower-file-limit", {**MACHINE, "facts": {**MACHINE["facts"], "workspace.max-file-bytes": 4}})],
    postconditions=[condition("/probes/target_sha256", "equals", "2" * 64)],
))

add_vector("http", "strict-request", http_vector(
    "unknown-request-field-is-refused", ["schema.strict-request"], "workspace.create",
    {**mutation_request("POST", "/v1/workspaces", "01JPHASE2STRICTREQUEST1", {"labels": {}, "source": "empty", "secret": "must-not-exist"}, "req_strict_00001")}, 422,
    error_body("req_strict_00001", "refused", "request.schema-invalid", "Request does not match the closed operation schema.", operation="01JPHASE2STRICTREQUEST1", address="input"),
    valid_input=False, postconditions=[condition("/driver/dispatch_count", "equals", 0)],
))

add_vector("http", "replay-conflict", http_vector(
    "same-operation-different-input-conflicts", ["error.conflict", "lifecycle.different-input-conflict"], "workspace.create",
    mutation_request("POST", "/v1/workspaces", "01JPHASE2REPLAYCONFLICT", {"labels": {"different": "body"}, "source": "empty"}, "req_replay_conf01"), 409,
    error_body("req_replay_conf01", "conflict", "operation.request-conflict", "Operation id is already bound to different input.", operation="01JPHASE2REPLAYCONFLICT", address="operation"),
    setup=[fixture("operation", "different-hash", {"operation": "01JPHASE2REPLAYCONFLICT", "request_hash": "3" * 64})],
    postconditions=[condition("/driver/dispatch_count", "equals", 0), condition("/probes/original_record_changed", "equals", False)],
))

egress_input = {**start_input, "sandbox": requested("aperture")}
add_vector("http", "egress-unserved", http_vector(
    "phase-two-egress-is-unserved", ["error.unserved", "security.no-egress"], "exec.start",
    mutation_request("POST", "/v1/execs", "01JPHASE2EGRESSUNSERVED", egress_input, "req_egress_0001"), 501,
    error_body("req_egress_0001", "unserved", "exec.network-unserved", "Requested network aperture is not served by this host.", operation="01JPHASE2EGRESSUNSERVED", address="exec.network-aperture"),
    setup=[fixture("machine", "minimum-host", MACHINE)],
    postconditions=[condition("/driver/dispatch_count", "equals", 0), condition("/probes/network_was_weakened", "equals", False)],
))

add_vector("http", "sandbox-unavailable", http_vector(
    "missing-sandbox-never-falls-back", ["security.sandbox-unavailable"], "exec.start",
    mutation_request("POST", "/v1/execs", "01JPHASE2SANDBOXABSENT", start_input, "req_sandbox_abs1"), 501,
    error_body("req_sandbox_abs1", "unserved", "exec.sandbox-unavailable", "Required host confinement is not available.", operation="01JPHASE2SANDBOXABSENT", address="exec.namespaces"),
    setup=[fixture("machine", "sandbox-absent", {**MACHINE, "facts": {key: value for key, value in MACHINE["facts"].items() if key not in {"exec.namespaces", "exec.cgroup-limits", "exec.cgroup-kill"}}})],
    postconditions=[condition("/driver/dispatch_count", "equals", 0), condition("/probes/unconfined_spawn_count", "equals", 0)],
))

add_vector("http", "cross-subject-resource", http_vector(
    "cross-subject-resource-is-not-found", ["security.subject-resource-isolation"], "workspace.get",
    read_request("GET", "/v1/workspaces/ws_other", {}, "req_cross_resource"), 404,
    error_body("req_cross_resource", "refused", "resource.not-found", "Resource was not found.", address="resource"),
    setup=[fixture("workspace", "other-subject", {"owner": "local:1001", "resource": workspace("ws_other")})],
    postconditions=[condition("/probes/disclosed_owner_fields", "equals", 0)],
))

add_vector("http", "cross-subject-not-found", http_vector(
    "cross-subject-operation-is-not-found", ["lifecycle.subject-operation-isolation"], "operation.get",
    read_request("GET", "/v1/ops/01JPHASE2OTHERSUBJECT01", {}, "req_cross_operation"), 404,
    error_body("req_cross_operation", "refused", "resource.not-found", "Resource was not found.", address="operation"),
    setup=[fixture("operation", "other-subject", {"operation": "01JPHASE2OTHERSUBJECT01", "owner": "local:1001"})],
    postconditions=[condition("/probes/disclosed_ledger_fields", "equals", 0)],
))

add_vector("http", "unauthenticated-startup", {
    "$schema": "../../schemas/vector.json",
    "action": {
        "configuration": {
            "authentication": "none",
            "listen": "0.0.0.0:7443",
            "tls": False,
            "trusted_tunnel": False,
        },
        "kind": "startup",
    },
    "context": http_context(subject="unmapped", authenticated=False, reachable=True),
    "covers": ["security.unauthenticated-reachable-startup"],
    "expected": {
        "kind": "startup-outcome",
        "outcome": {
            "code": "daemon.startup-refused",
            "message": "Reachable listener requires authentication and transport protection.",
            "started": False,
        },
    },
    "id": "reachable-unauthenticated-listener-refuses-startup",
    "layer": "http",
    "phase": 2,
    "postconditions": base_postconditions()
    + [condition("/probes/listener_bound", "equals", False), condition("/driver/probe_count", "equals", 0)],
    "setup": [],
})

add_vector("driver", "stale-capability", driver_vector(
    "stale-capability-is-refused-before-dispatch", ["security.stale-capability"], "exec",
    {"admitted_snapshot": CAPABILITY_SNAPSHOT, "operation": "exec.start"},
    {"code": "exec.capability-stale", "status": "refused"},
    setup=[fixture("backend", "generation-changed", {"admitted_generation": 7, "current_generation": 8})],
    postconditions=[condition("/driver/dispatch_count", "equals", 0), condition("/probes/credential_acquisition_count", "equals", 0)],
))

add_vector("driver", "process-tree-cancel", driver_vector(
    "cancel-reaps-daemonized-descendants", ["security.process-tree-cancellation"], "exec",
    {"grace_ms": 1000, "operation": "exec.signal", "signal": "TERM", "target": "ex_vector"},
    {"exit": {"code": None, "signal": "KILL"}, "state": "cancelled"},
    setup=[fixture("process-tree", "daemonized-ignore-term", {"descendants": 3, "marker_sha256": "4" * 64})],
    postconditions=[condition("/probes/cgroup_process_count", "equals", 0), condition("/probes/marker_changed_after_observation", "equals", False)],
))

add_vector("driver", "timeout", driver_vector(
    "timeout-kills-whole-cgroup", ["security.timeout"], "exec",
    {"argv": ["/fixture/sleep-forever"], "operation": "exec.start", "timeout_ms": 100},
    {"exit": {"code": None, "signal": "KILL"}, "state": "cancelled"},
    setup=[fixture("process-tree", "forking-sleeper", {"descendants": 2})],
    postconditions=[condition("/probes/cgroup_process_count", "equals", 0), condition("/probes/elapsed_ms_max", "lte", 31100)],
))

add_vector("driver", "output-truncation", driver_vector(
    "output-cap-drains-past-truncation", ["security.output-draining"], "exec",
    {"argv": ["/fixture/fill-both-pipes"], "operation": "exec.start", "output_limit_bytes": 65536},
    {"state": "exited", "stderr_truncated": True, "stdout_truncated": True},
    setup=[fixture("output", "twice-cap-concurrent", {"bytes_per_stream": 131072})],
    postconditions=[condition("/probes/pipe_deadlock", "equals", False), condition("/probes/stdout_drained_bytes", "equals", 131072), condition("/probes/stderr_drained_bytes", "equals", 131072)],
))

add_vector("driver", "no-egress", driver_vector(
    "minimum-exec-has-no-egress", [], "exec",
    {"argv": ["/fixture/probe-network"], "network": "none", "operation": "exec.start"},
    {"blocked_classes": ["dns", "loopback", "link-local", "metadata", "private", "public"], "state": "exited"},
    setup=[fixture("network", "all-address-classes", {"control_socket": True, "dns": True})],
    postconditions=[condition("/probes/successful_connect_count", "equals", 0), condition("/probes/distinct_network_namespace", "equals", True)],
))

add_vector("driver", "credential-inheritance", driver_vector(
    "daemon-authority-never-reaches-child", ["security.daemon-environment", "security.daemon-fd", "security.daemon-credential"], "exec",
    {"argv": ["/fixture/inspect-env-and-fds"], "environment": {"allow": [], "set": {"VECTOR_VISIBLE": "yes"}}, "operation": "exec.start"},
    {"state": "exited", "visible_environment": {"VECTOR_VISIBLE": "yes"}},
    setup=[fixture("daemon-authority", "sentinels", {"bearer_sha256": "5" * 64, "environment_sha256": "6" * 64, "fd_sha256": "8" * 64})],
    postconditions=[condition("/probes/daemon_environment_visible", "equals", False), condition("/probes/daemon_fd_visible", "equals", False), condition("/probes/daemon_credential_visible", "equals", False), condition("/probes/diagnostic_sentinel_count", "equals", 0)],
))

add_vector("driver", "machinery-failure", driver_vector(
    "accepted-driver-failure-is-terminal", [], "workspace",
    {"operation": "workspace.write-file", "path": "src/main.txt", "workspace": "ws_vector"},
    {"code": "workspace.driver-failed", "status": "failed"},
    setup=[fixture("fault", "atomic-rename-eio", {"after_acceptance": True})],
    postconditions=[condition("/probes/operation_state", "equals", "terminal"), condition("/probes/original_target_intact", "equals", True)],
))

failed_write_op = "01JPHASE2FAILEDWRITE0001"
add_vector("http", "machinery-failure", http_vector(
    "accepted-machinery-failure-is-wire-failed", ["error.failed"], "workspace.file.write",
    mutation_request("PUT", "/v1/workspaces/ws_vector/files/src/main.txt", failed_write_op, {"content": {"data": "aGVsbG8=", "encoding": "base64"}}, "req_failed_write1"), 500,
    error_body("req_failed_write1", "failed", "workspace.driver-failed", "Atomic replacement failed after operation acceptance.", operation=failed_write_op, address="workspace.file", retriable=True),
    setup=[fixture("fault", "atomic-rename-eio", {"after_acceptance": True}), fixture("workspace", "owned-with-old-file", {"owner": SUBJECT})],
    postconditions=[condition("/probes/operation_state", "equals", "terminal"), condition("/probes/original_target_intact", "equals", True)],
))

add_vector("driver", "crash-before-dispatch", driver_vector(
    "crash-before-dispatch-does-not-mutate", ["lifecycle.crash-before-dispatch"], "workspace",
    {"operation": "workspace.write-file", "operation_id": "01JPHASE2CRASHBEFORE001", "path": "src/main.txt"},
    {"operation_state_after_restart": "accepted"},
    setup=[fixture("crash", "after-ledger-before-dispatch", {"point": "accepted-commit"})],
    postconditions=[condition("/driver/dispatch_count", "equals", 0), condition("/probes/target_sha256", "equals", "9" * 64)],
))

add_vector("driver", "crash-after-dispatch", driver_vector(
    "crash-after-dispatch-never-fabricates-success", ["lifecycle.crash-after-dispatch", "lifecycle.unknown-preserved"], "workspace",
    {"operation": "workspace.write-file", "operation_id": "01JPHASE2CRASHAFTER0001", "path": "src/main.txt"},
    {"operation_state_after_restart": "unknown"},
    setup=[fixture("crash", "after-mutation-before-terminal-commit", {"point": "driver-return"})],
    postconditions=[condition("/driver/repeated_mutation_count", "equals", 0), condition("/probes/fabricated_success", "equals", False)],
))

add_vector("driver", "lost-answer", driver_vector(
    "lost-answer-reconciles-with-original-operation", ["lifecycle.lost-answer-reconciliation"], "workspace",
    {"operation": "workspace.create", "operation_id": "01JPHASE2LOSTANSWER00001"},
    {"operation_state": "terminal", "resource": "ws_lostanswer"},
    setup=[fixture("transport", "drop-after-terminal-commit", {"drop_answer": True})],
    postconditions=[condition("/driver/dispatch_count", "equals", 1), condition("/probes/reconciled_operation_id", "equals", "01JPHASE2LOSTANSWER00001")],
))

oversized_body_bytes = b" " * 1048577
add_vector("http", "input-body-limit", {
    "$schema": "../../schemas/vector.json",
    "action": {
        "kind": "raw-http",
        "operation": "workspace.create",
        "request": {
            "body": {
                "repeat": {"count": len(oversized_body_bytes), "octet_hex": "20"},
                "sha256": hashlib.sha256(oversized_body_bytes).hexdigest(),
            },
            "headers": {"content-type": "application/json", "x-request-id": "req_input_limit01"},
            "method": "POST",
            "path": "/v1/workspaces",
        },
    },
    "context": http_context(),
    "covers": ["security.bounds.input"],
    "expected": {
        "kind": "http-response",
        "response": {
            "body": error_body("req_input_limit01", "exhausted", "request.body-limit", "Request body exceeds the configured byte limit.", address="body", retriable=False),
            "status": 429,
        },
    },
    "id": "input-body-over-limit-is-exhausted",
    "layer": "http",
    "phase": 2,
    "postconditions": base_postconditions()
    + [condition("/probes/json_parse_count", "equals", 0), condition("/driver/dispatch_count", "equals", 0)],
    "setup": [fixture("transport", "body-limit", {"limit_bytes": 1048576})],
})

add_vector("driver", "resource-limits", driver_vector(
    "exec-resource-limits-are-cgroup-enforced", ["security.bounds.resource"], "exec",
    {"cpu_millis": 100, "memory_bytes": 16777216, "operation": "exec.start", "processes": 2},
    {"limit": "memory", "state": "exited"},
    setup=[fixture("process-tree", "exceed-each-cgroup-limit", {"cpu_millis": 101, "memory_bytes": 16777217, "processes": 3})],
    postconditions=[condition("/probes/max_processes", "lte", 2), condition("/probes/max_memory_bytes", "lte", 16777216), condition("/probes/cpu_millis", "lte", 100)],
))

nonzero = exec_resource(state="exited", exit_value={"code": 1, "signal": None})
add_vector("http", "exec-nonzero-is-observation", http_vector(
    "nonzero-exit-is-not-wire-failure", ["behavior.nonzero-exit-observation"], "exec.start",
    mutation_request("POST", "/v1/execs", "01JPHASE2EXECNONZERO001", {**start_input, "argv": ["/usr/bin/false"], "wait": True}, "req_exec_nonzero1"), 200,
    success_body("req_exec_nonzero1", nonzero, "01JPHASE2EXECNONZERO001"),
    setup=[fixture("machine", "minimum-host", MACHINE), fixture("workspace", "owned", {"owner": SUBJECT})],
    postconditions=[condition("/probes/wire_error_count", "equals", 0)],
))

# Design 04 fixes the future Git boundary now. These phase-6 driver fixtures are executable but do
# not claim that phase 2 serves Git.
for filename, vector_id, coverage, attack in [
    ("git-rebinding", "git-dns-rebinding-refuses-before-connect", "security.git.rebinding", "dns-rebinding"),
    ("git-redirect", "git-redirect-escape-refuses-before-connect", "security.git.redirect", "redirect-cross-aperture"),
    ("git-proxy", "git-caller-proxy-refuses-before-connect", "security.git.proxy", "caller-proxy"),
    ("git-helper", "git-credential-helper-is-disabled", "security.git.helper", "credential-helper"),
    ("git-hook", "git-hooks-are-disabled", "security.git.hook", "checkout-hook"),
    ("git-lfs", "git-lfs-secondary-fetch-is-disabled", "security.git.lfs", "lfs-secondary-fetch"),
    ("git-submodule", "git-submodule-secondary-fetch-is-disabled", "security.git.submodule", "submodule-secondary-fetch"),
]:
    add_vector("driver", filename, driver_vector(
        vector_id, [coverage], "workspace", {"attack": attack, "operation": "workspace.git-materialize", "source": "source_vector"},
        {"code": "workspace.git-destination-refused", "status": "refused"},
        setup=[fixture("git-source", attack, {"aperture": "public", "credential_binding": "cred_vector", "source": "source_vector"})],
        postconditions=[condition("/probes/credential_release_count", "equals", 0), condition("/probes/forbidden_connect_count", "equals", 0)],
        phase=6,
    ))


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


def evidence_for(requirement: str) -> list[dict[str, str]]:
    evidence = [
        {"id": vector["id"], "kind": "vector"}
        for vector in VECTORS.values()
        if requirement in vector["covers"]
    ]
    if requirement == "hash.transport-exclusions":
        evidence.append({"id": "workspace-create-transport-variant", "kind": "hash-fixture"})
    elif requirement == "hash.ledger-scope":
        evidence.append({"id": "workspace-create-transport-variant", "kind": "hash-fixture"})
    elif requirement == "hash.different-input-conflict":
        evidence.append({"id": "workspace-create-conflict", "kind": "hash-fixture"})
    return evidence


COVERAGE = {
    "$schema": "schemas/coverage.json",
    "format": "daemonloom.substrate-conformance-coverage.v1",
    "requirements": [
        {
            "evidence": evidence_for(requirement),
            "id": requirement,
            "source": (
                "Design 07 section 2" if requirement.startswith("route.")
                else "Design 04 section 9" if requirement.startswith("security.")
                else "Design 03" if requirement.startswith("lifecycle.")
                else "Design 07 section 6"
            ),
        }
        for requirement in sorted(REQUIRED_COVERAGE)
    ],
}


COVERAGE_SCHEMA = schema(
    "Conformance coverage inventory",
    schema_id="coverage",
    **closed_object(
        {
            "$schema": {"const": "schemas/coverage.json"},
            "format": {"const": "daemonloom.substrate-conformance-coverage.v1"},
            "requirements": {
                "items": closed_object(
                    {
                        "evidence": {
                            "items": closed_object(
                                {"id": {"pattern": "^[a-z][a-z0-9-]+$", "type": "string"}, "kind": {"enum": ["vector", "hash-fixture"]}},
                                ["kind", "id"],
                            ),
                            "minItems": 1,
                            "type": "array",
                        },
                        "id": {"pattern": "^[a-z][a-z0-9.-]+$", "type": "string"},
                        "source": {"minLength": 1, "type": "string"},
                    },
                    ["id", "source", "evidence"],
                ),
                "minItems": 1,
                "type": "array",
            },
        },
        ["$schema", "format", "requirements"],
    ),
)


VECTOR_SCHEMA = schema(
    "Substrate conformance vector",
    schema_id="vector",
    **closed_object(
        {
            "$schema": {"const": "../../schemas/vector.json"},
            "action": {
                "oneOf": [
                    closed_object(
                        {
                            "kind": {"const": "http"},
                            "operation": {"type": "string"},
                            "request": closed_object(
                                {
                                    "body": {"type": "object"},
                                    "headers": {"type": "object"},
                                    "method": {"enum": ["GET", "POST", "PUT", "DELETE"]},
                                    "path": {"pattern": "^/v1/", "type": "string"},
                                    "query": {"type": "object"},
                                },
                                ["method", "path", "query", "headers"],
                            ),
                            "valid_address": {"type": "boolean"},
                            "valid_input": {"type": "boolean"},
                        },
                        ["kind", "operation", "request", "valid_address", "valid_input"],
                    ),
                    closed_object(
                        {"command": {"type": "object"}, "kind": {"const": "driver"}, "port": {"enum": ["machine", "workspace", "exec"]}},
                        ["kind", "port", "command"],
                    ),
                    closed_object(
                        {
                            "kind": {"const": "raw-http"},
                            "operation": {"type": "string"},
                            "request": closed_object(
                                {
                                    "body": closed_object(
                                        {
                                            "repeat": closed_object(
                                                {
                                                    "count": {"minimum": 1, "type": "integer"},
                                                    "octet_hex": {"pattern": "^[0-9a-f]{2}$", "type": "string"},
                                                },
                                                ["octet_hex", "count"],
                                            ),
                                            "sha256": {"pattern": "^[0-9a-f]{64}$", "type": "string"},
                                        },
                                        ["repeat", "sha256"],
                                    ),
                                    "headers": {"type": "object"},
                                    "method": {"enum": ["GET", "POST", "PUT", "DELETE"]},
                                    "path": {"pattern": "^/v1/", "type": "string"},
                                },
                                ["method", "path", "headers", "body"],
                            ),
                        },
                        ["kind", "operation", "request"],
                    ),
                    closed_object(
                        {
                            "kind": {"const": "http-sequence"},
                            "operation": {"type": "string"},
                            "steps": {
                                "items": closed_object(
                                    {
                                        "request": closed_object(
                                            {
                                                "body": {"type": "object"},
                                                "headers": {"type": "object"},
                                                "method": {"enum": ["GET", "POST", "PUT", "DELETE"]},
                                                "path": {"pattern": "^/v1/", "type": "string"},
                                                "query": {"type": "object"},
                                            },
                                            ["method", "path", "query", "headers", "body"],
                                        ),
                                        "valid_input": {"const": True},
                                    },
                                    ["request", "valid_input"],
                                ),
                                "minItems": 2,
                                "type": "array",
                            },
                        },
                        ["kind", "operation", "steps"],
                    ),
                    closed_object(
                        {
                            "configuration": closed_object(
                                {
                                    "authentication": {"const": "none"},
                                    "listen": {"minLength": 1, "type": "string"},
                                    "tls": {"const": False},
                                    "trusted_tunnel": {"const": False},
                                },
                                ["listen", "authentication", "tls", "trusted_tunnel"],
                            ),
                            "kind": {"const": "startup"},
                        },
                        ["kind", "configuration"],
                    ),
                ]
            },
            "context": closed_object(
                {
                    "actor": {"type": "string"},
                    "authority": {"enum": ["http-harness", "driver-harness"]},
                    "clock": {"$ref": "common.json#/$defs/timestamp"},
                    "deployment": {"minLength": 1, "type": "string"},
                    "subject": {"minLength": 1, "type": "string"},
                    "transport": closed_object(
                        {"authenticated": {"type": "boolean"}, "kind": {"enum": ["unix", "tcp"]}, "reachable": {"type": "boolean"}},
                        ["kind", "authenticated", "reachable"],
                    ),
                },
                ["authority", "clock", "deployment", "subject"],
            ),
            "covers": {"items": {"type": "string"}, "minItems": 0, "type": "array", "uniqueItems": True},
            "expected": {
                "oneOf": [
                    closed_object(
                        {"kind": {"const": "http-response"}, "response": closed_object({"body": {"type": "object"}, "status": {"maximum": 599, "minimum": 200, "type": "integer"}}, ["status", "body"])},
                        ["kind", "response"],
                    ),
                    closed_object({"kind": {"const": "driver-outcome"}, "outcome": {"type": "object"}}, ["kind", "outcome"]),
                    closed_object(
                        {
                            "kind": {"const": "startup-outcome"},
                            "outcome": closed_object(
                                {
                                    "code": {"const": "daemon.startup-refused"},
                                    "message": {"minLength": 1, "type": "string"},
                                    "started": {"const": False},
                                },
                                ["started", "code", "message"],
                            ),
                        },
                        ["kind", "outcome"],
                    ),
                    closed_object(
                        {
                            "kind": {"const": "http-sequence"},
                            "responses": {
                                "items": closed_object(
                                    {
                                        "body": {"type": "object"},
                                        "status": {"maximum": 599, "minimum": 200, "type": "integer"},
                                    },
                                    ["status", "body"],
                                ),
                                "minItems": 2,
                                "type": "array",
                            },
                        },
                        ["kind", "responses"],
                    ),
                ]
            },
            "id": {"pattern": "^[a-z][a-z0-9-]+$", "type": "string"},
            "layer": {"enum": ["http", "driver"]},
            "phase": {"enum": [2, 6]},
            "postconditions": {
                "items": closed_object(
                    {"actual": {"pattern": "^/", "type": "string"}, "expected": {}, "operator": {"enum": ["equals", "not-equals", "lte", "gte"]}},
                    ["actual", "operator", "expected"],
                ),
                "minItems": 1,
                "type": "array",
            },
            "setup": {
                "items": closed_object(
                    {"kind": {"enum": ["backend", "crash", "daemon-authority", "daemon-config", "exec", "exec-output", "fault", "git-source", "machine", "network", "operation", "output", "path-shape", "process-tree", "transport", "workspace"]}, "name": {"minLength": 1, "type": "string"}, "state": {}},
                    ["kind", "name", "state"],
                ),
                "type": "array",
            },
        },
        ["$schema", "id", "phase", "layer", "covers", "context", "setup", "action", "expected", "postconditions"],
    ),
)


RUNNER = {
    "$schema": "schemas/runner.json",
    "comparison": {
        "expected": "exact-json-value-equality",
        "postconditions": "rfc6901-pointer-plus-declared-scalar-operator",
    },
    "exit_codes": {"0": "pass", "1": "conformance-failure", "2": "invalid-input", "3": "harness-failure"},
    "format": "daemonloom.substrate-clean-room-runner.v1",
    "invocation": {
        "argv": ["<runner>", "--bundle", "<bundle-dir>", "--vector", "<bundle-relative-vector-path>", "--output", "<runner-result.json>"],
        "network": "disabled-unless-vector-setup-provides-sentinels",
        "working_directory": "new-empty-directory",
    },
    "isolation": {
        "producer_input": "action-only",
        "repository_access": "bundle-only",
        "setup_delivery": "runner-only",
    },
    "protocol": {
        "result_schema": "schemas/runner-result.json",
        "vector_schema": "schemas/vector.json",
    },
    "roles": {
        "producer": "daemon or driver under test; receives only action bytes/commands",
        "runner": "independent clean-room harness; owns trusted setup, probes, comparison, and reporting",
    },
    "selection": {
        "fixture_identity": "out-of-band-only-never-request-data",
        "vector": "manifest-listed-bundle-relative-path",
    },
}

RUNNER_RESULT_SCHEMA = schema(
    "Clean-room runner result",
    schema_id="runner-result",
    **closed_object(
        {
            "diagnostics": {"items": {"type": "string"}, "type": "array"},
            "expected_match": {"type": "boolean"},
            "format": {"const": "daemonloom.substrate-runner-result.v1"},
            "postconditions": {
                "items": closed_object(
                    {
                        "actual": {},
                        "expected": {},
                        "matched": {"type": "boolean"},
                        "operator": {"enum": ["equals", "not-equals", "lte", "gte"]},
                        "pointer": {"pattern": "^/", "type": "string"},
                    },
                    ["pointer", "operator", "expected", "actual", "matched"],
                ),
                "type": "array",
            },
            "status": {"enum": ["pass", "fail"]},
            "vector_id": {"pattern": "^[a-z][a-z0-9-]+$", "type": "string"},
        },
        ["format", "vector_id", "status", "expected_match", "postconditions", "diagnostics"],
        allOf=[
            {
                "if": {"properties": {"status": {"const": "pass"}}, "required": ["status"]},
                "then": {
                    "properties": {
                        "diagnostics": {"maxItems": 0},
                        "expected_match": {"const": True},
                        "postconditions": {"items": {"properties": {"matched": {"const": True}}}},
                    }
                },
            }
        ],
    ),
)

RUNNER_SCHEMA = schema(
    "Clean-room conformance runner interface",
    schema_id="runner",
    **closed_object(
        {
            "$schema": {"const": "schemas/runner.json"},
            "comparison": closed_object(
                {
                    "expected": {"const": "exact-json-value-equality"},
                    "postconditions": {"const": "rfc6901-pointer-plus-declared-scalar-operator"},
                },
                ["expected", "postconditions"],
            ),
            "exit_codes": closed_object(
                {"0": {"const": "pass"}, "1": {"const": "conformance-failure"}, "2": {"const": "invalid-input"}, "3": {"const": "harness-failure"}},
                ["0", "1", "2", "3"],
            ),
            "format": {"const": "daemonloom.substrate-clean-room-runner.v1"},
            "invocation": closed_object(
                {
                    "argv": {"items": {"type": "string"}, "minItems": 7, "type": "array"},
                    "network": {"const": "disabled-unless-vector-setup-provides-sentinels"},
                    "working_directory": {"const": "new-empty-directory"},
                },
                ["argv", "working_directory", "network"],
            ),
            "isolation": closed_object(
                {
                    "producer_input": {"const": "action-only"},
                    "repository_access": {"const": "bundle-only"},
                    "setup_delivery": {"const": "runner-only"},
                },
                ["repository_access", "setup_delivery", "producer_input"],
            ),
            "protocol": closed_object(
                {"result_schema": {"const": "schemas/runner-result.json"}, "vector_schema": {"const": "schemas/vector.json"}},
                ["vector_schema", "result_schema"],
            ),
            "roles": closed_object(
                {"producer": {"type": "string"}, "runner": {"type": "string"}}, ["producer", "runner"]
            ),
            "selection": closed_object(
                {
                    "fixture_identity": {"const": "out-of-band-only-never-request-data"},
                    "vector": {"const": "manifest-listed-bundle-relative-path"},
                },
                ["vector", "fixture_identity"],
            ),
        },
        ["$schema", "format", "roles", "invocation", "isolation", "selection", "protocol", "comparison", "exit_codes"],
    ),
)


OPERATION_KINDS = [route[0] for route in ROUTES if route[5] == "keyed"]
MUTATION_RESULT_REFS = [
    {"$ref": f"results/{Path(route[9]).name}"}
    for route in ROUTES
    if route[5] == "keyed"
]

OPERATION = schema(
    "Subject-scoped operation record with structural lifecycle invariants",
    schema_id="operation",
    **{
        "$defs": {
            "base-properties": {
                "accepted_at": {"type": ["string", "null"], "format": "date-time"},
                "actor": {"minLength": 1, "type": "string"},
                "capability_snapshot": {"oneOf": [{"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"}, {"type": "null"}]},
                "operation": {"$ref": "common.json#/$defs/operation-id"},
                "operation_kind": {"enum": OPERATION_KINDS},
                "outcome": {},
                "principal": {"type": ["string", "null"]},
                "request_hash": {"pattern": "^[0-9a-f]{64}$", "type": "string"},
                "resource": {"type": ["string", "null"]},
                "state": {"enum": ["refused", "accepted", "unknown", "terminal"]},
                "terminal_at": {"type": ["string", "null"], "format": "date-time"},
            },
            "error-outcome": closed_object({"error": {"$ref": "error.json#/$defs/detail"}, "kind": {"const": "error"}}, ["kind", "error"]),
            "success-outcome": closed_object({"kind": {"const": "success"}, "result": {"oneOf": MUTATION_RESULT_REFS}}, ["kind", "result"]),
        },
        "oneOf": [
            closed_object({**{
                "accepted_at": {"type": "null"}, "actor": {"minLength": 1, "type": "string"}, "capability_snapshot": {"type": "null"}, "operation": {"$ref": "common.json#/$defs/operation-id"}, "operation_kind": {"enum": OPERATION_KINDS}, "outcome": {"$ref": "#/$defs/error-outcome"}, "principal": {"type": ["string", "null"]}, "request_hash": {"pattern": "^[0-9a-f]{64}$", "type": "string"}, "resource": {"type": ["string", "null"]}, "state": {"const": "refused"}, "terminal_at": {"$ref": "common.json#/$defs/timestamp"}
            }}, ["operation", "operation_kind", "request_hash", "state", "accepted_at", "terminal_at", "capability_snapshot", "actor", "principal", "resource", "outcome"]),
            closed_object({
                "accepted_at": {"$ref": "common.json#/$defs/timestamp"}, "actor": {"minLength": 1, "type": "string"}, "capability_snapshot": {"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"}, "operation": {"$ref": "common.json#/$defs/operation-id"}, "operation_kind": {"enum": OPERATION_KINDS}, "outcome": {"type": "null"}, "principal": {"type": ["string", "null"]}, "request_hash": {"pattern": "^[0-9a-f]{64}$", "type": "string"}, "resource": {"type": ["string", "null"]}, "state": {"enum": ["accepted", "unknown"]}, "terminal_at": {"type": "null"}
            }, ["operation", "operation_kind", "request_hash", "state", "accepted_at", "terminal_at", "capability_snapshot", "actor", "principal", "resource", "outcome"]),
            closed_object({
                "accepted_at": {"$ref": "common.json#/$defs/timestamp"}, "actor": {"minLength": 1, "type": "string"}, "capability_snapshot": {"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"}, "operation": {"$ref": "common.json#/$defs/operation-id"}, "operation_kind": {"enum": OPERATION_KINDS}, "outcome": {"oneOf": [{"$ref": "#/$defs/success-outcome"}, {"$ref": "#/$defs/error-outcome"}]}, "principal": {"type": ["string", "null"]}, "request_hash": {"pattern": "^[0-9a-f]{64}$", "type": "string"}, "resource": {"type": ["string", "null"]}, "state": {"const": "terminal"}, "terminal_at": {"$ref": "common.json#/$defs/timestamp"}
            }, ["operation", "operation_kind", "request_hash", "state", "accepted_at", "terminal_at", "capability_snapshot", "actor", "principal", "resource", "outcome"]),
        ],
        "allOf": [
            {
                "if": {
                    "properties": {
                        "operation_kind": {"const": route[0]},
                        "outcome": {
                            "properties": {"kind": {"const": "success"}},
                            "required": ["kind"],
                        },
                    },
                    "required": ["operation_kind", "outcome"],
                },
                "then": {
                    "properties": {
                        "outcome": {
                            "properties": {"result": {"$ref": f"results/{Path(route[9]).name}"}},
                            "required": ["result"],
                        }
                    }
                },
            }
            for route in ROUTES
            if route[5] == "keyed"
        ],
    },
)


REQUEST = schema(
    "Closed phase-2 mutation request union",
    schema_id="request",
    description="The route registry selects one branch; no generic input object is authoritative.",
    **{
        "anyOf": [
            closed_object(
                {
                    "input": {"$ref": f"inputs/{Path(route[8]).name}"},
                    "op": {"$ref": "common.json#/$defs/operation-id"},
                },
                ["op", "input"],
            )
            for route in ROUTES
            if route[5] == "keyed"
        ]
    },
)

RESPONSE = schema(
    "Closed phase-2 success response union",
    schema_id="response",
    description="The route registry selects one branch; no generic result value is authoritative.",
    **{
        "anyOf": [
            closed_object(
                {
                    "api_version": {"$ref": "common.json#/$defs/api-version"},
                    "operation": {"$ref": "common.json#/$defs/operation-id"},
                    "request_id": {"$ref": "common.json#/$defs/request-id"},
                    "result": {"$ref": f"results/{Path(route[9]).name}"},
                },
                ["api_version", "request_id", "result"]
                + (["operation"] if route[5] == "keyed" else []),
            )
            for route in ROUTES
        ]
    },
)

EVENT = schema(
    "Phase-2 persisted state transition",
    schema_id="event",
    description="Reserved state record; phase 2 serves no event endpoint.",
    **closed_object(
        {
            "actor": {"minLength": 1, "type": "string"},
            "generation": {"minimum": 1, "type": "integer"},
            "observed_at": {"$ref": "common.json#/$defs/timestamp"},
            "op": {"$ref": "common.json#/$defs/operation-id"},
            "principal": {"type": ["string", "null"]},
            "resource": {"minLength": 4, "type": "string"},
            "seq": {"minimum": 1, "type": "integer"},
            "transition": {"enum": ["workspace.created", "workspace.destroyed", "exec.accepted", "exec.running", "exec.exited", "exec.cancelled", "exec.unknown"]},
        },
        ["generation", "seq", "resource", "transition", "observed_at", "actor", "principal", "op"],
    ),
)


SCHEMA_FIXTURES_SCHEMA = schema(
    "Positive and negative schema invariant fixtures",
    schema_id="schema-fixtures",
    **closed_object(
        {
            "$schema": {"const": "../schemas/schema-fixtures.json"},
            "format": {"const": "daemonloom.substrate-schema-fixtures.v1"},
            "invalid": {
                "items": closed_object(
                    {"id": {"pattern": "^[a-z][a-z0-9-]+$", "type": "string"}, "instance": {}},
                    ["id", "instance"],
                ),
                "minItems": 1,
                "type": "array",
            },
            "schema": {"pattern": "^schemas/[a-z0-9/-]+\\.json$", "type": "string"},
            "valid": {"items": {}, "minItems": 1, "type": "array"},
        },
        ["$schema", "format", "schema", "valid", "invalid"],
    ),
)


REFUSED_OPERATION = {
    "accepted_at": None,
    "actor": "vector-client",
    "capability_snapshot": None,
    "operation": "01JPHASE2STATE_REFUSED1",
    "operation_kind": "workspace.create",
    "outcome": {
        "error": {
            "class": "refused",
            "code": "request.schema-invalid",
            "message": "Request does not match the closed operation schema.",
            "operation": "01JPHASE2STATE_REFUSED1",
            "retriable": False,
        },
        "kind": "error",
    },
    "principal": None,
    "request_hash": "a" * 64,
    "resource": None,
    "state": "refused",
    "terminal_at": FIXED_TIME,
}

ACCEPTED_OPERATION = {
    "accepted_at": FIXED_TIME,
    "actor": "vector-client",
    "capability_snapshot": CAPABILITY_SNAPSHOT,
    "operation": "01JPHASE2STATE_ACCEPTED1",
    "operation_kind": "workspace.create",
    "outcome": None,
    "principal": None,
    "request_hash": "b" * 64,
    "resource": None,
    "state": "accepted",
    "terminal_at": None,
}

UNKNOWN_OPERATION = {
    **ACCEPTED_OPERATION,
    "operation": "01JPHASE2STATE_UNKNOWN01",
    "request_hash": "c" * 64,
    "resource": "ws_unknown",
    "state": "unknown",
}

OPERATION_STATE_FIXTURES = {
    "$schema": "../schemas/schema-fixtures.json",
    "format": "daemonloom.substrate-schema-fixtures.v1",
    "invalid": [
        {"id": "accepted-cannot-have-outcome", "instance": {**ACCEPTED_OPERATION, "outcome": {"kind": "success", "result": workspace("ws_invalid")}}},
        {"id": "operation-kind-must-match-result", "instance": {**terminal_operation, "outcome": {"kind": "success", "result": exec_resource()}}},
        {"id": "terminal-requires-terminal-time", "instance": {**terminal_operation, "terminal_at": None}},
        {"id": "refused-cannot-have-capability-snapshot", "instance": {**REFUSED_OPERATION, "capability_snapshot": CAPABILITY_SNAPSHOT}},
    ],
    "schema": "schemas/operation.json",
    "valid": [REFUSED_OPERATION, ACCEPTED_OPERATION, UNKNOWN_OPERATION, terminal_operation],
}

RESOURCE_INVARIANT_FIXTURES = {
    "$schema": "../schemas/schema-fixtures.json",
    "format": "daemonloom.substrate-schema-fixtures.v1",
    "invalid": [
        {"id": "accepted-cannot-claim-applied", "instance": exec_resource(state="accepted", applied_value=applied())},
        {"id": "running-requires-applied", "instance": exec_resource(state="running", applied_value=None)},
        {"id": "minimum-host-applied-network-cannot-widen", "instance": {**exec_resource(), "applied": {**applied(), "network": "aperture"}}},
        {"id": "applied-network-cannot-weaken-request", "instance": {**exec_resource(), "requested": requested("aperture")}},
        {"id": "terminal-requires-exit-observation", "instance": exec_resource(state="exited", exit_value=None)},
    ],
    "schema": "schemas/resource.json",
    "valid": [
        exec_resource(state="accepted"),
        exec_resource(state="running"),
        exec_resource(state="exited", exit_value={"code": 0, "signal": None}),
        exec_resource(state="cancelled", exit_value={"code": None, "signal": "TERM"}),
        exec_resource(state="unknown", applied_value=None),
    ],
}

RUNNER_RESULT_FIXTURES = {
    "$schema": "../schemas/schema-fixtures.json",
    "format": "daemonloom.substrate-schema-fixtures.v1",
    "invalid": [
        {
            "id": "pass-requires-exact-expected-match",
            "instance": {
                "diagnostics": [],
                "expected_match": False,
                "format": "daemonloom.substrate-runner-result.v1",
                "postconditions": [],
                "status": "pass",
                "vector_id": "machine-probe-is-observed",
            },
        },
        {
            "id": "pass-requires-every-postcondition",
            "instance": {
                "diagnostics": [],
                "expected_match": True,
                "format": "daemonloom.substrate-runner-result.v1",
                "postconditions": [
                    {
                        "actual": 1,
                        "expected": 0,
                        "matched": False,
                        "operator": "equals",
                        "pointer": "/driver/dispatch_count",
                    }
                ],
                "status": "pass",
                "vector_id": "machine-probe-is-observed",
            },
        },
    ],
    "schema": "schemas/runner-result.json",
    "valid": [
        {
            "diagnostics": [],
            "expected_match": True,
            "format": "daemonloom.substrate-runner-result.v1",
            "postconditions": [
                {
                    "actual": 0,
                    "expected": 0,
                    "matched": True,
                    "operator": "equals",
                    "pointer": "/driver/dispatch_count",
                }
            ],
            "status": "pass",
            "vector_id": "machine-probe-is-observed",
        },
        {
            "diagnostics": ["response differed"],
            "expected_match": False,
            "format": "daemonloom.substrate-runner-result.v1",
            "postconditions": [],
            "status": "fail",
            "vector_id": "machine-probe-is-observed",
        },
    ],
}


def render() -> None:
    fixed = {
        "schemas/common.json": COMMON,
        "schemas/capability.json": CAPABILITY,
        "schemas/error.json": ERROR,
        "schemas/event.json": EVENT,
        "schemas/operation.json": OPERATION,
        "schemas/operation-registry.json": OPERATION_REGISTRY_SCHEMA,
        "schemas/request.json": REQUEST,
        "schemas/resource.json": RESOURCE,
        "schemas/response.json": RESPONSE,
        "schemas/vector.json": VECTOR_SCHEMA,
        "schemas/hash-fixtures.json": HASH_SCHEMA,
        "schemas/coverage.json": COVERAGE_SCHEMA,
        "schemas/runner.json": RUNNER_SCHEMA,
        "schemas/runner-result.json": RUNNER_RESULT_SCHEMA,
        "schemas/schema-fixtures.json": SCHEMA_FIXTURES_SCHEMA,
        "operations.json": OPERATION_REGISTRY,
        "hashing.json": HASHING,
        "fixtures/canonical-hash.json": HASH_FIXTURES,
        "fixtures/operation-states.json": OPERATION_STATE_FIXTURES,
        "fixtures/resource-invariants.json": RESOURCE_INVARIANT_FIXTURES,
        "fixtures/runner-results.json": RUNNER_RESULT_FIXTURES,
        "coverage.json": COVERAGE,
        "runner.json": RUNNER,
    }
    for relative, value in {
        **fixed,
        **ADDRESS_SCHEMAS,
        **INPUT_SCHEMAS,
        **RESULT_SCHEMAS,
        **VECTORS,
    }.items():
        write_json(relative, value)

    files = []
    for path in sorted(BUNDLE.rglob("*")):
        if not path.is_file() or path.name == "bundle.json":
            continue
        relative = path.relative_to(BUNDLE).as_posix()
        data = path.read_bytes()
        files.append(
            {
                "byte_length": len(data),
                "media_type": "application/json" if path.suffix == ".json" else "text/markdown",
                "path": relative,
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        )
    manifest = {
        "api_version": "v1",
        "bundle_format": "daemonloom.contract-bundle.v1",
        "files": files,
        "generator": {
            "digest": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "name": "scripts/render-contract-bundle.py",
            "version": "1",
        },
        "name": "substrate-wire",
        "origin": "daemonloom",
        "source_base_commit": None,
        "status": "development",
        "version": "0.1.0",
    }
    write_json("bundle.json", manifest)


if __name__ == "__main__":
    render()
