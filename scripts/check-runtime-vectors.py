#!/usr/bin/env python3
"""Independent black-box checks for the minimum Unix-socket HTTP runtime."""

from __future__ import annotations

import argparse
import base64
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
from typing import Any


class UnixConnection(http.client.HTTPConnection):
    def __init__(self, socket_path: Path) -> None:
        super().__init__("localhost")
        self.socket_path = socket_path

    def connect(self) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(self.socket_path))


class Harness:
    def __init__(
        self, binary: Path, root: Path, cgroup_root: Path | None = None
    ) -> None:
        self.socket = root / "substrate.sock"
        command = [
            str(binary),
            "--socket",
            str(self.socket),
            "--state",
            str(root / "state.db"),
            "--workspaces",
            str(root / "workspaces"),
            "--deployment",
            "dep_cleanroom",
            "--allow-uid",
            str(os.getuid()),
        ]
        if cgroup_root is not None:
            command.extend(["--cgroup-root", str(cgroup_root)])
        environment = os.environ.copy()
        environment["SUBSTRATE_TEST_SECRET_SENTINEL"] = "must-not-reach-child"
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            env=environment,
        )
        deadline = time.monotonic() + 10
        while not self.socket.exists():
            if self.process.poll() is not None:
                error = self.process.stderr.read() if self.process.stderr else ""
                raise AssertionError(f"substrated exited before readiness: {error}")
            if time.monotonic() >= deadline:
                raise AssertionError("substrated did not create its Unix socket")
            time.sleep(0.02)

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            self.process.wait(timeout=10)
        if self.process.returncode != 0:
            error = self.process.stderr.read() if self.process.stderr else ""
            raise AssertionError(f"substrated shutdown failed: {error}")

    def call(
        self,
        method: str,
        path: str,
        request_id: str,
        body: bytes | None = None,
    ) -> tuple[int, dict[str, Any]]:
        connection = UnixConnection(self.socket)
        headers = {"x-request-id": request_id}
        if body is not None:
            headers["content-type"] = "application/json"
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        payload = json.loads(response.read())
        status = response.status
        connection.close()
        assert payload["api_version"] == "v1"
        assert payload["request_id"] == request_id
        return status, payload


def mutation(operation: str, input_value: dict[str, Any]) -> bytes:
    return json.dumps(
        {"op": operation, "input": input_value},
        separators=(",", ":"),
    ).encode()


def expect_error(
    response: tuple[int, dict[str, Any]], status: int, code: str
) -> None:
    actual_status, payload = response
    assert actual_status == status, payload
    assert payload["error"]["code"] == code, payload


