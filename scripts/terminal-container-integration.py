#!/usr/bin/env python3
# ruff: noqa: EM102, PLR0915, S101, S104, S310, S603, TRY003
from __future__ import annotations

import contextlib
import fcntl
import json
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import time
import traceback
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
IMAGE = os.environ.get(
    "AGENTSPACE_TERMINAL_INTEGRATION_IMAGE", "agentspace-kernel-kernel:latest"
)
SESSION_ID = "12345678-1234-5678-9234-567812345678"
COPILOT_SESSION_ID = "87654321-4321-4765-a321-876543210000"


def command(
    runtime: str, *args: str, check: bool = True
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [runtime, *args],
        check=check,
        cwd=ROOT,
        text=True,
        capture_output=True,
    )


def log(message: str, *, error: bool = False) -> None:
    stream = sys.stderr if error else sys.stdout
    stream.write(f"{message}\n")
    stream.flush()


def select_runtime() -> tuple[str | None, list[str]]:
    requested = os.environ.get("CONTAINER_RUNTIME")
    candidates = [requested] if requested else ["podman", "docker"]
    failures: list[str] = []
    for candidate in candidates:
        executable = shutil.which(candidate)
        if executable is None or not os.access(executable, os.X_OK):
            failures.append(f"{candidate}: executable not available")
            continue
        try:
            result = command(candidate, "info", check=False)
        except (OSError, subprocess.SubprocessError) as error:
            failures.append(f"{candidate}: {error}")
            continue
        if result.returncode == 0:
            return candidate, failures
        detail = result.stderr.strip().splitlines()
        failures.append(
            f"{candidate}: {detail[0] if detail else f'exit {result.returncode}'}"
        )
    return None, failures


def ensure_image(runtime: str) -> None:
    if os.environ.get("AGENTSPACE_TERMINAL_INTEGRATION_SKIP_BUILD") == "1":
        if command(runtime, "image", "inspect", IMAGE, check=False).returncode != 0:
            raise RuntimeError(
                f"{IMAGE} is missing and integration image build was disabled"
            )
        return
    log(f"building {IMAGE} for terminal container integration")
    result = subprocess.run(
        [
            runtime,
            "build",
            "--file",
            "kernels/kernel_host/Dockerfile",
            "--tag",
            IMAGE,
            ".",
        ],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"failed to build {IMAGE}")


def set_window_size(fd: int, cols: int, rows: int) -> None:
    fcntl.ioctl(fd, 0x5414, struct.pack("HHHH", rows, cols, 0, 0))


class PtyAttachment:
    def __init__(
        self,
        runtime: str,
        container: str,
        attach_argv: list[str],
        *,
        cols: int,
        rows: int,
    ) -> None:
        master, slave = pty.openpty()
        set_window_size(slave, cols, rows)
        self.master = master
        self.buffer = bytearray()
        self.process = subprocess.Popen(
            [runtime, "exec", "-it", container, *attach_argv],
            cwd=ROOT,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            close_fds=True,
            start_new_session=True,
        )
        os.close(slave)
        os.set_blocking(master, False)

    def read_until(self, expected: bytes, timeout: float = 10.0) -> bytes:
        deadline = time.monotonic() + timeout
        while expected not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError(
                    f"PTY output did not contain {expected!r}: {bytes(self.buffer)!r}"
                )
            readable, _, _ = select.select([self.master], [], [], remaining)
            if not readable:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except BlockingIOError:
                continue
            if not chunk:
                break
            self.buffer.extend(chunk)
        return bytes(self.buffer)

    def clear(self) -> None:
        self.buffer.clear()
        while True:
            readable, _, _ = select.select([self.master], [], [], 0)
            if not readable:
                return
            try:
                chunk = os.read(self.master, 65536)
            except BlockingIOError:
                return
            if not chunk:
                return

    def send(self, data: str) -> None:
        os.write(self.master, data.encode())

    def resize(self, cols: int, rows: int) -> None:
        set_window_size(self.master, cols, rows)
        os.killpg(self.process.pid, signal.SIGWINCH)

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        os.close(self.master)


