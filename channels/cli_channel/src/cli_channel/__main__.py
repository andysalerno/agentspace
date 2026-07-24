from __future__ import annotations

import argparse
import asyncio
import getpass
import io
import json
import logging
import os
import sys
import zipfile
from pathlib import Path

from cli_channel.client import ClientServiceSessionClient, ConfigDownload


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="AgentSpace CLI client")
    parser.add_argument("--agent-id")
    parser.add_argument("--name", default="cli")
    parser.add_argument("--session-id")
    parser.add_argument(
        "--client-service-base-url",
        default="http://127.0.0.1:8002",
    )
    commands = parser.add_subparsers(dest="command")

    config = commands.add_parser("config", help="Manage declarative configuration")
    config_commands = config.add_subparsers(dest="config_command", required=True)
    for command in ("validate", "plan", "apply"):
        command_parser = config_commands.add_parser(command)
        command_parser.add_argument(
            "-f",
            "--file",
            required=True,
            type=Path,
        )

    export = config_commands.add_parser("export")
    export.add_argument("resource", nargs="?")
    export.add_argument(
        "--mode",
        choices=("source", "canonical"),
        default="source",
    )
    export.add_argument("-o", "--output", type=Path)

    secret = commands.add_parser("secret", help="Manage write-only secret values")
    secret_commands = secret.add_subparsers(dest="action", required=True)
    secret_commands.add_parser("list")
    set_secret = secret_commands.add_parser("set")
    set_secret.add_argument("secret_name")
    set_secret.add_argument("--value-stdin", action="store_true")
    clear_secret = secret_commands.add_parser("clear")
    clear_secret.add_argument("secret_name")

    return parser.parse_args(argv)


async def run(
    argv: list[str] | None = None,
    client: ClientServiceSessionClient | None = None,
) -> None:
    log_level = os.environ.get("LOG_LEVEL", "INFO").upper()
    logging.basicConfig(
        level=getattr(logging, log_level, logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    args = parse_args(argv)
    if client is None:
        client = ClientServiceSessionClient(base_url=args.client_service_base_url)
    if args.command == "config":
        await _run_config(client, args)
        return
    if args.command == "secret":
        await _run_secret(client, args)
        return
    await _run_session(client, args)


async def _run_session(
    client: ClientServiceSessionClient,
    args: argparse.Namespace,
) -> None:
    if args.session_id is None and args.agent_id is None:
        msg = "--agent-id is required when --session-id is not provided"
        raise SystemExit(msg)
    registration = (
        await client.get_session(args.session_id)
        if args.session_id
        else await client.create_session(
            agent_id=args.agent_id,
            channel_name=args.name,
        )
    )
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
            registration = await client.reset(registration.session_id)
            _write_line(f"reset -> session: {registration.session_id}")
            continue

        reply = await client.send_message(registration.session_id, prompt)
        _write_line(reply.assistant_text)


async def _run_config(
    client: ClientServiceSessionClient,
    args: argparse.Namespace,
) -> None:
    if args.config_command == "export":
        download = await _export_config(client, args)
        output = args.output or Path(download.filename)
        _write_download(download, output)
        return

    source, content_type = _read_config_source(args.file)
    if args.config_command == "validate":
        result = await client.validate_config(source, content_type)
    elif args.config_command == "plan":
        result = await client.plan_config(source, content_type)
    else:
        result = await client.apply_config(source, content_type)
    _write_json(result)


async def _export_config(
    client: ClientServiceSessionClient,
    args: argparse.Namespace,
) -> ConfigDownload:
    if args.resource is None:
        return await client.export_config(args.mode)
    if "/" not in args.resource:
        msg = "resource must use KIND/NAME syntax"
        raise SystemExit(msg)
    kind, name = args.resource.split("/", maxsplit=1)
    if not kind or not name:
        msg = "resource must use KIND/NAME syntax"
        raise SystemExit(msg)
    return await client.export_resource(kind, name)


async def _run_secret(
    client: ClientServiceSessionClient,
    args: argparse.Namespace,
) -> None:
    if args.action == "list":
        for secret in await client.list_secrets():
            state = "set" if secret.is_set else "unset"
            description = "" if secret.description is None else secret.description
            _write_line(f"{secret.name}\t{state}\t{description}")
        return

    if args.action == "set":
        if args.value_stdin:
            value = sys.stdin.read().removesuffix("\n")
        else:
            value = await asyncio.to_thread(
                getpass.getpass,
                f"Value for {args.secret_name}: ",
            )
        if not value:
            msg = "secret value must not be empty"
            raise SystemExit(msg)
        await client.set_secret_value(args.secret_name, value)
        _write_line(f"{args.secret_name}: set")
        return

    await client.clear_secret_value(args.secret_name)
    _write_line(f"{args.secret_name}: cleared")


def _read_config_source(path: Path) -> tuple[bytes, str]:
    if path.is_dir():
        return _bundle_directory(path), "application/zip"
    if path.is_file():
        content_type = (
            "application/zip"
            if path.suffix.casefold() == ".zip"
            else "application/yaml"
        )
        return path.read_bytes(), content_type
    msg = f"config path not found: {path}"
    raise SystemExit(msg)


def _bundle_directory(path: Path) -> bytes:
    files = sorted(candidate for candidate in path.rglob("*") if not candidate.is_dir())
    if not any(candidate.suffix.casefold() in {".yaml", ".yml"} for candidate in files):
        msg = f"config directory contains no YAML files: {path}"
        raise SystemExit(msg)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for candidate in files:
            if candidate.is_symlink():
                msg = f"config bundles cannot contain symlinks: {candidate}"
                raise SystemExit(msg)
            relative = candidate.relative_to(path).as_posix()
            info = zipfile.ZipInfo(relative, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, candidate.read_bytes())
    return buffer.getvalue()


def _write_download(download: ConfigDownload, output: Path) -> None:
    if str(output) == "-":
        sys.stdout.buffer.write(download.content)
        sys.stdout.buffer.flush()
        return
    output.write_bytes(download.content)
    _write_line(str(output))


def _write_json(value: object) -> None:
    _write_line(json.dumps(value, indent=2, sort_keys=True))


def main() -> None:
    asyncio.run(run())


def _write_line(text: str) -> None:
    sys.stdout.write(f"{text}\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
