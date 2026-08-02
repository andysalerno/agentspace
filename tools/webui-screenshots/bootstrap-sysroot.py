#!/usr/bin/env python3
"""Build a local sysroot so Playwright's bundled Chromium can run.

Only needed on hosts where the Chromium shared libraries are not installed and
you cannot install them (no root, or a distro Playwright cannot provision).
The dev container installs them natively -- see dev.Dockerfile -- so this
script is a fallback, not the normal path.

Downloads Debian .deb archives, unpacks them into ./.sysroot, and installs the
bundled fonts into ~/.fonts. Chromium aborts with SIGTRAP if Skia cannot
resolve any font, so the fonts are not optional.

Usage:  python3 bootstrap-sysroot.py
"""

from __future__ import annotations

import gzip
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

MIRROR = "https://deb.debian.org/debian"
DIST = "bookworm"
HERE = Path(__file__).resolve().parent
ROOT = HERE / ".sysroot"
CACHE = HERE / ".debcache"
FONT_DIR = Path("~/.fonts").expanduser()
PROGRESS_EVERY = 20

SEEDS = [
    "libasound2",
    "libatk-bridge2.0-0",
    "libatk1.0-0",
    "libatspi2.0-0",
    "libcairo-gobject2",
    "libcairo2",
    "libcups2",
    "libdbus-1-3",
    "libdrm2",
    "libexpat1",
    "libfontconfig1",
    "libfreetype6",
    "libgbm1",
    "libglib2.0-0",
    "libgtk-3-0",
    "libharfbuzz0b",
    "libnspr4",
    "libnss3",
    "libpango-1.0-0",
    "libpangocairo-1.0-0",
    "libx11-6",
    "libxcb1",
    "libxcomposite1",
    "libxdamage1",
    "libxext6",
    "libxfixes3",
    "libxi6",
    "libxkbcommon0",
    "libxrandr2",
    "libxrender1",
    "libxshmfence1",
    "libxtst6",
    "fontconfig",
    "fontconfig-config",
    "fonts-dejavu-core",
    "fonts-liberation2",
]

# Provided by the host. A Debian libc mixed with the host loader breaks every
# binary in the container, including node.
SKIP = {
    "libc6",
    "libgcc-s1",
    "libstdc++6",
    "libc-bin",
    "gcc-12-base",
    "libcrypt1",
    "debianutils",
    "dpkg",
    "install-info",
    "libselinux1",
    "sensible-utils",
    "adduser",
    "passwd",
    "init-system-helpers",
    "libpam0g",
    "libaudit1",
    "libsemanage2",
    "libsepol2",
    "shared-mime-info",
    "hicolor-icon-theme",
    "adwaita-icon-theme",
    "libgtk-3-common",
    "gtk-update-icon-cache",
    "ucf",
    "libdconf1",
    "dconf-service",
    "dconf-gsettings-backend",
}


def log(message: str) -> None:
    sys.stdout.write(f"{message}\n")
    sys.stdout.flush()


def fetch(url: str, dest: Path) -> Path:
    """Download `url` to `dest`, skipping files already present."""
    if dest.exists() and dest.stat().st_size > 0:
        return dest
    # Download beside the target and rename, so an interrupted transfer never
    # leaves a partial file that later runs treat as a valid cache entry.
    partial = dest.with_name(f"{dest.name}.partial")
    try:
        with (
            urllib.request.urlopen(url, timeout=180) as response,  # noqa: S310
            partial.open("wb") as out,
        ):
            shutil.copyfileobj(response, out)
        partial.replace(dest)
    finally:
        partial.unlink(missing_ok=True)
    return dest


def load_index() -> dict[str, dict[str, str]]:
    """Download and parse the Debian binary package index."""
    log("fetching Debian package index ...")
    path = fetch(
        f"{MIRROR}/dists/{DIST}/main/binary-amd64/Packages.gz",
        CACHE / "Packages.gz",
    )
    packages: dict[str, dict[str, str]] = {}
    current: dict[str, str] = {}
    with gzip.open(path, "rt", errors="replace") as handle:
        for line in handle:
            if not line.strip():
                if current.get("Package"):
                    packages.setdefault(current["Package"], current)
                current = {}
            elif not line.startswith((" ", "\t")) and ":" in line:
                key, value = line.split(":", 1)
                current[key] = value.strip()
    if current.get("Package"):
        packages.setdefault(current["Package"], current)
    return packages


def resolve(packages: dict[str, dict[str, str]]) -> list[str]:
    """Walk `Depends` from the seed set, taking the first of each alternative."""
    seen: set[str] = set()
    order: list[str] = []
    queue = list(SEEDS)
    while queue:
        name = queue.pop(0)
        if name in seen or name in SKIP or name not in packages:
            continue
        seen.add(name)
        order.append(name)
        for group in packages[name].get("Depends", "").split(","):
            first = group.split("|")[0].strip()
            if first:
                queue.append(first.split()[0].split(":")[0])
    return order


def unpack(deb: Path) -> None:
    """Extract the data payload of a .deb into the sysroot."""
    subprocess.run(["ar", "x", str(deb), "--output", str(CACHE)], check=True)  # noqa: S603, S607
    for name in ("data.tar.xz", "data.tar.gz", "data.tar.zst"):
        payload = CACHE / name
        if payload.exists():
            subprocess.run(["tar", "-xf", str(payload), "-C", str(ROOT)], check=True)  # noqa: S603, S607
            payload.unlink()
            return
    log(f"  warning: no data tarball inside {deb.name}")


def install_fonts() -> None:
    """Copy the unpacked TrueType fonts where fontconfig looks by default."""
    fonts = ROOT / "usr/share/fonts/truetype"
    if not fonts.is_dir():
        return
    FONT_DIR.mkdir(parents=True, exist_ok=True)
    for entry in fonts.iterdir():
        target = FONT_DIR / entry.name
        if not target.exists():
            shutil.copytree(entry, target)
    log(f"installed fonts into {FONT_DIR}")


def main() -> int:
    """Download, unpack, and wire up the Chromium sysroot."""
    if shutil.which("ar") is None:
        sys.stderr.write("error: 'ar' (binutils) is required\n")
        return 1
    CACHE.mkdir(parents=True, exist_ok=True)
    ROOT.mkdir(parents=True, exist_ok=True)

    packages = load_index()
    order = resolve(packages)
    log(f"unpacking {len(order)} packages into {ROOT}")
    for index, name in enumerate(order, 1):
        meta = packages[name]
        deb = CACHE / Path(meta["Filename"]).name
        fetch(f"{MIRROR}/{meta['Filename']}", deb)
        unpack(deb)
        if index % PROGRESS_EVERY == 0:
            log(f"  {index}/{len(order)}")

    install_fonts()
    shutil.rmtree(CACHE, ignore_errors=True)
    log(f"sysroot ready at {ROOT}")
    log("capture.mjs picks it up automatically.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