def check_startup_refusal(binary: Path, root: Path) -> None:
    result = subprocess.run(
        [
            str(binary),
            "--socket",
            str(root / "unmapped.sock"),
            "--state",
            str(root / "unmapped.db"),
            "--workspaces",
            str(root / "unmapped-workspaces"),
            "--deployment",
            "dep_unmapped",
        ],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    assert result.returncode != 0
    assert "explicit --allow-uid mapping is required" in result.stderr
    assert not (root / "unmapped.sock").exists()


def exec_input(
    workspace: str,
    snapshot: str,
    argv: list[str],
    *,
    wait: bool,
    timeout_ms: int = 5000,
    output_bytes: int = 65536,
    environment: dict[str, str] | None = None,
) -> dict[str, Any]:
    return {
        "workspace": workspace,
        "argv": argv,
        "env": {"allow": [], "set": environment or {}},
        "sandbox": {
            "capability_snapshot": snapshot,
            "network": "none",
            "profile": "workspace",
            "require": True,
        },
        "limits": {
            "timeout_ms": timeout_ms,
            "output_bytes": output_bytes,
            "processes": 16,
            "memory_bytes": 67108864,
            "cpu_millis": 1000,
        },
        "wait": wait,
    }


def read_output(harness: Harness, exec_id: str, stream: str) -> dict[str, Any]:
    status, payload = harness.call(
        "GET",
        f"/v1/execs/{exec_id}/output?stream={stream}&offset=0&limit_bytes=65536",
        f"req_clean_output_{stream}",
    )
    assert status == 200, payload
    return payload["result"]


def check_confined_execs(
    harness: Harness,
    workspace: str,
    snapshot: str,
    cgroup_root: Path,
) -> int:
    passed = 0
    status, executed = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_exec",
        mutation(
            "01JPHASE2CLEANEXEC0001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/printf", "hello"],
                wait=True,
            ),
        ),
    )
    assert status == 200, executed
    assert executed["result"]["state"] == "exited"
    assert executed["result"]["exit"] == {"code": 0, "signal": None}
    assert executed["result"]["applied"]["network"] == "none"
    assert not (cgroup_root / executed["result"]["applied"]["cgroup"]).exists()
    output = read_output(harness, executed["result"]["id"], "stdout")
    assert base64.b64decode(output["content"]["data"]) == b"hello"
    assert output["eof"] is True
    passed += 1

    status, environment_exec = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_environment",
        mutation(
            "01JPHASE2CLEANENV000001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/env"],
                wait=True,
                environment={"VECTOR_VISIBLE": "yes"},
            ),
        ),
    )
    assert status == 200, environment_exec
    environment_output = read_output(
        harness, environment_exec["result"]["id"], "stdout"
    )
    visible = base64.b64decode(environment_output["content"]["data"])
    assert visible == b"VECTOR_VISIBLE=yes\n", visible
    passed += 1

    no_egress_program = (
        "import socket;socket.create_connection(('1.1.1.1',53),1)"
    )
    status, no_egress = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_no_egress",
        mutation(
            "01JPHASE2CLEANNOEGRESS1",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/python3", "-c", no_egress_program],
                wait=True,
            ),
        ),
    )
    assert status == 200, no_egress
    assert no_egress["result"]["state"] == "exited"
    assert no_egress["result"]["exit"]["code"] != 0
    passed += 1

    pids_program = """
import os,time
children=[]
for _ in range(64):
    try:
        pid=os.fork()
    except OSError:
        break
    if pid == 0:
        time.sleep(.2)
        os._exit(0)
    children.append(pid)
print(len(children), flush=True)
for pid in children:
    os.waitpid(pid,0)
"""
    status, pids_exec = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_pids",
        mutation(
            "01JPHASE2CLEANPIDS00001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/python3", "-c", pids_program],
                wait=True,
            ),
        ),
    )
    assert status == 200, pids_exec
    pids_output = read_output(harness, pids_exec["result"]["id"], "stdout")
    children = int(base64.b64decode(pids_output["content"]["data"]).strip())
    assert 0 < children < 16, children
    passed += 1

    memory_program = (
        "x=bytearray(128*1024*1024);"
        "[(x.__setitem__(i,1)) for i in range(0,len(x),4096)];"
        "print(len(x))"
    )
    status, memory_exec = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_memory",
        mutation(
            "01JPHASE2CLEANMEMORY0001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/python3", "-c", memory_program],
                wait=True,
            ),
        ),
    )
    assert status == 200, memory_exec
    assert memory_exec["result"]["exit"] != {"code": 0, "signal": None}
    passed += 1

    fill_program = (
        "import os;"
        "os.write(1,b'x'*131072);"
        "os.write(2,b'y'*131072)"
    )
    status, filled = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_truncation",
        mutation(
            "01JPHASE2CLEANFILL00001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/python3", "-c", fill_program],
                wait=True,
            ),
        ),
    )
    assert status == 200, filled
    for stream in ("stdout", "stderr"):
        output = read_output(harness, filled["result"]["id"], stream)
        assert output["returned_bytes"] == 65536
        assert output["truncated"] is True
        assert base64.b64decode(output["content"]["data"]).endswith(
            b"[substrate: output truncated]\n"
        )
    passed += 1

    status, timed_out = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_timeout",
        mutation(
            "01JPHASE2CLEANTIMEOUT001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/sleep", "60"],
                wait=True,
                timeout_ms=100,
            ),
        ),
    )
    assert status == 200, timed_out
    assert timed_out["result"]["state"] == "cancelled"
    assert timed_out["result"]["exit"] == {"code": None, "signal": "KILL"}
    assert not (cgroup_root / timed_out["result"]["applied"]["cgroup"]).exists()
    passed += 1

    tree_program = (
        "import os,signal,time;"
        "signal.signal(signal.SIGTERM,signal.SIG_IGN);"
        "os.fork();time.sleep(60)"
    )
    status, running = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_tree_start",
        mutation(
            "01JPHASE2CLEANTREE00001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/python3", "-c", tree_program],
                wait=False,
            ),
        ),
    )
    assert status == 202, running
    exec_id = running["result"]["id"]
    cgroup_name = running["result"]["applied"]["cgroup"]
    time.sleep(0.1)
    status, cancelled = harness.call(
        "POST",
        f"/v1/execs/{exec_id}/signal",
        "req_clean_tree_signal",
        mutation(
            "01JPHASE2CLEANTREESIGNAL",
            {"signal": "TERM", "grace_ms": 100},
        ),
    )
    assert status == 200, cancelled
    assert cancelled["result"]["state"] == "cancelled"
    assert cancelled["result"]["exit"] == {
        "code": None,
        "signal": "KILL",
    }, cancelled
    assert not (cgroup_root / cgroup_name).exists()
    passed += 1
    return passed


