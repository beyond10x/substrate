#!/usr/bin/env python3
"""Independent black-box checks for the phase-3 Unix-socket HTTP runtime."""

from __future__ import annotations

import argparse
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import struct
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
        self.workspaces = root / "workspaces"
        command = [
            str(binary),
            "--socket",
            str(self.socket),
            "--state",
            str(root / "state.db"),
            "--workspaces",
            str(self.workspaces),
            "--deployment",
            "dep_cleanroom",
            "--allow-uid",
            str(os.getuid()),
            "--event-retention",
            "64",
        ]
        if cgroup_root is not None:
            command.extend(["--cgroup-root", str(cgroup_root)])
        self.command = command
        environment = os.environ.copy()
        environment["SUBSTRATE_TEST_SECRET_SENTINEL"] = "must-not-reach-child"
        self.process = subprocess.Popen(
            self.command,
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
                raise AssertionError(f"substrate-daemon exited before readiness: {error}")
            if time.monotonic() >= deadline:
                raise AssertionError("substrate-daemon did not create its Unix socket")
            time.sleep(0.02)

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
            self.process.wait(timeout=10)
        if self.process.returncode != 0:
            error = self.process.stderr.read() if self.process.stderr else ""
            raise AssertionError(f"substrate-daemon shutdown failed: {error}")

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

    def websocket(self, path: str) -> socket.socket:
        stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        stream.settimeout(10)
        stream.connect(str(self.socket))
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            f"GET {path} HTTP/1.1\r\n"
            "Host: localhost\r\n"
            "Connection: Upgrade\r\n"
            "Upgrade: websocket\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            f"Sec-WebSocket-Key: {key}\r\n\r\n"
        )
        stream.sendall(request.encode())
        head = receive_until(stream, b"\r\n\r\n")
        assert head.startswith(b"HTTP/1.1 101 "), head
        response_headers = {
            name.strip().lower(): value.strip()
            for line in head.decode("ascii").split("\r\n")[1:]
            if line and (name_value := line.split(":", 1))
            for name, value in [name_value]
        }
        expected_accept = base64.b64encode(
            hashlib.sha1(
                (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii"),
                usedforsecurity=False,
            ).digest()
        ).decode("ascii")
        assert response_headers["sec-websocket-accept"] == expected_accept
        return stream


def receive_until(stream: socket.socket, marker: bytes) -> bytes:
    value = bytearray()
    while marker not in value:
        chunk = stream.recv(1)
        if not chunk:
            raise AssertionError("connection closed before expected boundary")
        value.extend(chunk)
    return bytes(value)


def receive_exact(stream: socket.socket, count: int) -> bytes:
    value = bytearray()
    while len(value) < count:
        chunk = stream.recv(count - len(value))
        if not chunk:
            raise AssertionError("websocket closed mid-frame")
        value.extend(chunk)
    return bytes(value)


def wait_absent(path: Path, timeout: float = 3.0) -> None:
    deadline = time.monotonic() + timeout
    while path.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    assert not path.exists(), f"resource was not cleaned up while idle: {path}"


def websocket_frame(stream: socket.socket) -> tuple[int, bytes]:
    first, second = receive_exact(stream, 2)
    opcode = first & 0x0F
    length = second & 0x7F
    assert second & 0x80 == 0, "server frames must not be masked"
    if length == 126:
        length = struct.unpack("!H", receive_exact(stream, 2))[0]
    elif length == 127:
        length = struct.unpack("!Q", receive_exact(stream, 8))[0]
    return opcode, receive_exact(stream, length)


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


def check_dual_daemon_refusal(harness: Harness) -> None:
    result = subprocess.run(
        harness.command,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    assert result.returncode != 0
    assert "another substrate daemon owns this socket identity" in result.stderr
    status, machine = harness.call(
        "GET", "/v1/machine", "req_dual_daemon_owner_survives"
    )
    assert status == 200, machine


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

    status, pwd_exec = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_pwd",
        mutation(
            "01JPHASE3CLEANPWD000001",
            exec_input(
                workspace,
                snapshot,
                ["/usr/bin/test", "/workspace", "=", "/workspace"],
                wait=True,
                environment={"PWD": "/workspace"},
            ),
        ),
    )
    assert status == 200, pwd_exec
    assert pwd_exec["result"]["exit"] == {"code": 0, "signal": None}, pwd_exec
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

    status, trapping = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_trap_start",
        mutation(
            "01JPHASE3CLEANTRAPSTART1",
            exec_input(
                workspace,
                snapshot,
                [
                    "/usr/bin/sh",
                    "-c",
                    "trap 'exit 0' TERM; echo ready; while :; do sleep 1; done",
                ],
                wait=False,
            ),
        ),
    )
    assert status == 202, trapping
    time.sleep(0.1)
    status, trapped = harness.call(
        "POST",
        f"/v1/execs/{trapping['result']['id']}/signal",
        "req_clean_trap_signal",
        mutation("01JPHASE3CLEANTRAPSIGNAL", {"signal": "TERM", "grace_ms": 5000}),
    )
    assert status == 200, trapped
    assert trapped["result"]["state"] == "exited", trapped
    assert trapped["result"]["exit"] == {"code": 0, "signal": None}, trapped
    passed += 1

    for index in range(129):
        status, completed = harness.call(
            "POST",
            "/v1/execs",
            f"req_clean_waited_{index:03}",
            mutation(
                f"01JPHASE3WAITED{index:09}",
                exec_input(workspace, snapshot, ["/usr/bin/true"], wait=True),
            ),
        )
        assert status == 200, (index, completed)
    passed += 1

    for index in range(129):
        status, abandoned = harness.call(
            "POST",
            "/v1/execs",
            f"req_clean_abandoned_{index:03}",
            mutation(
                f"01JPHASE3ABANDON{index:09}",
                exec_input(workspace, snapshot, ["/usr/bin/true"], wait=False),
            ),
        )
        assert status == 202, (index, abandoned)
        if index % 16 == 15:
            time.sleep(0.3)
    time.sleep(0.3)
    status, after_abandon = harness.call(
        "POST",
        "/v1/execs",
        "req_clean_after_abandon",
        mutation(
            "01JPHASE3AFTERABANDON01",
            exec_input(workspace, snapshot, ["/usr/bin/true"], wait=True),
        ),
    )
    assert status == 200, after_abandon
    passed += 1
    return passed


