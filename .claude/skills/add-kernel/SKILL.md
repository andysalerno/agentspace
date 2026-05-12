---
name: add-kernel
description: "Add a new kernel to AgentSpace by implementing the Kernel protocol and wrapping an external CLI agent."
---

# Skill: Adding a New Kernel to AgentSpace

## Overview

A "kernel" in AgentSpace wraps an external CLI agent (e.g. copilot, codex, opencode) and translates its output into a standardized event stream. Each kernel is a separate Python package in `kernels/`.

## Step-by-step

### 1. Create the kernel package

Create `kernels/kernel_<name>/` with this structure:

```
kernels/kernel_<name>/
├── pyproject.toml
├── src/
│   └── kernel_<name>/
│       └── __init__.py
└── tests/
    └── test_<name>.py
```

**pyproject.toml** — minimal, depends on `kernel`:
```toml
[project]
name = "kernel-<name>"
version = "0.1.0"
description = "<Name> kernel implementation"
requires-python = ">=3.13"
dependencies = ["kernel"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
packages = ["src/kernel_<name>"]

[tool.pyright]
pythonVersion = "3.13"
typeCheckingMode = "strict"
```

**Note:** Do NOT add `__init__.py` to the `tests/` directory — other kernel test dirs don't have one and it causes namespace collisions.

### 2. Implement the kernel class

In `src/kernel_<name>/__init__.py`, implement a class that satisfies the `Kernel` protocol from `kernel.protocol`. The class needs:

- **Properties:** `name` (str), `status` (KernelStatus), `resume_token` (str | None)
- **Methods:** `start(config)`, `send(message)`, `recv()` (async iterator), `stop()`

**Pattern for CLI-wrapping kernels** (see kernel_codex, kernel_copilot, kernel_opencode):
- `start()` saves config, generates session ID, queues a `session_start` event
- `send(message)` spawns the CLI subprocess with the message, then reads output in a background task
- `recv()` yields events from an internal asyncio queue
- `stop()` terminates the subprocess

**Pattern for non-CLI kernels** (see kernel_echo):
- Purely in-process, no subprocess — just queue events directly
- Useful for testing or for wrapping APIs/SDKs instead of CLIs.

A kernel does NOT have to wrap a CLI. It just needs to satisfy the protocol. It could call an HTTP API, use a Python SDK, or do anything else.

**Key implementation details for CLI kernels:**
- Build the CLI command in `_build_command(message)` — map `KernelConfig.env` entries to CLI flags. Every CLI is different — check `--help` to learn the flags.
- Build env vars in `_build_env()` — forward API keys etc from `KernelConfig.env`
- Parse stdout in `_map_event(obj)` — translate the CLI's output format into standard `KernelEvent` types. The output format is entirely CLI-specific (JSONL, plain text, SSE, etc.).
- Read stderr as error events
- Use `KernelConfig.session_id` and `KernelConfig.env` for session resume, model selection, workspace dir, etc.

**Standard events to emit** (from `kernel.events`):
- `session_start(session_id, kernel_name)` — on start
- `status_event(KernelStatus.BUSY)` — when processing begins
- `text_delta(content)` — for streamed text output
- `reasoning_delta(content)` — for thinking/reasoning output
- `tool_call(tool_name, input_dict)` — when the agent invokes a tool
- `tool_result(tool_name, output_str)` — when a tool completes
- `status_event(KernelStatus.IDLE)` — when a turn completes
- `status_event(KernelStatus.DONE)` → `session_end()` → `None` (sentinel) — on finish

### 3. Write tests

In `tests/test_<name>.py`, capture real output from the tool and use it as test fixtures. Test:
- Event mapping for each event type the tool emits
- For CLI kernels: command building with various config options (model, session resume, etc.)
- Full conversation flows (simple text, tool use)
- Session ID capture

Use `# pyright: reportPrivateUsage=false` at the top since tests access private methods directly.

### 4. Register in the workspace root `pyproject.toml`

Three additions needed:
1. **`[tool.uv.sources]`** — add `kernel-<name> = { workspace = true }`
2. **`[tool.pyright] extraPaths`** — add `"kernels/kernel_<name>/src"`
3. **`[tool.ruff.lint.per-file-ignores]`** — add `"kernels/kernel_<name>/tests/test_<name>.py" = ["E501", "SLF001"]`

### 5. Register in the kernel host

**`kernels/kernel_host/pyproject.toml`** — add `kernel-<name>` to `dependencies`

**`kernels/kernel_host/src/kernel_host/registry.py`**:
- Import: `from kernel_<name> import <Name>Kernel`
- Add enum value: `<NAME> = "<name>"` to `HarnessName`
- Add to `KERNEL_REGISTRY`: `HarnessName.<NAME>: <Name>Kernel`

### 6. Register in the agent host

**`services/agent_host_rs/src/models.rs`** — add the harness variant to `HarnessName`.

**`services/agent_host_rs/src/docker_runtime.rs`** — add the harness to `skills_mount_path`:
```rust
HarnessName::<Name> => "/skills", // or a custom path if the CLI expects skills elsewhere
```

### 7. Update all Dockerfiles that copy kernel packages

These Dockerfiles copy the root `pyproject.toml` which references all workspace members, so every kernel dir must be present:

- `kernels/kernel_host/Dockerfile` — add `COPY kernels/kernel_<name> kernels/kernel_<name>`

If the CLI tool needs to be installed in the kernel_host container, also add an install command to `kernels/kernel_host/Dockerfile` (in the `RUN apt-get update` block).

### 8. Update tests that enumerate harnesses

Check and update any tests that assert on the full list of harnesses, e.g.:
- `services/client_service_rs/tests/route_contract.rs` — route tests that assert harness lists or session creation behavior
- `services/agent_host_rs/src/docker_runtime.rs` — `skills_mount_paths_cover_harnesses`

### 9. Verify

```bash
uv sync
just test  # all 164+ tests should pass
just stack-build  # Docker images should build
```

## Tips

- Every CLI/tool is different. Start by exploring its interface (`--help`, docs, etc.) to understand how to invoke it non-interactively and get structured output.
- Try to find a way to get JSON or structured output from the tool — this makes parsing much easier. Examples: `copilot --output-format json`, `codex exec --json`, `opencode run --format json`. If there's no JSON mode, you'll need to parse plain text.
- Run the tool with a simple prompt (e.g. "say hello") and a tool-using prompt (e.g. "list files in the current directory") to capture sample output for test fixtures.
- The core job of every kernel is mapping the tool's native output into standardized `KernelEvent` types.
- Look at existing kernels for the closest match to your tool's behavior and copy from there.
- Not all kernels wrap CLIs — a kernel could wrap an HTTP API, a Python SDK, or anything else. The only requirement is satisfying the `Kernel` protocol.