def check_http_journey(
    harness: Harness, cgroup_root: Path | None = None
) -> int:
    passed = 0
    status, machine = harness.call("GET", "/v1/machine", "req_clean_machine")
    assert status == 200
    assert machine["result"]["driver"] == "host"
    assert machine["result"]["facts"]["workspace.guarded-io"] is True
    snapshot = machine["result"]["snapshot"]
    passed += 1

    operation = "01JPHASE2CLEANCREATE01"
    create_input = {"source": "empty", "labels": {"runner": "cleanroom"}}
    status, created = harness.call(
        "POST",
        "/v1/workspaces",
        "req_clean_create",
        mutation(operation, create_input),
    )
    assert status == 201
    workspace = created["result"]["id"]
    assert workspace.startswith("ws_")
    passed += 1

    status, replay = harness.call(
        "POST",
        "/v1/workspaces",
        "req_clean_replay",
        mutation(operation, create_input),
    )
    assert status == 201
    assert replay["result"]["id"] == workspace
    passed += 1

    expect_error(
        harness.call(
            "POST",
            "/v1/workspaces",
            "req_clean_conflict",
            mutation(operation, {"source": "empty", "labels": {"changed": "yes"}}),
        ),
        409,
        "operation.request-conflict",
    )
    passed += 1

    status, observed = harness.call(
        "GET", f"/v1/workspaces/{workspace}", "req_clean_get"
    )
    assert status == 200
    assert observed["result"]["state"] == "ready"
    passed += 1

    file_path = f"/v1/workspaces/{workspace}/files/main.txt"
    status, written = harness.call(
        "PUT",
        file_path,
        "req_clean_write",
        mutation(
            "01JPHASE2CLEANWRITE001",
            {"content": {"encoding": "base64", "data": "aGVsbG8="}},
        ),
    )
    assert status == 200
    assert written["result"]["atomic_replacement"] is True
    assert written["result"]["sha256"] == (
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    )
    passed += 1

    status, read = harness.call(
        "GET",
        f"{file_path}?mode=file&offset=0&limit_bytes=5",
        "req_clean_read",
    )
    assert status == 200
    assert read["result"]["content"]["data"] == "aGVsbG8="
    assert read["result"]["eof"] is True
    passed += 1

    expect_error(
        harness.call(
            "GET",
            f"/v1/workspaces/{workspace}/files/%2e%2e%2fetc%2fpasswd"
            "?mode=file&offset=0&limit_bytes=16",
            "req_clean_escape",
        ),
        422,
        "workspace.path-escape",
    )
    passed += 1

    expect_error(
        harness.call(
            "POST",
            "/v1/workspaces",
            "req_clean_strict",
            mutation(
                "01JPHASE2CLEANSTRICT01",
                {"source": "empty", "labels": {}, "secret": "forbidden"},
            ),
        ),
        422,
        "request.schema-invalid",
    )
    passed += 1

    expect_error(
        harness.call(
            "POST", "/v1/workspaces", "req_clean_limit", b" " * 1_048_577
        ),
        429,
        "request.body-limit",
    )
    passed += 1

    if cgroup_root is None:
        expect_error(
            harness.call(
                "POST",
                "/v1/execs",
                "req_clean_exec",
                mutation(
                    "01JPHASE2CLEANEXEC0001",
                    exec_input(
                        workspace,
                        snapshot,
                        ["/usr/bin/true"],
                        wait=False,
                    ),
                ),
            ),
            501,
            "exec.sandbox-unavailable",
        )
        passed += 1
    else:
        passed += check_confined_execs(harness, workspace, snapshot, cgroup_root)

    status, ledger = harness.call(
        "GET", f"/v1/ops/{operation}", "req_clean_operation"
    )
    assert status == 200
    assert ledger["result"]["state"] == "terminal"
    assert ledger["result"]["resource"] == workspace
    passed += 1

    status, deleted = harness.call(
        "DELETE",
        file_path,
        "req_clean_delete",
        mutation("01JPHASE2CLEANDELETE01", {}),
    )
    assert status == 200
    assert deleted["result"]["absent"] is True
    passed += 1

    status, destroyed = harness.call(
        "DELETE",
        f"/v1/workspaces/{workspace}",
        "req_clean_destroy",
        mutation("01JPHASE2CLEANDESTROY1", {}),
    )
    assert status == 200
    assert destroyed["result"]["absent"] is True
    passed += 1
    return passed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary", type=Path, default=Path("target/debug/substrated")
    )
    parser.add_argument("--cgroup-root", type=Path)
    arguments = parser.parse_args()
    binary = arguments.binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"missing daemon binary: {binary}")
    with tempfile.TemporaryDirectory(prefix="substrate-cleanroom-") as temporary:
        root = Path(temporary)
        check_startup_refusal(binary, root)
        harness = Harness(binary, root, arguments.cgroup_root)
        try:
            passed = check_http_journey(harness, arguments.cgroup_root)
        finally:
            harness.close()
    print(f"runtime clean-room: {passed} HTTP cases and startup refusal passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