def check_phase3_journey(
    harness: Harness,
    workspace: str,
    snapshot: str,
    cgroup_root: Path | None,
) -> int:
    passed = 0

    status, before = harness.call(
        "GET", "/v1/events?limit=100", "req_phase3_events_before"
    )
    assert status == 200, before
    source_generation = before["result"]["generation"]
    source_scope = before["result"]["source_scope"]
    start_cursor = before["result"]["next_cursor"]
    assert before["result"]["through_seq"] >= len(before["result"]["items"])
    passed += 1

    noise: list[str] = []
    for index in range(20):
        status, created = harness.call(
            "POST",
            "/v1/workspaces",
            f"req_phase3_noise_{index:02}",
            mutation(
                f"01JPHASE3NOISECREATE{index:02}",
                {"source": "empty", "labels": {"noise": f"{index:02}"}},
            ),
        )
        assert status == 201, created
        noise.append(created["result"]["id"])

    stream = harness.websocket(
        f"/v1/events/stream?cursor={start_cursor}&limit=1"
    )
    last_cursor = start_cursor
    boundary: dict[str, Any] | None = None
    event_frames = 0
    while boundary is None:
        opcode, payload = websocket_frame(stream)
        assert opcode == 1, (opcode, payload)
        frame = json.loads(payload)
        if frame["kind"] == "events":
            event_frames += 1
            last_cursor = frame["page"]["next_cursor"]
            assert frame["page"]["source_scope"] == source_scope
            for event in frame["page"]["items"]:
                assert event["generation"] == source_generation
                assert event["cause"]["kind"] in {"operation", "control"}
                assert "op" not in event
        else:
            boundary = frame
    assert event_frames == 16
    assert boundary == {
        "kind": "backpressure",
        "code": "event.catch-up-limit",
        "last_cursor": last_cursor,
        "recovery": "pull",
    }, boundary
    opcode, close_payload = websocket_frame(stream)
    assert opcode == 8
    assert struct.unpack("!H", close_payload[:2])[0] == 1013
    assert close_payload[2:] == b"resume with pull from last_cursor"
    stream.close()
    status, recovered = harness.call(
        "GET",
        f"/v1/events?cursor={last_cursor}&limit=100",
        "req_phase3_pull_recover",
    )
    assert status == 200, recovered
    assert recovered["result"]["generation"] == source_generation
    assert recovered["result"]["source_scope"] == source_scope
    assert recovered["result"]["items"]
    passed += 1

    status, created_snapshot = harness.call(
        "POST",
        "/v1/reconciliation-snapshots",
        "req_phase3_snapshot_create",
        b"{}",
    )
    assert status == 201, created_snapshot
    assert "operation" not in created_snapshot
    snapshot_id = created_snapshot["result"]["id"]
    through_seq = created_snapshot["result"]["through_seq"]
    assert created_snapshot["result"]["source_scope"] == source_scope
    assert created_snapshot["result"]["resume_cursor"] == (
        f"ev2.{source_scope}.{source_generation}.{through_seq}"
    )
    partitions = created_snapshot["result"]["partitions"]
    assert set(partitions) == {"workspaces", "execs", "provenance_events"}
    assert sum(partitions.values()) == created_snapshot["result"]["item_count"]
    history = created_snapshot["result"]["history"]
    assert history["item_count"] == partitions["provenance_events"]
    if history["item_count"]:
        assert history["first_seq"] <= history["through_seq"] < through_seq
    else:
        assert history["first_seq"] is None
        assert history["through_seq"] == 0
    passed += 1

    status, first_page = harness.call(
        "GET",
        f"/v1/reconciliation-snapshots/{snapshot_id}?limit=2",
        "req_phase3_snapshot_page_1",
    )
    assert status == 200, first_page
    assert first_page["result"]["through_seq"] == through_seq
    cursor = first_page["result"]["next_cursor"]

    status, late = harness.call(
        "POST",
        "/v1/workspaces",
        "req_phase3_late_create",
        mutation(
            "01JPHASE3LATECREATE0001",
            {"source": "empty", "labels": {"after": "snapshot"}},
        ),
    )
    assert status == 201, late
    late_workspace = late["result"]["id"]
    snapshot_ids = {item["id"] for item in first_page["result"]["items"]}
    for item in first_page["result"]["items"]:
        assert item["kind"] in {"workspace", "exec", "provenance-event"}
        if item["kind"] == "provenance-event":
            assert item["id"] == f"event:{item['value']['generation']}:{item['value']['seq']}"
    while cursor is not None:
        status, page = harness.call(
            "GET",
            f"/v1/reconciliation-snapshots/{snapshot_id}?cursor={cursor}&limit=2",
            f"req_phase3_snapshot_page_{len(snapshot_ids):03}",
        )
        assert status == 200, page
        assert page["result"]["through_seq"] == through_seq
        snapshot_ids.update(item["id"] for item in page["result"]["items"])
        cursor = page["result"]["next_cursor"]
    assert f"workspace:{late_workspace}" not in snapshot_ids
    assert len(snapshot_ids) == created_snapshot["result"]["item_count"]
    passed += 1

    lease_operation = "01JPHASE3LEASECREATE0001"
    status, leased = harness.call(
        "POST",
        "/v1/workspaces",
        "req_phase3_lease_create",
        mutation(
            lease_operation,
            {
                "source": "empty",
                "labels": {"lease": "cleanroom"},
                "lease_ttl_ms": 1000,
            },
        ),
    )
    assert status == 201, leased
    leased_workspace = leased["result"]["id"]
    assert leased["result"]["lease"]["state"] == "active"
    assert leased["result"]["lease"]["authorizing_operation"] == lease_operation
    status, renewed = harness.call(
        "POST",
        f"/v1/workspaces/{leased_workspace}/lease/renew",
        "req_phase3_lease_renew",
        mutation("01JPHASE3LEASERENEW0001", {"ttl_ms": 1000}),
    )
    assert status == 200, renewed
    assert renewed["result"]["lease"]["ttl_ms"] == 1000
    assert renewed["result"]["lease"]["authorizing_operation"] == "01JPHASE3LEASERENEW0001"
    passed += 1

    wait_absent(harness.workspaces / leased_workspace)
    status, replayed_renewal = harness.call(
        "POST",
        f"/v1/workspaces/{leased_workspace}/lease/renew",
        "req_phase3_lease_renew_replay",
        mutation("01JPHASE3LEASERENEW0001", {"ttl_ms": 1000}),
    )
    assert status == 200, replayed_renewal
    assert replayed_renewal["result"] == renewed["result"]
    expect_error(
        harness.call(
            "POST",
            f"/v1/workspaces/{leased_workspace}/lease/renew",
            "req_phase3_lease_renew_conflict",
            mutation("01JPHASE3LEASERENEW0001", {"ttl_ms": 2000}),
        ),
        409,
        "operation.request-conflict",
    )
    expect_error(
        harness.call(
            "GET",
            f"/v1/workspaces/{leased_workspace}",
            "req_phase3_lease_expired",
        ),
        404,
        "resource.not-found",
    )
    passed += 1

    if cgroup_root is None:
        expect_error(
            harness.call(
                "POST",
                "/v1/execs/ex_missing/lease/renew",
                "req_phase3_exec_lease_absent",
                mutation("01JPHASE3EXECLEASEROUTE1", {"ttl_ms": 1000}),
            ),
            404,
            "resource.not-found",
        )
    else:
        leased_input = exec_input(
            workspace,
            snapshot,
            ["/usr/bin/sleep", "60"],
            wait=False,
        )
        leased_input["lease_ttl_ms"] = 1000
        status, started = harness.call(
            "POST",
            "/v1/execs",
            "req_phase3_exec_lease_start",
            mutation("01JPHASE3EXECLEASESTART1", leased_input),
        )
        assert status == 202, started
        exec_id = started["result"]["id"]
        status, renewed_exec = harness.call(
            "POST",
            f"/v1/execs/{exec_id}/lease/renew",
            "req_phase3_exec_lease_renew",
            mutation("01JPHASE3EXECLEASERENEW1", {"ttl_ms": 1000}),
        )
        assert status == 200, renewed_exec
        assert renewed_exec["result"]["lease"]["authorizing_operation"] == "01JPHASE3EXECLEASERENEW1"
        wait_absent(cgroup_root / started["result"]["applied"]["cgroup"])
        status, replayed_exec_renewal = harness.call(
            "POST",
            f"/v1/execs/{exec_id}/lease/renew",
            "req_phase3_exec_lease_replay",
            mutation("01JPHASE3EXECLEASERENEW1", {"ttl_ms": 1000}),
        )
        assert status == 200, replayed_exec_renewal
        assert replayed_exec_renewal["result"] == renewed_exec["result"]
        expect_error(
            harness.call(
                "POST",
                f"/v1/execs/{exec_id}/lease/renew",
                "req_phase3_exec_lease_conflict",
                mutation("01JPHASE3EXECLEASERENEW1", {"ttl_ms": 2000}),
            ),
            409,
            "operation.request-conflict",
        )
        status, expired_exec = harness.call(
            "GET", f"/v1/execs/{exec_id}", "req_phase3_exec_lease_expired"
        )
        assert status == 200, expired_exec
        assert expired_exec["result"]["state"] == "expired"
        retire_operation = "01JPHASE3EXECRETIRE00001"
        status, retired_exec = harness.call(
            "DELETE",
            f"/v1/execs/{exec_id}",
            "req_phase3_exec_retire",
            mutation(retire_operation, {}),
        )
        assert status == 200, retired_exec
        assert retired_exec["result"] == {
            "absent": True,
            "id": exec_id,
            "kind": "exec",
            "observed_at": retired_exec["result"]["observed_at"],
        }
        status, retired_replay = harness.call(
            "DELETE",
            f"/v1/execs/{exec_id}",
            "req_phase3_exec_retire_replay",
            mutation(retire_operation, {}),
        )
        assert status == 200, retired_replay
        assert retired_replay["result"] == retired_exec["result"]
        expect_error(
            harness.call(
                "GET", f"/v1/execs/{exec_id}", "req_phase3_exec_retired_absent"
            ),
            404,
            "resource.not-found",
        )
    passed += 1

    for index, item in enumerate(noise + [late_workspace]):
        status, removed = harness.call(
            "DELETE",
            f"/v1/workspaces/{item}",
            f"req_phase3_noise_destroy_{index:02}",
            mutation(f"01JPHASE3NOISEDESTROY{index:02}", {}),
        )
        assert status == 200, removed
    expect_error(
        harness.call(
            "GET",
            f"/v1/events?cursor={start_cursor}&limit=10",
            "req_phase3_retention_gap",
        ),
        409,
        "event.retention-gap",
    )
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

    maximum_content = b"m" * 1_048_576
    status, maximum_written = harness.call(
        "PUT",
        f"/v1/workspaces/{workspace}/files/maximum.bin",
        "req_clean_maximum_write",
        mutation(
            "01JPHASE3MAXIMUMWRITE01",
            {
                "content": {
                    "encoding": "base64",
                    "data": base64.b64encode(maximum_content).decode("ascii"),
                }
            },
        ),
    )
    assert status == 200, maximum_written
    assert maximum_written["result"]["size"] == 1_048_576
    passed += 1

    deep_path = "/".join(["d"] * 65)
    expect_error(
        harness.call(
            "DELETE",
            f"/v1/workspaces/{workspace}/files/{deep_path}",
            "req_clean_path_depth",
            mutation("01JPHASE3PATHDEPTH0001", {}),
        ),
        422,
        "workspace.path-depth",
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

    float_operation = "01JPHASE3FLOATREFUSAL01"
    float_input = {"source": "empty", "labels": {}, "priority": 1.5}
    status, float_refused = harness.call(
        "POST",
        "/v1/workspaces",
        "req_clean_float_refusal",
        mutation(float_operation, float_input),
    )
    assert status == 422, float_refused
    assert float_refused["error"]["code"] == "request.schema-invalid"
    assert float_refused["error"]["operation"] == float_operation
    status, float_replay = harness.call(
        "POST",
        "/v1/workspaces",
        "req_clean_float_replay",
        mutation(float_operation, float_input),
    )
    assert status == 422, float_replay
    assert float_replay["error"] == float_refused["error"]
    expect_error(
        harness.call(
            "POST",
            "/v1/workspaces",
            "req_clean_float_conflict",
            mutation(
                float_operation,
                {"source": "empty", "labels": {}, "priority": 2.5},
            ),
        ),
        409,
        "operation.request-conflict",
    )
    status, float_record = harness.call(
        "GET", f"/v1/ops/{float_operation}", "req_clean_float_record"
    )
    assert status == 200, float_record
    assert float_record["result"]["state"] == "refused"
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
            "POST", "/v1/workspaces", "req_clean_limit", b" " * 2_097_153
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

    passed += check_phase3_journey(harness, workspace, snapshot, cgroup_root)

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
    status, destroyed_replay = harness.call(
        "DELETE",
        f"/v1/workspaces/{workspace}",
        "req_clean_destroy_replay",
        mutation("01JPHASE2CLEANDESTROY1", {}),
    )
    assert status == 200, destroyed_replay
    assert destroyed_replay["result"] == destroyed["result"]
    passed += 1
    status, write_replay = harness.call(
        "PUT",
        file_path,
        "req_clean_write_replay",
        mutation(
            "01JPHASE2CLEANWRITE001",
            {"content": {"encoding": "base64", "data": "aGVsbG8="}},
        ),
    )
    assert status == 200, write_replay
    assert write_replay["result"]["sha256"] == (
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    )
    passed += 1
    return passed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary", type=Path, default=Path("target/debug/substrate-daemon")
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
            check_dual_daemon_refusal(harness)
            passed = check_http_journey(harness, arguments.cgroup_root)
        finally:
            harness.close()
    print(
        f"runtime clean-room: {passed} HTTP cases, startup refusal, "
        "and dual-daemon refusal passed"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
