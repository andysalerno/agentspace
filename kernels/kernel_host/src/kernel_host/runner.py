"""Kernel host runner — container entry point.

Usage: python -m kernel_host.runner "your message here"

Reads KERNEL_HARNESS from env (default: echo), instantiates the kernel,
sends the message, and streams JSONL events to stdout.
"""

from __future__ import annotations

import asyncio
import logging
import os
import re
import sys

from kernel.protocol import KernelConfig

from kernel_host.registry import HarnessName, get_kernel


def _configure_logging() -> None:
    level_name = os.environ.get("LOG_LEVEL", "WARNING").upper()
    level = getattr(logging, level_name, logging.WARNING)
    logging.basicConfig(level=level, format="%(levelname)s %(name)s: %(message)s")


async def run(message: str) -> None:
    harness_name = HarnessName(os.environ.get("KERNEL_HARNESS", HarnessName.ECHO))
    kernel = get_kernel(harness_name)

    additional_paths = tuple(
        _split_paths(os.environ.get("KERNEL_ADDITIONAL_PATHS", "")),
    )
    config = KernelConfig(
        env=dict(os.environ),
        session_id=os.environ.get("KERNEL_SESSION_ID") or None,
        additional_paths=additional_paths,
    )

    await kernel.start(config)
    send_task = asyncio.create_task(kernel.send(message))
    try:
        async for event in kernel.recv():
            print(event.to_jsonl(), flush=True)  # noqa: T201
        await send_task
    finally:
        await send_task
        await kernel.stop()


def _split_paths(raw: str) -> list[str]:
    parts = [segment for segment in re.split(r"[\n;]+", raw) if segment]
    if len(parts) != 1 or ":" not in raw:
        return parts

    colon_parts = [segment for segment in raw.split(":") if segment]
    if all(segment.startswith("/") for segment in colon_parts):
        return colon_parts
    return parts


def main() -> None:
    _configure_logging()
    if len(sys.argv) < 2:
        print("Usage: python -m kernel_host.runner <message>", file=sys.stderr)  # noqa: T201
        sys.exit(1)

    message = sys.argv[1]
    asyncio.run(run(message))


if __name__ == "__main__":
    main()
