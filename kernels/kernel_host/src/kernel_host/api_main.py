from __future__ import annotations

import uvicorn


def main() -> None:
    uvicorn.run(
        "kernel_host.app:app",
        host="0.0.0.0",  # noqa: S104
        port=8000,
        reload=False,
    )


if __name__ == "__main__":
    main()
