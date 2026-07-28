#!/usr/bin/env python3
"""Real-process integration checks for the Oikade Matter adapter RPC API."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
from typing import Any, Dict, Optional


API_VERSION = 1
MAX_FRAME_SIZE = 1 << 20
MAX_STDERR_SIZE = 256 << 10
SETUP_PASSCODE = "20202021"
DISCRIMINATOR = "3840"


class HarnessError(RuntimeError):
    """Raised when an adapter process violates the RPC contract."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise HarnessError(message)


class AdapterProcess:
    def __init__(
        self,
        binary: pathlib.Path,
        state_directory: pathlib.Path,
        timeout: float,
    ) -> None:
        require(state_directory.is_dir(), "state directory was not created")
        require(
            not any(state_directory.iterdir()),
            f"state directory is not fresh: {state_directory}",
        )
        self._timeout = timeout
        self._buffer = bytearray()
        self._stderr = bytearray()
        self._stderr_lock = threading.Lock()

        parent_socket, child_socket = socket.socketpair()
        self._socket = parent_socket
        self._socket.settimeout(timeout)
        child_fd = child_socket.fileno()

        environment = {
            "HOME": str(state_directory),
            "LANG": "C",
            "LC_ALL": "C",
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "TMPDIR": str(state_directory),
            "OIKADE_ADAPTER_RPC_FD": str(child_fd),
            "OIKADE_ADAPTER_STATE_DIR": str(state_directory),
            "OIKADE_MATTER_SETUP_PASSCODE": SETUP_PASSCODE,
            "OIKADE_MATTER_DISCRIMINATOR": DISCRIMINATOR,
        }
        try:
            self.process = subprocess.Popen(
                [str(binary), "--matter-log-level=none"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                env=environment,
                pass_fds=(child_fd,),
                close_fds=True,
            )
        except Exception:
            parent_socket.close()
            raise
        finally:
            child_socket.close()

        require(self.process.stderr is not None, "child stderr pipe is missing")
        self._stderr_thread = threading.Thread(
            target=self._drain_stderr,
            name="matter-adapter-stderr",
            daemon=True,
        )
        self._stderr_thread.start()

    def _drain_stderr(self) -> None:
        assert self.process.stderr is not None
        while True:
            chunk = self.process.stderr.read(8192)
            if not chunk:
                return
            with self._stderr_lock:
                self._stderr.extend(chunk)
                overflow = len(self._stderr) - MAX_STDERR_SIZE
                if overflow > 0:
                    del self._stderr[:overflow]

    def diagnostic_stderr(self) -> str:
        with self._stderr_lock:
            output = bytes(self._stderr).decode("utf-8", errors="replace")
        return output.replace(SETUP_PASSCODE, "<redacted-passcode>").replace(
            DISCRIMINATOR, "<redacted-discriminator>"
        )

    def send_frame(self, frame: Dict[str, Any]) -> None:
        encoded = json.dumps(frame, separators=(",", ":")).encode("utf-8") + b"\n"
        require(len(encoded) <= MAX_FRAME_SIZE, "test frame exceeds protocol limit")
        self._socket.sendall(encoded)

    def send_raw(self, encoded: bytes) -> None:
        self._socket.sendall(encoded)

    def read_frame(self, timeout: Optional[float] = None) -> Dict[str, Any]:
        deadline = time.monotonic() + (self._timeout if timeout is None else timeout)
        while b"\n" not in self._buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise HarnessError("timed out waiting for adapter RPC frame")
            self._socket.settimeout(remaining)
            try:
                chunk = self._socket.recv(65536)
            except socket.timeout as error:
                raise HarnessError("timed out waiting for adapter RPC frame") from error
            if not chunk:
                status = self.process.poll()
                raise HarnessError(
                    f"adapter RPC socket closed before a frame (status={status})"
                )
            self._buffer.extend(chunk)
            require(
                len(self._buffer) <= MAX_FRAME_SIZE,
                "adapter emitted an oversized RPC frame",
            )

        encoded, _, remaining = self._buffer.partition(b"\n")
        self._buffer = bytearray(remaining)
        try:
            decoded = json.loads(encoded)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise HarnessError(f"adapter emitted invalid JSON: {error}") from error
        require(isinstance(decoded, dict), "adapter frame is not a JSON object")
        return decoded

    def expect_hello(self) -> Dict[str, Any]:
        frame = self.read_frame()
        require(frame.get("version") == API_VERSION, "hello has wrong API version")
        require(frame.get("kind") == "notification", "hello is not a notification")
        require(frame.get("method") == "hello", "first frame is not hello")
        require("id" not in frame, "hello unexpectedly has a request ID")
        body = frame.get("body")
        require(isinstance(body, dict), "hello body is missing")
        require(body.get("adapter_id") == "oikade.matter", "wrong adapter ID")
        require(
            isinstance(body.get("adapter_version"), str)
            and bool(body["adapter_version"]),
            "adapter version is missing",
        )
        require(body.get("min_api_version") == API_VERSION, "wrong minimum API")
        require(body.get("max_api_version") == API_VERSION, "wrong maximum API")
        require(body.get("protocols") == ["matter"], "wrong protocol list")
        return body

    def request(self, request_id: int, method: str, body: Dict[str, Any]) -> Dict[str, Any]:
        self.send_frame(
            {
                "version": API_VERSION,
                "kind": "request",
                "id": request_id,
                "method": method,
                "body": body,
            }
        )
        response = self.read_frame()
        require(response.get("version") == API_VERSION, f"{method} response version")
        require(response.get("kind") == "response", f"{method} is not a response")
        require(response.get("id") == request_id, f"{method} response ID mismatch")
        require(response.get("method") == method, f"{method} response method mismatch")
        if "error" in response:
            raise HarnessError(f"{method} returned RPC error: {response['error']!r}")
        result = response.get("body")
        require(isinstance(result, dict), f"{method} response body is missing")
        return result

    def initialize(self, request_id: int = 1) -> None:
        body = self.request(
            request_id,
            "initialize",
            {"api_version": API_VERSION, "instance_id": "rpc-integration", "config": {}},
        )
        require(body.get("ready") is True, "adapter did not become ready")

    def sync_topology(self, request_id: int = 2) -> None:
        body = self.request(
            request_id,
            "sync",
            {
                "generation": 1,
                "revision": 1,
                "devices": [
                    {
                        "device": {
                            "id": "integration-switch",
                            "name": "Integration Switch",
                            "manufacturer": "Oikade",
                            "model": "RPC Harness",
                            "capabilities": [
                                {
                                    "id": "on",
                                    "type": "oikade.switch.on",
                                    "name": "On",
                                    "kind": "bool",
                                    "permissions": {
                                        "read": True,
                                        "write": True,
                                        "observe": True,
                                    },
                                }
                            ],
                        },
                        "values": [
                            {"capability_id": "on", "value": {"kind": "bool", "bool": False}}
                        ],
                    },
                    {
                        "device": {
                            "id": "integration-light",
                            "name": "Integration Light",
                            "manufacturer": "Oikade",
                            "model": "RPC Harness",
                            "capabilities": [
                                {
                                    "id": "on",
                                    "type": "oikade.light.on",
                                    "name": "On",
                                    "kind": "bool",
                                    "permissions": {
                                        "read": True,
                                        "write": True,
                                        "observe": True,
                                    },
                                },
                                {
                                    "id": "level",
                                    "type": "oikade.light.level",
                                    "name": "Level",
                                    "kind": "number",
                                    "permissions": {
                                        "read": True,
                                        "write": True,
                                        "observe": True,
                                    },
                                },
                            ],
                        },
                        "values": [
                            {"capability_id": "on", "value": {"kind": "bool", "bool": False}},
                            {
                                "capability_id": "level",
                                "value": {"kind": "number", "number": 50.0},
                            },
                        ],
                    },
                ],
            },
        )
        require(body.get("generation") == 1, "sync generation mismatch")
        require(body.get("devices") == 2, "sync device count mismatch")
        require(
            body.get("projections")
            == [
                {"device_id": "integration-switch", "capability_id": "on"},
                {"device_id": "integration-light", "capability_id": "on"},
                {"device_id": "integration-light", "capability_id": "level"},
            ],
            f"unexpected sync projections: {body.get('projections')!r}",
        )
        require(body.get("diagnostics") == [], "sync returned diagnostics")

    def health(self, request_id: int) -> None:
        body = self.request(request_id, "health", {})
        require(body.get("healthy") is True, "adapter health is false")
        require(isinstance(body.get("resources"), list), "health resources missing")

    def event(
        self,
        request_id: int,
        device_id: str,
        capability_id: str,
        value: Dict[str, Any],
        revision: int,
    ) -> None:
        body = self.request(
            request_id,
            "event",
            {
                "device_id": device_id,
                "capability_id": capability_id,
                "value": value,
                "revision": revision,
                "occurred_at": "2026-01-01T00:00:00Z",
            },
        )
        require(body == {}, "event response must be empty")

    def shutdown(self, request_id: int) -> None:
        body = self.request(request_id, "shutdown", {})
        require(body == {}, "shutdown response must be empty")
        status = self.wait()
        require(status == 0, f"graceful shutdown returned status {status}")

    def wait(self, timeout: Optional[float] = None) -> int:
        try:
            status = self.process.wait(timeout=self._timeout if timeout is None else timeout)
        except subprocess.TimeoutExpired as error:
            raise HarnessError("adapter did not exit within the bounded timeout") from error
        self._stderr_thread.join(timeout=1)
        return status

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=self._timeout)
        self._stderr_thread.join(timeout=1)

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=self._timeout)
        self._socket.close()
        # Let the reader observe EOF before closing its file object. Closing the
        # buffered stream underneath an active read is racy on Python 3.14.
        self._stderr_thread.join(timeout=1)
        if self.process.stderr is not None:
            self.process.stderr.close()


