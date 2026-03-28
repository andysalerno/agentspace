"""Kernel host runner — container entry point.

Usage: python -m kernel_host.runner "your message here"

Reads KERNEL_HARNESS from env (default: echo), instantiates the kernel,
sends the message, and streams JSONL events to stdout.
"""

from __future__ import annotations

import asyncio
import logging
import os
import sys

from kernel.protocol import KernelConfig

from kernel_host.registry import get_kernel


def _configure_logging() -> None:
    level_name = os.environ.get("LOG_LEVEL", "WARNING").upper()
    level = getattr(logging, level_name, logging.WARNING)
    logging.basicConfig(level=level, format="%(levelname)s %(name)s: %(message)s")


async def run(message: str) -> None:
    harness_name = os.environ.get("KERNEL_HARNESS", "echo")
    kernel = get_kernel(harness_name)

    # Build config from environment
    config = KernelConfig(env=dict(os.environ))

    await kernel.start(config)

    # Send message (for echo kernel, triggers in-process; for subprocess
    # kernels, sends to stdin or was already passed as CLI arg)
    await kernel.send(message)

    # Stream events as JSONL to stdout
    async for event in kernel.recv():
        print(event.to_jsonl(), flush=True)  # noqa: T201

    await kernel.stop()


def main() -> None:
    _configure_logging()
    if len(sys.argv) < 2:
        print("Usage: python -m kernel_host.runner <message>", file=sys.stderr)  # noqa: T201
        sys.exit(1)

    message = sys.argv[1]
    asyncio.run(run(message))


if __name__ == "__main__":
    main()
