## Current Shape

The repository is in an early kernel-first phase.

Today the implemented stack is:

1. `kernel`
   Defines the shared protocol and the JSONL event contract.
2. `kernel_echo`
   Small in-process reference implementation for fast testing.
3. `kernel_copilot`
   Wraps `copilot -p ... --output-format json` and maps Copilot events into the shared event stream.
4. `kernel_host`
   Can run either as a one-shot runner or as a one-session HTTP service inside a kernel container.
5. `agent_host`
   Manages sessions by spawning and supervising `kernel_host` containers and exposing them via a small FastAPI service.
6. `client_service`
   Client-facing gateway that stores agent definitions, transcript history, and session metadata in memory while proxying session work to `agent_host`.
7. `webui`
   Minimal TypeScript dashboard that talks only to `client_service`.
8. `cli_channel`
   A proof-of-concept external CLI process that creates or resumes sessions through `client_service`.

This is the thin vertical slice needed before building the higher-level services from `PLAN.md`.

## Event Contract

Every kernel emits standard events on stdout as JSON Lines:

- `session_start`
- `status`
- `text_delta`
- `tool_call`
- `tool_result`
- `error`
- `session_end`

The host and any future `agent-host` or `client-service` layers can consume this stream without caring which harness produced it.

## Copilot-Only Milestone

The active non-test path is `copilot-cli`.

Important runtime decisions in the current prototype:

- one host invocation runs one prompt through one kernel
- Copilot runs in non-interactive prompt mode with JSON output
- Copilot config/session data is persisted in a named Docker volume
- `KERNEL_WORKDIR` is intentionally left for the deployer to decide
- launch scripts tear down previous compose resources before new runs

## What Exists vs Planned

Implemented:

- kernel abstraction
- echo kernel
- copilot kernel
- kernel host runner
- kernel host per-session HTTP service mode
- Docker launch path for the kernel host
- in-memory agent host service
- in-memory client service
- minimal hosted web UI
- kernel-session listing routed from `agent_host` through `client_service`
- optional session source metadata (`channel_name`, `client_type`) in `client_service`
- minimal `cli_channel` proof client
- automated tests for event serialization, echo flow, copilot mapping, and runner config

Not implemented yet:

- `proto/`
- `client-cli/`
- `channels/`
- `store/`

Those remain aligned to `PLAN.md`, but the repo has not reached that breadth yet.
