"""AgentSpace TUI — entry point."""

from __future__ import annotations

import argparse

from cli_ui.app import AgentSpaceApp


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="AgentSpace TUI client")
    parser.add_argument(
        "--url",
        default="http://127.0.0.1:8002",
        help="client-service base URL",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    app = AgentSpaceApp(base_url=args.url)
    app.run()


if __name__ == "__main__":
    main()
