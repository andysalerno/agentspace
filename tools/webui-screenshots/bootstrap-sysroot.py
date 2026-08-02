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
import hashlib
import platform
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

MIRROR = "https://deb.debian.org/debian"
DIST = "bookworm"

# Debian architecture name and multiarch triplet for the host. The dev image
# builds for both x86_64 and arm64, and a sysroot of foreign-architecture
# libraries would leave Chromium unable to start.
ARCHITECTURES = {
    "x86_64": ("amd64", "x86_64-linux-gnu"),
    "aarch64": ("arm64", "aarch64-linux-gnu"),
    "arm64": ("arm64", "aarch64-linux-gnu"),
}
_machine = platform.machine()
if _machine not in ARCHITECTURES:
    _supported = ", ".join(sorted(ARCHITECTURES))
    sys.exit(f"unsupported architecture {_machine!r}; expected one of {_supported}")
DEB_ARCH, MULTIARCH = ARCHITECTURES[_machine]
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


def fetch(url: str, dest: Path, sha256: str | None = None) -> Path:
    """Download `url` to `dest`, reusing a cached file only if it verifies.

    Archives from this cache are unpacked and then loaded into Chromium as
    executable code, so a cached file is trusted only when its digest matches
    the one Debian published for it.
    """
    if dest.exists() and dest.stat().st_size > 0:
        if sha256 is None or digest(dest) == sha256:
            return dest
        log(f"  warning: {dest.name} failed its checksum, re-downloading")
        dest.unlink()
    # Download beside the target and rename, so an interrupted transfer never
    # leaves a partial file that later runs treat as a valid cache entry.
    partial = dest.with_name(f"{dest.name}.partial")
    try:
        with (
            urllib.request.urlopen(url, timeout=180) as response,  # noqa: S310
            partial.open("wb") as out,
        ):
            shutil.copyfileobj(response, out)
        if sha256 is not None:
            actual = digest(partial)
            if actual != sha256:
                msg = f"checksum mismatch for {url}: expected {sha256}, got {actual}"
                raise RuntimeError(msg)
        partial.replace(dest)
    finally:
        partial.unlink(missing_ok=True)
    return dest


def digest(path: Path) -> str:
    """SHA-256 of `path`, streamed so large archives stay off the heap."""
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def load_index() -> dict[str, dict[str, str]]:
    """Download and parse the Debian binary package index.

    The index is what every later checksum is compared against, so it is itself
    checked against the digest published in `InRelease`. Both come from the
    mirror over HTTPS and the OpenPGP signature on `InRelease` is not verified,
    so this detects corruption and stale caches rather than a hostile mirror.
    """
    log("fetching Debian package index ...")
    relative = f"main/binary-{DEB_ARCH}/Packages.gz"
    path = fetch(
        f"{MIRROR}/dists/{DIST}/{relative}",
        CACHE / "Packages.gz",
        index_digest(relative),
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


def index_digest(relative: str) -> str:
    """SHA-256 that `InRelease` publishes for the index file `relative`."""
    url = f"{MIRROR}/dists/{DIST}/InRelease"
    with urllib.request.urlopen(url, timeout=180) as response:  # noqa: S310
        release = response.read().decode("utf-8", errors="replace")
    in_sha256 = False
    for line in release.splitlines():
        if not line.startswith((" ", "\t")):
            in_sha256 = line.strip() == "SHA256:"
            continue
        if not in_sha256:
            continue
        fields = line.split()
        if len(fields) == 3 and fields[2] == relative:
            return fields[0]
    msg = f"{url} does not list a SHA256 for {relative}"
    raise RuntimeError(msg)


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
        expected = meta.get("SHA256")
        if expected is None:
            msg = f"package index has no SHA256 for {name}"
            raise RuntimeError(msg)
        fetch(f"{MIRROR}/{meta['Filename']}", deb, expected)
        unpack(deb)
        if index % PROGRESS_EVERY == 0:
            log(f"  {index}/{len(order)}")

    install_fonts()
    # Single source of truth for the library paths capture.mjs adds to
    # LD_LIBRARY_PATH, so the two never disagree about the host architecture.
    (ROOT / ".multiarch").write_text(f"{MULTIARCH}\n", encoding="utf-8")
    shutil.rmtree(CACHE, ignore_errors=True)
    log(f"sysroot ready at {ROOT}")
    log("capture.mjs picks it up automatically.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
