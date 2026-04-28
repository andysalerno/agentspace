# Pythondev rules

These rules apply to any change under any Python package in this repo
(`kernels/*`, `services/*`, `channels/*`, `gateways/*`, `clients/cli_ui`, and
any future Python package added to the `uv` workspace).

## Required checks before reporting a change complete

After finishing any work in this repo, run the aggregate verification recipe
from the repo root and confirm it passes:

```
just check
```

After making changes to any Python source, always run **all** of the
following from the repo root and confirm they pass:

```
uv run ruff format .
uv run ruff check .
uv run pyright
uv run --all-packages pytest
```

A change is not complete until every one of these exits cleanly. Do not
report a Python change as done based on tests alone — formatting, lint,
and type-check must also pass.

The `just check` recipe runs these Python checks plus the web checks. Use the
individual commands above when you need to format or diagnose a Python-only
failure; use `just test` when you only need the Python test suite
(`uv run --all-packages pytest`).

### 1. `ruff format` — formatting

`ruff format` is the canonical formatter for this repo. Run it before
`ruff check` so the lint pass sees correctly formatted code. It must
produce **zero changes** when re-run on a clean tree.

### 2. `ruff check` — lint

Configured in [pyproject.toml](pyproject.toml) under `[tool.ruff.lint]`.
All default rule groups are enabled. This catches real bug classes and
style violations:

- unused imports, unused variables, dead code
- mutable default arguments, broad `except:` clauses
- import ordering, naming conventions, docstring rules
- complexity / argument-count thresholds (`PLR0913` etc.)

`ruff check` must produce **zero new errors** introduced by your change.
Pre-existing baseline errors on `main` are acceptable to leave alone, but
do not add new ones. Auto-fix what you can with `uv run ruff check --fix`.

### 3. `pyright` — strict type checking

`pyright` runs in **strict mode** across the whole workspace. This
catches:

- missing or incorrect type annotations
- unsafe `Any` propagation
- protocol / structural-typing mismatches
- unreachable code, unused `# type: ignore`
- nullable access without narrowing

`pyright` must produce **zero errors**. Strict mode is non-negotiable —
do not weaken it locally with broad `# pyright: ignore` to make a change
pass.

### 4. `pytest` — tests

Run the full workspace test suite with `uv run --all-packages pytest`
(or equivalently `just test`). `asyncio_mode = "auto"` is set, so async
tests do not need explicit markers.

The full suite must pass. Do not skip or `xfail` a test to land a
change unless you have a clear reason and have flagged it to the user.

## Adding a new runtime dependency

If your change introduces a new `import some_package` in Python source,
you must:

1. Add the package to the appropriate `dependencies` (or
   `[dependency-groups].dev` for test/lint-only deps) array in the
   relevant package's `pyproject.toml`. Each Python package in this
   workspace has its own `pyproject.toml`; add the dep to the package
   that imports it, not the workspace root.
2. Run `uv sync --all-packages --dev` so `uv.lock` is updated.
3. Re-run the four checks above to confirm a clean pass.

Never rely on a package being available transitively through another
dependency. If you import it, declare it.

## Type-only imports

Pure `from typing import TYPE_CHECKING` guarded imports still count as
a dependency on the imported package. They must be declared in the
package's `pyproject.toml`.