def launch(
    binary: pathlib.Path, root: pathlib.Path, case: str, timeout: float
) -> AdapterProcess:
    state_directory = root / case
    state_directory.mkdir(mode=0o700)
    return AdapterProcess(binary, state_directory, timeout)


def run_happy_path(binary: pathlib.Path, root: pathlib.Path, timeout: float) -> None:
    adapter = launch(binary, root, "happy", timeout)
    try:
        adapter.expect_hello()
        adapter.initialize()
        adapter.sync_topology()
        adapter.health(3)
        adapter.event(4, "integration-switch", "on", {"kind": "bool", "bool": True}, 2)
        adapter.event(
            5,
            "integration-light",
            "level",
            {"kind": "number", "number": 72.5},
            3,
        )
        adapter.health(6)
        adapter.shutdown(7)
    except Exception as error:
        raise HarnessError(f"happy-path failure: {error}\n{adapter.diagnostic_stderr()}") from error
    finally:
        adapter.close()
    print("PASS real-process hello/init/sync/health/event/shutdown")


def run_malformed_frame(binary: pathlib.Path, root: pathlib.Path, timeout: float) -> None:
    adapter = launch(binary, root, "malformed", timeout)
    try:
        adapter.expect_hello()
        adapter.send_raw(
            b'{"version":1,"version":2,"kind":"request","id":1,'
            b'"method":"health","body":{}}\n'
        )
        status = adapter.wait()
        require(isinstance(status, int), "malformed-frame child did not terminate")
    except Exception as error:
        raise HarnessError(
            f"malformed-frame failure: {error}\n{adapter.diagnostic_stderr()}"
        ) from error
    finally:
        adapter.close()
    print("PASS duplicate-key malformed frame is rejected and bounded")


