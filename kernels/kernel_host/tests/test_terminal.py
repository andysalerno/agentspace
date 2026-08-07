from __future__ import annotations

import asyncio
import fcntl
import os
import struct
import termios
from typing import TYPE_CHECKING, ClassVar, Self

import pytest
from fastapi import FastAPI
from kernel_host import app as app_module
from kernel_host.terminal import (
    TerminalSession,
    terminal_authorized,
    valid_terminal_size,
)
from starlette.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

if TYPE_CHECKING:
    from pathlib import Path


class FakeTerminalSession:
    instances: ClassVar[list[FakeTerminalSession]] = []

    def __init__(self, cols: int, rows: int, workdir: str) -> None:
        self.cols = cols
        self.rows = rows
        self.workdir = workdir
        self.input = bytearray()
        self.closed = False
        self._sent_output = False
        self.instances.append(self)

    @classmethod
    async def open(
        cls,
        cols: int,
        rows: int,
        *,
        workdir: str,
    ) -> Self:
        return cls(cols, rows, workdir)

    async def read(self) -> bytes:
        if not self._sent_output:
            self._sent_output = True
            return b"terminal ready"
        await asyncio.Future[None]()
        return b""

    async def write(self, data: bytes) -> None:
        self.input.extend(data)

    async def resize(self, cols: int, rows: int) -> None:
        self.cols = cols
        self.rows = rows

    async def close(self) -> None:
        self.closed = True


async def _read_until(session: TerminalSession, marker: bytes) -> bytes:
    output = bytearray()
    while marker not in output:
        chunk = await session.read()
        if not chunk:
            break
        output.extend(chunk)
    return bytes(output)


def test_terminal_authentication_and_dimensions() -> None:
    assert terminal_authorized("Bearer secret", "secret")
    assert not terminal_authorized("Bearer wrong", "secret")
    assert not terminal_authorized(None, "secret")
    assert not terminal_authorized("Bearer secret", None)
    assert valid_terminal_size(80, 24)
    assert not valid_terminal_size(0, 24)
    assert not valid_terminal_size(501, 24)


def test_terminal_websocket_requires_token_and_proxies_io(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    FakeTerminalSession.instances.clear()
    monkeypatch.setenv("KERNEL_HOST_TERMINAL_TOKEN", "secret")
    monkeypatch.setattr(app_module, "TerminalSession", FakeTerminalSession)
    test_app = FastAPI()
    test_app.websocket("/terminal")(app_module.terminal)
    client = TestClient(test_app)

    with (
        pytest.raises(WebSocketDisconnect),
        client.websocket_connect("/terminal?cols=80&rows=24"),
    ):
        pass

    with client.websocket_connect(
        "/terminal?cols=80&rows=24",
        headers={"authorization": "Bearer secret"},
    ) as websocket:
        assert websocket.receive_bytes() == b"terminal ready"
        websocket.send_bytes(b"pwd\r")
        websocket.send_text('{"type":"resize","cols":132,"rows":48}')

    session = FakeTerminalSession.instances[0]
    assert session.input == b"pwd\r"
    assert (session.cols, session.rows) == (132, 48)
    assert session.workdir == "/workspace"
    assert session.closed


@pytest.mark.asyncio
async def test_terminal_session_runs_in_workspace_and_resizes(
    tmp_path: Path,
) -> None:
    session = await TerminalSession.open(80, 24, workdir=str(tmp_path))
    try:
        await session.write(b"stty -echo\r")
        await asyncio.sleep(0.05)
        await session.write(b"pwd\r")
        output = await asyncio.wait_for(
            _read_until(session, f"{tmp_path}\r\n".encode()),
            timeout=2.0,
        )
        assert f"{tmp_path}\r\n".encode() in output

        await session.resize(132, 48)
        size = fcntl.ioctl(session.master_fd, termios.TIOCGWINSZ, bytes(8))
        rows, cols, _x_pixels, _y_pixels = struct.unpack("HHHH", size)
        assert (cols, rows) == (132, 48)
    finally:
        pid = session.pid
        await session.close()

    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
