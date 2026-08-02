# Playwright screenshots of the web UI

This document explains how to render and screenshot the AgentSpace web UI
(`clients/webui`) with Playwright, without a running backend. It exists because
getting a headless browser working in this repository's dev container is not
obvious, and the failure modes are misleading.

## TL;DR

```bash
just webui-screenshots-setup   # once per container
just webui-screenshots         # writes PNGs to tools/webui-screenshots/out/
```

Then open the PNGs, or view them from an agent session with an image-capable
tool. Screenshots are captured for every view in both light and dark themes.

## What the harness is

`tools/webui-screenshots/` contains three pieces:

| File | Purpose |
| --- | --- |
| `mock-api.mjs` | HTTP server on `:8010`. Serves `clients/webui/dist` as static files and answers every `/api/*` route with realistic fixture data. |
| `capture.mjs` | Drives Playwright over the view matrix and writes one PNG per view per theme. |
| `bootstrap-sysroot.py` | Fallback that downloads Chromium's shared libraries when they are not installed on the host. Not needed in an up-to-date dev container. |

The harness never talks to `client_service` or `agent_host`. It only needs a
production build of the web UI on disk, which makes it fast (sub-second builds)
and usable offline.

### Why a production build and not the dev server

The Vite dev server serves thousands of unbundled ES modules. Loading them in a
memory-constrained container reliably OOM-kills the renderer partway through
the run. `vite build` takes under a second here and produces a single bundle, so
the harness always screenshots `dist/`. Rebuild after each change:

```bash
cd clients/webui && pnpm run build
```

`just webui-screenshots` does this for you.

## Environment requirements

Playwright's bundled Chromium is a dynamically linked binary. It needs both
shared libraries and at least one installed font.

### Shared libraries

`playwright install --with-deps` **does not work here**. It only knows how to
provision Debian and Ubuntu, and it shells out to `sudo`, which is absent. On
openSUSE it fails with:

```
BEWARE: your OS is not officially supported by Playwright
Failed to install browsers
Error: spawn sudo ENOENT
```

The dev container therefore installs the libraries natively via `zypper`. They
are already listed in `dev.Dockerfile`; the openSUSE package names are:

```
libX11-6 libXcomposite1 libXdamage1 libXext6 libXfixes3 libXi6 libXrandr2
libXrender1 libXtst6 libasound2 libatk-1_0-0 libatk-bridge-2_0-0 libatspi0
libcairo2 libcups2 libdrm2 libgbm1 libpango-1_0-0 libxcb1 libxkbcommon0
libxshmfence1 mozilla-nspr mozilla-nss
```

If they are missing, Chromium fails to launch at all:

```
chrome-headless-shell: error while loading shared libraries:
libgobject-2.0.so.0: cannot open shared object file
```

### Fonts

This is the non-obvious one. **Chromium aborts with `SIGTRAP` if Skia cannot
resolve a single font.** The symptom in Playwright is not a font error, it is:

```
Target page, context or browser has been closed
```

The page simply dies, usually on the first view that renders substantial text.
The real cause is only visible with `DEBUG=pw:browser`:

```
FATAL:third_party/skia/src/ports/SkFontMgr_FontConfigInterface.cpp:163] Not implemented.
```

The dev container installs `dejavu-fonts`, `google-noto-fonts`, and
`fontconfig`. If you hit this on another host, installing any TrueType font
into `~/.fonts` is enough.

## Fallback: no root and no packages

If you are on a host where you cannot install system packages, run:

```bash
cd tools/webui-screenshots
python3 bootstrap-sysroot.py
```

It downloads Debian `.deb` archives, unpacks the libraries into `.sysroot/`,
and copies the bundled fonts into `~/.fonts`. `capture.mjs` detects `.sysroot/`
automatically and sets `LD_LIBRARY_PATH`, `FONTCONFIG_FILE`, and `XDG_DATA_DIRS`
for the browser process.

Deliberately excluded from the sysroot: `libc6`, `libgcc-s1`, `libstdc++6`, and
friends. Mixing a Debian libc with the host loader breaks every binary in the
container, including `node`. Use the host's copies of those.

Requires `ar` (binutils) and `tar`, both present in the dev container.

## Running it manually

```bash
cd clients/webui && pnpm run build

cd ../../tools/webui-screenshots
node mock-api.mjs &          # http://127.0.0.1:8010
node capture.mjs ./out
```

`capture.mjs` honours these environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `BASE_URL` | `http://127.0.0.1:8010` | Where the UI is served. |
| `THEMES` | `light,dark` | Comma separated. |
| `ONLY` | all | Comma separated view ids, e.g. `ONLY=chat,memory`. |
| `WIDTH` / `HEIGHT` | `1440` / `900` | Viewport size. |
| `PW_SYSROOT` | `./.sysroot` | Override the fallback sysroot location. |

View ids: `chat`, `agents`, `workspaces`, `sessions`, `kernels`, `memory`,
`gateways`, `skills`, `info`, `config`, `config-secrets`, `config-kernels`,
`connections`.

Output files are named `<theme>-<view>.png`.

## Memory

Chromium's renderer accumulates memory across navigations, and this container
has a modest budget. `capture.mjs` launches a **fresh browser per view** for
that reason. A single long-lived browser walking the whole matrix gets killed
around the sixth or seventh view. If you rewrite the harness, keep that
property.

## Extending the mock data

`mock-api.mjs` holds plain object fixtures near the top of the file (agents,
sessions, workspaces, kernels, gateways, secrets, memory pages, and a
multi-turn chat transcript with tool calls). Add or edit entries there to
exercise a state you care about, such as long names, error statuses, or empty
lists.

Routes are matched in two passes: an exact `METHOD /path` lookup table, then a
series of regular expressions for parameterised paths. Anything unmatched under
`/api/` returns `{}` with status 200, so an unstubbed endpoint degrades to an
empty view rather than a crash.

When adding a route, match the real response shape in
`clients/webui/src/types.ts`. A shape mismatch surfaces as a `pageerror` line in
the capture output, for example `TypeError: schema.fields is not iterable`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `Target page, context or browser has been closed` | No fonts installed. | Install a TrueType font, or run `bootstrap-sysroot.py`. |
| `error while loading shared libraries: libgobject-2.0.so.0` | Chromium runtime libraries missing. | Rebuild the dev image, or run `bootstrap-sysroot.py`. |
| `Error: spawn sudo ENOENT` | You ran `playwright install --with-deps`. | Drop `--with-deps`; install the libraries via `zypper`. |
| `Fontconfig error: Cannot load default config file` | Harmless warning when `/etc/fonts` is absent. | Ignore, or set `FONTCONFIG_FILE`. |
| `no version information available (required by libselinux.so.1)` | Harmless sysroot symbol-versioning warning. | Ignore. |
| `Failed to connect to the bus` / `drmGetDevices2()` | No D-Bus or GPU in the container. | Ignore; the harness passes `--disable-gpu`. |
| Blank or partial page | `dist/` is stale or missing. | Re-run `pnpm run build` in `clients/webui`. |

To see what Chromium is actually doing, re-run with browser logging:

```bash
DEBUG=pw:browser node capture.mjs 2>&1 | grep -i "err\]"
```