def run_relaunch(binary: pathlib.Path, root: pathlib.Path, timeout: float) -> None:
    crashed = launch(binary, root, "crashed-child", timeout)
    try:
        crashed.expect_hello()
        crashed.initialize()
        crashed.sync_topology()
        crashed.kill()
        require(crashed.process.returncode != 0, "forced child termination returned success")
    except Exception as error:
        raise HarnessError(f"crash setup failure: {error}\n{crashed.diagnostic_stderr()}") from error
    finally:
        crashed.close()

    replacement = launch(binary, root, "replacement-child", timeout)
    try:
        replacement.expect_hello()
        replacement.initialize()
        replacement.sync_topology()
        replacement.health(3)
        replacement.shutdown(4)
    except Exception as error:
        raise HarnessError(
            f"replacement child failure: {error}\n{replacement.diagnostic_stderr()}"
        ) from error
    finally:
        replacement.close()
    print("PASS killed child can be relaunched with independent fresh state")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=pathlib.Path)
    parser.add_argument("--timeout", type=float, default=30.0)
    arguments = parser.parse_args()

    binary = arguments.binary.resolve()
    require(binary.is_file(), f"adapter binary does not exist: {binary}")
    require(os.access(binary, os.X_OK), f"adapter binary is not executable: {binary}")
    require(arguments.timeout > 0, "timeout must be positive")

    with tempfile.TemporaryDirectory(prefix="oikade-matter-rpc-") as temporary:
        root = pathlib.Path(temporary)
        run_happy_path(binary, root, arguments.timeout)
        run_malformed_frame(binary, root, arguments.timeout)
        run_relaunch(binary, root, arguments.timeout)
    print("PASS all adapter state was disposable")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (HarnessError, OSError, subprocess.SubprocessError) as error:
        raise SystemExit(f"FAIL {error}") from error
