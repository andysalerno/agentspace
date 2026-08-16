from __future__ import annotations

import asyncio
import logging
import os
import tempfile
from contextlib import suppress
from pathlib import Path
from time import perf_counter
from typing import TYPE_CHECKING, Any

from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig

from kernel_host.registry import HarnessName, get_kernel

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.protocol import Kernel

logger = logging.getLogger(__name__)


class KernelSessionService:
    def __init__(
        self,
        *,
        harness: HarnessName,
        env: dict[str, str],
        additional_paths: tuple[str, ...],
        session_id: str | None = None,
    ) -> None:
        self._harness = harness
        self._base_env = dict(env)
        self._additional_paths = additional_paths
        self._initial_session_id = session_id
        self._session_id = session_id
        self._history: list[list[KernelEvent]] = []
        self._log_path = Path(tempfile.mkdtemp()) / "kernel.log"
        self._log_path.touch()
        self._status = KernelStatus.IDLE
        self._kernel: Kernel | None = None
        self._kernel_started = False
        self._stream_lock = asyncio.Lock()
        self._raw_log_offset = 0

    async def send_message(self, message: str) -> list[KernelEvent]:
        return [event async for event in self.stream_message(message)]

    def stream_message(self, message: str) -> AsyncIterator[KernelEvent]:
        events: list[KernelEvent] = []

        async def iterator() -> AsyncIterator[KernelEvent]:
            started_at = perf_counter()
            send_task: asyncio.Task[None] | None = None
            kernel: Kernel | None = None
            stream_completed = False
            logger.info(
                "kernel_host waiting for lock: message_chars=%d",
                len(message),
            )
            async with self._stream_lock:
                logger.info(
                    "kernel_host stream lock acquired: wait_ms=%.1f",
                    (perf_counter() - started_at) * 1000,
                )
                try:
                    kernel = await self._kernel_for_message()
                    logger.info(
                        "kernel_host kernel ready: elapsed_ms=%.1f kernel=%s",
                        (perf_counter() - started_at) * 1000,
                        kernel.name,
                    )
                    send_task = asyncio.create_task(kernel.send(message))
                    async for event in kernel.recv():
                        if not events:
                            logger.info(
                                "kernel_host first event: elapsed_ms=%.1f type=%s",
                                (perf_counter() - started_at) * 1000,
                                event.type,
                            )
                        events.append(event)
                        yield event
                    stream_completed = True
                    await send_task
                finally:
                    if send_task is not None:
                        with suppress(asyncio.CancelledError):
                            await send_task
                    if kernel is not None:
                        if not stream_completed and not self._should_stop_after_message(
                            kernel,
                        ):
                            events.extend([event async for event in kernel.recv()])
                        if kernel.resume_token is not None:
                            self._session_id = kernel.resume_token
                        self._history.append(list(events))
                        self._append_raw_logs(kernel)
                        self._status = self._derive_status(events, kernel.status)
                        if (
                            self._should_stop_after_message(kernel)
                            or kernel.status == KernelStatus.ERROR
                        ):
                            await self._stop_kernel()
                    logger.info(
                        "kernel_host stream final: elapsed_ms=%.1f events=%d status=%s",
                        (perf_counter() - started_at) * 1000,
                        len(events),
                        self._status,
                    )

        return iterator()

    async def _kernel_for_message(self) -> Kernel:
        if self._kernel is None:
            logger.info("kernel_host creating kernel: harness=%s", self._harness.value)
            self._kernel = get_kernel(self._harness)
            self._kernel_started = False
            self._raw_log_offset = 0
        if not self._kernel_started:
            started_at = perf_counter()
            config = KernelConfig(
                env=dict(self._base_env),
                session_id=self._session_id,
                additional_paths=self._additional_paths,
            )
            try:
                await self._kernel.start(config)
            except asyncio.CancelledError:
                await self._stop_kernel()
                raise
            except Exception:
                await self._stop_kernel()
                raise
            self._kernel_started = True
            logger.info(
                "kernel_host kernel started: harness=%s elapsed_ms=%.1f",
                self._harness.value,
                (perf_counter() - started_at) * 1000,
            )
        return self._kernel

    def _should_stop_after_message(self, kernel: Kernel) -> bool:
        return not bool(getattr(kernel, "supports_persistent_process", False))

    def _append_raw_logs(self, kernel: Kernel) -> None:
        raw_logs: list[str] = getattr(kernel, "raw_logs", [])
        if not raw_logs:
            return
        new_logs = raw_logs[self._raw_log_offset :]
        self._raw_log_offset = len(raw_logs)
        if new_logs:
            with self._log_path.open("a", encoding="utf-8") as f:
                f.writelines(line + "\n" for line in new_logs)

    async def _stop_kernel(self) -> None:
        kernel = self._kernel
        self._kernel = None
        self._kernel_started = False
        self._raw_log_offset = 0
        if kernel is not None:
            await kernel.stop()

    async def summary(self) -> dict[str, Any]:
        return {
            "harness": self._harness,
            "status": self._status,
            "turns": len(self._history),
            "resume_token": self._session_id,
            "additional_paths": list(self._additional_paths),
            "vscode_url": os.environ.get("KERNEL_VSCODE_URL") or None,
        }

    async def history(self) -> list[list[KernelEvent]]:
        return list(self._history)

    async def logs(self) -> list[str]:
        try:
            return self._log_path.read_text(encoding="utf-8").splitlines()
        except FileNotFoundError:
            return []

    async def reset(self) -> dict[str, Any]:
        await self._stop_kernel()
        self._session_id = self._initial_session_id
        self._history.clear()
        self._log_path.write_text("", encoding="utf-8")
        self._status = KernelStatus.IDLE
        return await self.summary()

    async def stop(self) -> None:
        await self._stop_kernel()
        self._status = KernelStatus.DONE

    def _derive_status(
        self,
        events: list[KernelEvent],
        fallback_status: KernelStatus,
    ) -> KernelStatus:
        for event in reversed(events):
            if event.type == EventType.SESSION_STATUS and event.status is not None:
                return event.status
        return fallback_status


