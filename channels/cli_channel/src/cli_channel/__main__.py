from __future__ import annotations

import argparse
import asyncio
import logging
import sys

from cli_channel.client import ClientServiceChannelClient


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="AgentSpace CLI channel")
    parser.add_argument("--agent-id", required=True)
    parser.add_argument("--name", default="cli-channel")
    parser.add_argument("--cwd")
    parser.add_argument(
        "--client-service-base-url",
        default="http://127.0.0.1:8002",
    )
    return parser.parse_args()


async def run() -> None:
    logging.basicConfig(level=logging.INFO)
    args = parse_args()
    client = ClientServiceChannelClient(base_url=args.client_service_base_url)
    registration = await client.register_channel(
        agent_id=args.agent_id,
        name=args.name,
        cwd=args.cwd,
    )
    _write_line(f"channel: {registration.channel_id}")
    _write_line(f"session: {registration.session_id}")
    _write_line("commands: /reset, /exit")

    while True:
        try:
            prompt = (await asyncio.to_thread(input, "> ")).strip()
        except EOFError:
            _write_line("")
            return

        if not prompt:
            continue
        if prompt == "/exit":
            return
        if prompt == "/reset":
            registration = await client.reset(registration.channel_id)
            _write_line(f"reset -> session: {registration.session_id}")
            continue

        reply = await client.send_message(registration.channel_id, prompt)
        _write_line(reply.assistant_text)


def main() -> None:
    asyncio.run(run())


def _write_line(text: str) -> None:
    sys.stdout.write(f"{text}\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
