from __future__ import annotations

import asyncio
import errno
import fcntl
import hmac
import logging
import os
import pathlib
import pty
import shutil
import signal
import struct
import termios
import uuid
from contextlib import suppress
from dataclasses import dataclass

MIN_TERMINAL_COLS = 1
MIN_TERMINAL_ROWS = 1
MAX_TERMINAL_COLS = 500
MAX_TERMINAL_ROWS = 300
REAP_TIMEOUT_SECONDS = 1.0

logger = logging.getLogger(__name__)


def valid_terminal_size(cols: int, rows: int) -> bool:
    return (
        MIN_TERMINAL_COLS <= cols <= MAX_TERMINAL_COLS
        and MIN_TERMINAL_ROWS <= rows <= MAX_TERMINAL_ROWS
    )


def terminal_authorized(authorization: str | None, token: str | None) -> bool:
    if not token or not authorization:
        return False
    return hmac.compare_digest(authorization, f"Bearer {token}")


async def _wait_for_fd(fd: int, *, writable: bool) -> None:
    loop = asyncio.get_running_loop()
    ready = loop.create_future()

    def mark_ready() -> None:
        if not ready.done():
            ready.set_result(None)

    register = loop.add_writer if writable else loop.add_reader
    unregister = loop.remove_writer if writable else loop.remove_reader
    register(fd, mark_ready)
    try:
        await ready
    finally:
        unregister(fd)


def _set_size(fd: int, cols: int, rows: int) -> None:
    size = struct.pack("HHHH", rows, cols, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, size)


def _matching_process_groups(terminal_id: str) -> set[int]:
    needle = f"AGENTSPACE_TERMINAL_ID={terminal_id}".encode()
    groups: set[int] = set()
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            environ = (entry / "environ").read_bytes().split(b"\0")
            if needle in environ:
                groups.add(os.getpgid(int(entry.name)))
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
    return groups


def _signal_process_groups(
    terminal_id: str,
    original_process_group: int,
    signum: signal.Signals,
) -> None:
    groups = _matching_process_groups(terminal_id)
    groups.add(original_process_group)
    for group in groups:
        with suppress(ProcessLookupError):
            os.killpg(group, signum)


def _waitpid_nohang(pid: int) -> int:
    waited_pid, _status = os.waitpid(pid, os.WNOHANG)
    return waited_pid


@dataclass
class TerminalSession:
    master_fd: int
    pid: int
    terminal_id: str
    _closed: bool = False

    @classmethod
    async def open(
        cls,
        cols: int,
        rows: int,
        *,
        workdir: str,
    ) -> TerminalSession:
        if not valid_terminal_size(cols, rows):
            msg = "terminal size must be between 1x1 and 500x300"
            raise ValueError(msg)

        shell = shutil.which("bash") or shutil.which("sh")
        if shell is None:
            msg = "no supported shell is installed"
            raise RuntimeError(msg)

        terminal_id = uuid.uuid4().hex
        pid, master_fd = pty.fork()
        if pid == 0:
            environment = os.environ.copy()
            environment.pop("KERNEL_HOST_TERMINAL_TOKEN", None)
            environment.update(
                {
                    "TERM": "xterm-256color",
                    "COLORTERM": "truecolor",
                    "AGENTSPACE_TERMINAL_ID": terminal_id,
                }
            )
            try:
                os.chdir(workdir)
                os.execvpe(shell, [shell, "-l"], environment)  # noqa: S606
            except (OSError, ValueError):
                os._exit(127)

        os.set_blocking(master_fd, False)
        _set_size(master_fd, cols, rows)
        return cls(master_fd=master_fd, pid=pid, terminal_id=terminal_id)

    async def read(self, size: int = 64 * 1024) -> bytes:
        while not self._closed:
            try:
                return os.read(self.master_fd, size)
            except BlockingIOError:
                await _wait_for_fd(self.master_fd, writable=False)
            except OSError as error:
                if error.errno in {errno.EIO, errno.EBADF}:
                    return b""
                raise
        return b""

    async def write(self, data: bytes) -> None:
        view = memoryview(data)
        while view and not self._closed:
            try:
                written = os.write(self.master_fd, view)
                view = view[written:]
            except BlockingIOError:
                await _wait_for_fd(self.master_fd, writable=True)

    async def resize(self, cols: int, rows: int) -> None:
        if not valid_terminal_size(cols, rows):
            msg = "terminal size must be between 1x1 and 500x300"
            raise ValueError(msg)
        _set_size(self.master_fd, cols, rows)

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True

        for signum, delay in (
            (signal.SIGHUP, 0.25),
            (signal.SIGTERM, 0.5),
            (signal.SIGKILL, 0.0),
        ):
            _signal_process_groups(self.terminal_id, self.pid, signum)
            if delay:
                await asyncio.sleep(delay)

        with suppress(OSError):
            os.close(self.master_fd)
        if not await self._reap(REAP_TIMEOUT_SECONDS):
            logger.warning("terminal shell %s was not reaped before timeout", self.pid)

    async def _reap(self, max_wait_seconds: float) -> bool:
        deadline = asyncio.get_running_loop().time() + max_wait_seconds
        while True:
            try:
                waited_pid = _waitpid_nohang(self.pid)
            except ChildProcessError:
                return True
            if waited_pid == self.pid:
                return True
            if asyncio.get_running_loop().time() >= deadline:
                return False
            await asyncio.sleep(0.01)