def service_from_env() -> KernelSessionService:
    harness = HarnessName(os.environ.get("KERNEL_HARNESS", HarnessName.ACP))
    additional_paths = tuple(
        path
        for path in os.environ.get("KERNEL_ADDITIONAL_PATHS", "").split(os.pathsep)
        if path
    )

    skills_dir = os.environ.get("KERNEL_SKILLS_DIR", "")
    staging_dir = os.environ.get("KERNEL_SKILLS_STAGING_DIR", "")
    enabled_skills_raw = os.environ.get("KERNEL_ENABLED_SKILLS")
    enabled_skills = (
        {s for s in enabled_skills_raw.split(",") if s}
        if enabled_skills_raw is not None
        else None
    )

    if harness != HarnessName.COPILOT_CLI and staging_dir and skills_dir:
        link_enabled_skills(staging_dir, skills_dir, enabled_skills)

    skill_paths = (
        discover_skill_dirs(skills_dir, enabled_skills)
        if skills_dir and harness != HarnessName.COPILOT_CLI
        else ()
    )

    return KernelSessionService(
        harness=harness,
        env=dict(os.environ),
        additional_paths=additional_paths + skill_paths,
        session_id=os.environ.get("KERNEL_SESSION_ID") or None,
    )


def _remove_stale_skill_links(
    target: Path,
    staging_resolved: Path,
    enabled_skills: set[str] | None,
) -> None:
    """Remove symlinks in *target* pointing into *staging_resolved* but not enabled."""
    for existing in target.iterdir():
        if not existing.is_symlink():
            continue
        try:
            link_target = existing.resolve()
        except OSError:
            continue
        if not str(link_target).startswith(str(staging_resolved)):
            continue
        if enabled_skills is None or existing.name in enabled_skills:
            continue
        existing.unlink()
        logger.info("removed stale skill link %s", existing)


def link_enabled_skills(
    staging_dir: str,
    skills_dir: str,
    enabled_skills: set[str] | None,
) -> None:
    """Symlink enabled skills from *staging_dir* into *skills_dir*.

    Only skill directories whose name appears in *enabled_skills* are linked.
    If *enabled_skills* is ``None``, all skills are linked.

    Stale symlinks that point into *staging_dir* but are not in the enabled
    set are removed so that skills from previous sessions don't leak through
    persistent volumes.
    """
    staging = Path(staging_dir)
    target = Path(skills_dir)
    if not staging.is_dir():
        return
    target.mkdir(parents=True, exist_ok=True)

    _remove_stale_skill_links(target, staging.resolve(), enabled_skills)

    for entry in sorted(staging.iterdir()):
        if not entry.is_dir():
            continue
        if enabled_skills is not None and entry.name not in enabled_skills:
            continue
        link = target / entry.name
        if not link.exists():
            link.symlink_to(entry)
            logger.info("linked skill %s -> %s", link, entry)


def discover_skill_dirs(
    skills_dir: str,
    enabled_skills: set[str] | None = None,
) -> tuple[str, ...]:
    """Enumerate subdirectories under skills_dir to use as additional paths.

    If *enabled_skills* is not ``None``, only directories whose name appears
    in the set are included.  An empty set means no skills.
    """
    base = Path(skills_dir)
    if not base.is_dir():
        return ()
    return tuple(
        str(entry)
        for entry in sorted(base.iterdir())
        if entry.is_dir() and (enabled_skills is None or entry.name in enabled_skills)
    )