class Integration:
    def __init__(self, runtime: str) -> None:
        suffix = uuid.uuid4().hex[:12]
        self.runtime = runtime
        self.label = f"terminal-integration-{suffix}"
        self.container = f"agentspace-terminal-it-{suffix}"
        self.workspace_volume = f"agentspace-terminal-it-workspace-{suffix}"
        self.copilot_volume = f"agentspace-terminal-it-copilot-{suffix}"
        self.base_url = ""
        self.attachments: list[PtyAttachment] = []

    def create_volume(self, name: str, role: str) -> None:
        command(
            self.runtime,
            "volume",
            "create",
            "--label",
            f"agentspace.test={self.label}",
            "--label",
            "agentspace.managed=true",
            "--label",
            f"agentspace.role={role}",
            name,
        )

    def start_container(self, *, recovery: bool) -> None:
        fake = ROOT / "scripts/fixtures/fake-copilot"
        args = [
            "run",
            "--detach",
            "--name",
            self.container,
            "--label",
            f"agentspace.test={self.label}",
            "--label",
            "agentspace.managed=true",
            "--label",
            "agentspace.role=terminal-integration",
            "--security-opt",
            "label=disable",
            "--publish",
            "127.0.0.1::8000",
            "--env",
            "KERNEL_HARNESS=copilot-cli",
            "--env",
            f"AGENTSPACE_SESSION_ID={SESSION_ID}",
            "--env",
            f"KERNEL_SESSION_ID={COPILOT_SESSION_ID}",
            "--env",
            "KERNEL_VSCODE_ENABLED=0",
            "--env",
            "COPILOT_CONFIG_DIR=/root/.copilot",
            "--env",
            "PATH=/fixture:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin",
            "--volume",
            f"{self.workspace_volume}:/workspace",
            "--volume",
            f"{self.copilot_volume}:/root/.copilot",
            "--volume",
            f"{fake}:/fixture/copilot:ro",
            "--entrypoint",
            "uv",
        ]
        if recovery:
            args.extend(["--env", "KERNEL_TERMINAL_RESUME=1"])
        args.extend(
            [
                IMAGE,
                "run",
                "--no-dev",
                "--package",
                "kernel-host",
                "-m",
                "uvicorn",
                "kernel_host.app:app",
                "--host",
                "0.0.0.0",
                "--port",
                "8000",
            ]
        )
        command(self.runtime, *args)
        port = command(self.runtime, "port", self.container, "8000/tcp").stdout.strip()
        host_port = port.rsplit(":", 1)[-1]
        self.base_url = f"http://127.0.0.1:{host_port}"
        self.wait_ready()

    def wait_ready(self) -> None:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            try:
                if self.request("GET", "/healthz") == {"status": "ok"}:
                    return
            except (OSError, urllib.error.URLError, json.JSONDecodeError):
                time.sleep(0.5)
        logs = command(self.runtime, "logs", self.container, check=False)
        raise RuntimeError(
            f"kernel container did not become ready:\n{logs.stdout}\n{logs.stderr}"
        )

    def request(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        data = None
        headers: dict[str, str] = {}
        if payload is not None:
            data = json.dumps(payload).encode()
            headers["content-type"] = "application/json"
        elif method == "POST":
            data = b""
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=data,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace")
            raise RuntimeError(
                f"{method} {path} returned HTTP {error.code}: {detail}"
            ) from error

    def status(self) -> dict[str, Any]:
        return self.request("GET", "/terminal")

    def wait_status(
        self,
        *,
        state: str | None = None,
        attachments: int | None = None,
        timeout: float = 10,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        last: dict[str, Any] = {}
        while time.monotonic() < deadline:
            last = self.status()
            if (state is None or last["state"] == state) and (
                attachments is None or last["attachment_count"] == attachments
            ):
                return last
            time.sleep(0.1)
        raise AssertionError(f"terminal status did not converge: {last}")

    def attach(self, argv: list[str], *, cols: int, rows: int) -> PtyAttachment:
        attachment = PtyAttachment(
            self.runtime,
            self.container,
            argv,
            cols=cols,
            rows=rows,
        )
        self.attachments.append(attachment)
        attachment.read_until(b"FAKE_COPILOT_READY")
        return attachment

    def close_attachment(self, attachment: PtyAttachment) -> None:
        attachment.close()
        self.attachments.remove(attachment)

    def detach_client(self, client_id: str) -> None:
        self.request(
            "POST",
            "/terminal/detach-client",
            {"tmux_client_id": client_id},
        )

    def remove_container(self) -> None:
        command(self.runtime, "rm", "--force", self.container, check=False)

    def durable_state(self) -> dict[str, Any]:
        result = command(
            self.runtime,
            "exec",
            self.container,
            "cat",
            "/root/.copilot/terminal-integration-state.json",
        )
        return json.loads(result.stdout)

    def wait_durable_state(
        self, invocations: int, timeout: float = 10
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        last: dict[str, Any] = {}
        while time.monotonic() < deadline:
            last = self.durable_state()
            if last.get("invocations") == invocations:
                return last
            time.sleep(0.1)
        raise AssertionError(
            f"durable state did not reach invocation {invocations}: {last}"
        )

    def cleanup(self) -> None:
        for attachment in list(self.attachments):
            with contextlib.suppress(OSError):
                self.close_attachment(attachment)
        self.remove_container()
        for volume in (self.workspace_volume, self.copilot_volume):
            inspected = command(
                self.runtime,
                "volume",
                "inspect",
                "--format",
                '{{ index .Labels "agentspace.test" }}',
                volume,
                check=False,
            )
            if inspected.returncode == 0 and inspected.stdout.strip() == self.label:
                command(self.runtime, "volume", "rm", volume, check=False)

    def run(self) -> None:
        self.create_volume(self.workspace_volume, "terminal-integration-workspace")
        self.create_volume(self.copilot_volume, "terminal-integration-copilot-state")
        self.start_container(recovery=False)
        command(
            self.runtime,
            "exec",
            self.container,
            "sh",
            "-c",
            "printf stable-workspace > /workspace/integration-marker",
        )

        first = self.request("POST", "/terminal/ensure")
        assert first["attach_kind"] == "started", first
        duplicate = self.request("POST", "/terminal/ensure")
        assert duplicate["attach_kind"] == "attached", duplicate
        attach_argv = list(first["attach_argv"])

        first_client = self.attach(attach_argv, cols=100, rows=30)
        second_client = self.attach(attach_argv, cols=80, rows=24)
        self.wait_status(state="running", attachments=2)

        first_client.clear()
        second_client.clear()
        first_client.send("shared-input\n")
        first_client.read_until(b"ECHO shared-input")
        second_client.read_until(b"ECHO shared-input")

        first_client.resize(120, 40)
        sized = self.wait_status(attachments=2)
        dimensions = {
            (client["width"], client["height"]) for client in sized["clients"]
        }
        assert (120, 40) in dimensions, dimensions
        assert (80, 24) in dimensions, dimensions

        self.detach_client(
            next(client["id"] for client in sized["clients"] if client["width"] == 120)
        )
        self.close_attachment(first_client)
        self.wait_status(state="running", attachments=1)
        remaining = self.status()["clients"][0]["id"]
        self.detach_client(remaining)
        self.close_attachment(second_client)
        self.wait_status(state="running", attachments=0)

        reattached = self.attach(attach_argv, cols=90, rows=28)
        reattached.send("state\n")
        expected_state = (
            f"STATE session={COPILOT_SESSION_ID} invocation=1 "
            "workspace=stable-workspace"
        )
        reattached.read_until(expected_state.encode())
        reattached.send("exit 7\n")
        exited = self.wait_status(state="exited")
        assert exited["exit_status"] == 7, exited
        self.detach_client(exited["clients"][0]["id"])
        self.close_attachment(reattached)

        resumed = self.request("POST", "/terminal/resume")
        assert resumed["attach_kind"] == "resumed", resumed
        assert self.wait_durable_state(2) == {
            "invocations": 2,
            "session_id": COPILOT_SESSION_ID,
        }

        resumed_client = self.attach(list(resumed["attach_argv"]), cols=96, rows=32)
        resumed_client.send("state\n")
        expected_state = (
            f"STATE session={COPILOT_SESSION_ID} invocation=2 "
            "workspace=stable-workspace"
        )
        resumed_client.read_until(expected_state.encode())
        resumed_client_id = self.status()["clients"][0]["id"]
        self.detach_client(resumed_client_id)
        self.close_attachment(resumed_client)
        self.wait_status(state="running", attachments=0)

        self.remove_container()
        self.start_container(recovery=True)
        recovered = self.request("POST", "/terminal/ensure")
        assert recovered["attach_kind"] == "resumed", recovered
        assert self.wait_durable_state(3) == {
            "invocations": 3,
            "session_id": COPILOT_SESSION_ID,
        }
        recovered_client = self.attach(
            list(recovered["attach_argv"]), cols=100, rows=30
        )
        recovered_client.send("state\n")
        expected_state = (
            f"STATE session={COPILOT_SESSION_ID} invocation=3 "
            "workspace=stable-workspace"
        )
        recovered_client.read_until(expected_state.encode())
        recovered_client_id = self.status()["clients"][0]["id"]
        self.detach_client(recovered_client_id)
        self.close_attachment(recovered_client)
        self.wait_status(state="running", attachments=0)


def main() -> int:
    runtime, failures = select_runtime()
    if runtime is None:
        condition = "; ".join(failures) if failures else "no runtime candidates"
        log(f"SKIP terminal container integration: no compatible daemon ({condition})")
        return 0
    ensure_image(runtime)
    integration = Integration(runtime)
    try:
        integration.run()
    finally:
        integration.cleanup()
    log(f"terminal container integration passed with {runtime}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, RuntimeError, subprocess.CalledProcessError) as error:
        log(f"terminal container integration failed: {error}", error=True)
        traceback.print_exc()
        raise SystemExit(1) from error
