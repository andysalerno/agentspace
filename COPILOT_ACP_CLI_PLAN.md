# GitHub Copilot CLI ACP Integration Plan

## Status

**Implemented as a disabled experimental path; production enablement remains
blocked by the upstream compatibility gate.**

AgentSpace should expose GitHub Copilot CLI through the existing canonical
`acp` harness and run it as:

```text
copilot --acp --yolo
```

The legacy `kernel_copilot` implementation, which invokes prompt mode and
parses Copilot-specific JSONL events, should be deleted. GitHub login will not
be supported. Every Copilot-backed agent must use an AgentSpace Connection for
its model provider, and the child process must always run with
`COPILOT_OFFLINE=true`.

Production enablement must not occur until a released Copilot CLI version
passes the no-login ACP compatibility gate described below. Copilot CLI 1.0.73
does not:
it advertises only `copilot-login` during ACP initialization and rejects
`authenticate` and `session/new` with `-32000 Authentication required`, even
when a complete offline BYOK provider is configured.

At the user's direction, the integration was implemented behind
`KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED`, which defaults off. This permits
review and compatibility testing without presenting the currently broken
upstream path as supported.

## Goals

- Add GitHub Copilot CLI as a named ACP server behind the existing `acp`
  harness.
- Reuse `kernel_acp.AcpKernel` for JSON-RPC, session lifecycle, streaming,
  terminal, filesystem, permission, and resume behavior.
- Launch Copilot with `--acp --yolo`.
- Require an AgentSpace Connection instead of GitHub authentication.
- Translate Connection and agent model settings into Copilot's supported BYOK
  environment variables.
- Force offline mode and prevent inherited GitHub credentials or persisted
  login state from affecting the process.
- Preserve existing OpenCode ACP support as a separate ACP-server profile.
- Remove the old `copilot-cli` harness, parser, package, login setup flow, and
  shared Copilot credential volume.
- Migrate existing `copilot-cli` agent records to the ACP/Copilot profile.

## Non-goals

- GitHub OAuth, device login, PAT login, account switching, or any other GitHub
  authentication flow.
- GitHub MCP, GitHub Code Search, `/delegate`, remote sessions, telemetry, or
  other GitHub-hosted integrations.
- A fallback from a misconfigured Connection to GitHub-hosted model routing.
- Maintaining the old Copilot prompt-mode JSONL parser.
- Translating ACP events into Copilot-specific or legacy kernel events.
- Claiming that a remote BYOK endpoint is air-gapped. Offline mode prevents
  GitHub traffic, but prompts still travel to a remote configured provider.

## Research Findings

### Copilot CLI provider contract

Current Copilot CLI help and GitHub documentation define these BYOK settings:

| Setting | Meaning |
| --- | --- |
| `COPILOT_PROVIDER_BASE_URL` | Activates BYOK and identifies the provider endpoint. |
| `COPILOT_PROVIDER_TYPE` | `openai` (default), `azure`, or `anthropic`. |
| `COPILOT_PROVIDER_API_KEY` | Optional API key; local providers may omit it. |
| `COPILOT_PROVIDER_BEARER_TOKEN` | Optional bearer token; takes precedence over the API key. |
| `COPILOT_PROVIDER_WIRE_API` | `completions` (default) or `responses`. |
| `COPILOT_PROVIDER_TRANSPORT` | `http` (default) or `websockets`. |
| `COPILOT_PROVIDER_AZURE_API_VERSION` | Optional Azure API version. |
| `COPILOT_PROVIDER_HEADERS` | Optional provider-only headers. |
| `COPILOT_MODEL` | Simple model setting used as both internal and wire model. |
| `COPILOT_PROVIDER_MODEL_ID` | Optional known base model for agent behavior and limits. |
| `COPILOT_PROVIDER_WIRE_MODEL` | Optional model/deployment name sent to the provider. |
| `COPILOT_PROVIDER_MAX_PROMPT_TOKENS` | Optional prompt-token override. |
| `COPILOT_PROVIDER_MAX_OUTPUT_TOKENS` | Optional output-token override. |
| `COPILOT_OFFLINE=true` | Disables GitHub authentication, telemetry, web tools, GitHub MCP, and auto-update. |

GitHub states that BYOK normally does not require GitHub authentication, that
offline mode limits network requests to the configured provider, and that
models must support streaming and tool calling. A context window of at least
128k tokens is recommended.

### ACP contract

Copilot's `--acp` mode is a newline-delimited JSON-RPC ACP server. AgentSpace's
existing `AcpKernel` already implements the required client flow:

1. `initialize`
2. `session/new`, `session/load`, or `session/resume`
3. `session/prompt`
4. `session/update` passthrough
5. client-side filesystem, terminal, and permission request handling

No Copilot event parser is needed in this path.

### Upstream blocker

The no-login requirement is currently incompatible with Copilot's ACP server:

- GitHub documents unauthenticated BYOK and offline operation for Copilot CLI
  generally.
- Copilot CLI 1.0.56 claimed that BYOK provider configuration applied to ACP
  sessions.
- Copilot CLI 1.0.69 changed ACP authentication to require a Copilot login
  before `authenticate` succeeds.
- Open issues
  [github/copilot-cli#4016](https://github.com/github/copilot-cli/issues/4016)
  and
  [github/copilot-cli#4037](https://github.com/github/copilot-cli/issues/4037)
  report that ACP still rejects BYOK-only sessions.
- A local probe against the stable release current when this plan was written,
  1.0.73, using
  `COPILOT_OFFLINE=true`, all required provider variables, and
  `copilot --acp --yolo` reproduced the failure:
  `initialize` succeeded, while both `authenticate` and `session/new` returned
  `Authentication required`.

AgentSpace must not work around this by adding a GitHub token or persisted
login. The integration should remain gated until an upstream stable release
supports ACP session creation with BYOK and no GitHub identity.

### Sources

- [Using your own LLM models in GitHub Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-byok-models)
- [Authenticating GitHub Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli)
- [Copilot CLI BYOK and local model announcement](https://github.blog/changelog/2026-04-07-copilot-cli-now-supports-byok-and-local-models/)
- [Copilot CLI changelog](https://github.com/github/copilot-cli/blob/main/changelog.md)
- [Copilot CLI ACP server reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
- [ACP initialization](https://agentclientprotocol.com/protocol/v1/initialization)
- [ACP authentication](https://agentclientprotocol.com/protocol/v1/authentication)
- [ACP session setup](https://agentclientprotocol.com/protocol/v1/session-setup)
- [Copilot ACP BYOK bug #4016](https://github.com/github/copilot-cli/issues/4016)
- [Copilot ACP BYOK feature request #4037](https://github.com/github/copilot-cli/issues/4037)

## Current AgentSpace Shape

- `kernel_acp.AcpKernel` is the only registered kernel and currently defaults
  to `opencode acp`.
- The ACP kernel contains OpenCode-specific command, provider-file, permission,
  and custom-agent setup. That setup currently runs unconditionally and
  requires `CONNECTION_API_KEY`, so the profile refactor must land before a
  keyless local Copilot provider can work.
- `kernel_copilot.CopilotKernel` remains in the tree but is excluded from the
  active Python workspace. It invokes `copilot -p --output-format json` and
  maintains a custom JSONL parser.
- Rust service enums and the web UI still advertise retired harness values,
  including `copilot-cli`.
- Connections currently provide a URL, API flavor, and optional API key. At
  session creation, `client_service` emits `CONNECTION_URL`,
  `CONNECTION_API_FLAVOR`, and `CONNECTION_API_KEY`.
- The ACP model field is currently `KERNEL_ACP_MODEL_NAME`.
- Every kernel container receives a shared `/root/.copilot` volume originally
  intended to persist GitHub login state.
- Standalone scripts and documentation still include a `setup` path that asks
  users to run `copilot login`.

## Target Architecture

```text
Agent + ACP server selection + Connection
                  |
                  v
client_service resolves provider-neutral CONNECTION_* values
                  |
                  v
agent_host passes the session environment to kernel_host
                  |
                  v
AcpKernel selects its ACP-server profile
                  |
          +-------+--------+
          |                |
          v                v
   OpenCode profile   Copilot profile
                      - map provider env
                      - force offline
                      - remove GitHub tokens
                      - copilot --acp --yolo
                              |
                              v
                   user-selected model provider
```

The public harness remains `acp`. The ACP server implementation is a separate
setting, proposed as:

```text
KERNEL_ACP_SERVER=copilot
```

Supported initial values are `copilot`, `opencode`, and `custom`. Keeping this
selection separate from `HarnessName` avoids reintroducing a Copilot-specific
kernel and avoids confusing the ACP server with the Connection's model
provider. An absent value resolves to `opencode` for backward compatibility
with existing ACP agents.

## Configuration Contract

### Connection fields

Extend Connection records with the provider metadata needed by Copilot while
keeping existing records valid:

| Connection field | Provider-neutral session key | Copilot child-process key |
| --- | --- | --- |
| `url` | `CONNECTION_URL` | `COPILOT_PROVIDER_BASE_URL` |
| `provider_type` | `CONNECTION_PROVIDER_TYPE` | `COPILOT_PROVIDER_TYPE` |
| `api_flavor=chat_completions` | `CONNECTION_API_FLAVOR=chat_completions` | `COPILOT_PROVIDER_WIRE_API=completions` |
| `api_flavor=responses` | `CONNECTION_API_FLAVOR=responses` | `COPILOT_PROVIDER_WIRE_API=responses` |
| `api_key` | `CONNECTION_API_KEY` | `COPILOT_PROVIDER_API_KEY` |
| `bearer_token` | `CONNECTION_BEARER_TOKEN` | `COPILOT_PROVIDER_BEARER_TOKEN` |
| `transport` | `CONNECTION_TRANSPORT` | `COPILOT_PROVIDER_TRANSPORT` |
| `azure_api_version` | `CONNECTION_AZURE_API_VERSION` | `COPILOT_PROVIDER_AZURE_API_VERSION` |
| `headers` | `CONNECTION_HEADERS` | `COPILOT_PROVIDER_HEADERS` |

Defaults:

- `provider_type=openai`
- `api_flavor=chat_completions`
- `transport=http`

`api_key`, `bearer_token`, and `headers` are secrets. API responses should
report only presence metadata for them after creation/update, and logs must
never include their values. API key and bearer token should be mutually
exclusive to avoid Copilot's documented credential precedence ambiguity.

The first migration should add `provider_type` and optional advanced fields
without changing existing Connection IDs. Existing rows become OpenAI
Chat-Completions HTTP connections.

### Agent/model fields

Keep the existing AgentSpace model setting and translate it inside the Copilot
ACP-server profile:

| AgentSpace setting | Copilot child-process setting |
| --- | --- |
| `KERNEL_ACP_MODEL_NAME` | `COPILOT_MODEL` |
| `KERNEL_ACP_PROVIDER_MODEL_ID` | `COPILOT_PROVIDER_MODEL_ID` |
| `KERNEL_ACP_PROVIDER_WIRE_MODEL` | `COPILOT_PROVIDER_WIRE_MODEL` |
| `KERNEL_ACP_MAX_PROMPT_TOKENS` | `COPILOT_PROVIDER_MAX_PROMPT_TOKENS` |
| `KERNEL_ACP_MAX_OUTPUT_TOKENS` | `COPILOT_PROVIDER_MAX_OUTPUT_TOKENS` |

`KERNEL_ACP_MODEL_NAME` and a Connection are required when
`KERNEL_ACP_SERVER=copilot`. Model discovery remains a convenience; manual
model entry must remain available because not every supported provider exposes
an OpenAI-compatible `/models` endpoint.

### Forced process settings

The Copilot profile must apply these settings after all kernel, Connection, and
agent environment overlays:

```text
COPILOT_OFFLINE=true
```

It must also remove `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, and `GITHUB_TOKEN` from
the child environment, regardless of inherited host or agent settings. Before
building the provider environment, it must also clear inherited/raw
`COPILOT_PROVIDER_*` and `COPILOT_MODEL` values, then repopulate only the
allowlisted values derived from the selected Connection and AgentSpace model
settings. This makes the Connection the authoritative provider source rather
than allowing raw agent environment variables to bypass validation. A
session-local `COPILOT_HOME` should be used so old shared credentials cannot be
read. Users must not be able to override offline mode through raw environment
configuration.

Provider credentials must also be excluded from terminal and MCP subprocess
environments. The Copilot command should name provider secret keys with
`--secret-env-vars`, and `AcpKernel`'s `terminal/create` environment builder
must start from a sanitized environment rather than the full provider-bearing
child environment.

## Implementation Milestones

### Milestone 0: Establish the upstream compatibility gate

1. Track `github/copilot-cli#4016` and `#4037`.
2. Identify the first stable Copilot CLI release that claims no-login BYOK
   support in ACP mode.
3. Run an isolated handshake probe with an empty `COPILOT_HOME`, no GitHub
   token variables, `COPILOT_OFFLINE=true`, and a local provider URL.
4. Require `initialize`, `session/new`, and one `session/prompt` turn to succeed
   without invoking `authenticate`.
5. Verify that the configured provider/model is used rather than a
   GitHub-hosted default.
6. Add a pinned `COPILOT_CLI_VERSION` build argument to
   `kernels/kernel_host/Dockerfile`; do not install an unbounded latest release.
7. Record the validated version and ACP transcript fixture in kernel tests.

**Exit criterion:** a released, pinned Copilot CLI passes the no-login ACP
smoke flow. If no release passes, stop here; do not add GitHub authentication.

### Milestone 1: Extend and validate Connections

1. Extend `ConnectionApiFlavor` mapping so AgentSpace's
   `chat_completions` value explicitly becomes Copilot's `completions`.
2. Add provider type and optional transport/Azure/header/bearer settings to
   `ConnectionRecord`, request models, summaries, and SQLite migrations.
3. Keep existing Connections backward compatible with OpenAI-compatible
   defaults.
4. Update `session_env` to emit provider-neutral `CONNECTION_*` keys, preserving
   the current precedence of kernel defaults, then Connection values, then
   agent values.
5. Validate provider-specific combinations:
   - URL and provider type are always required.
   - API key remains optional for local providers.
   - API key and bearer token cannot both be set.
   - WebSocket transport requires the Responses wire API.
   - Azure-only settings are rejected or ignored with an actionable error for
     non-Azure providers.
6. Update model discovery authentication and URL behavior per provider, while
   preserving manual model entry when discovery is unsupported.
7. Require a Connection when creating or updating a Copilot-backed ACP agent.
   Return a clear validation error before session creation rather than failing
   inside the kernel container.
8. Add store, route-contract, and agent-host proxy tests for schema migration,
   secret redaction, validation, and generated session environment.

Likely files:

- `services/client_service_rs/src/models.rs`
- `services/client_service_rs/src/api.rs`
- `services/client_service_rs/src/store/sqlite.rs`
- `services/client_service_rs/tests/route_contract.rs`
- `services/client_service_rs/tests/agent_host_proxy.rs`
- `clients/webui/src/types.ts`
- `clients/webui/src/api.ts`
- `clients/webui/src/ConnectionsView.tsx`

### Milestone 2: Add an ACP-server profile boundary

1. Refactor the OpenCode-specific logic out of the generic ACP transport into a
   small profile boundary responsible for:
   - command construction;
   - provider environment construction;
   - server-specific files;
   - server-specific system-prompt setup.
2. Add `copilot`, `opencode`, and `custom` profile selection through
   `KERNEL_ACP_SERVER`.
3. Preserve current OpenCode config generation under the OpenCode profile.
4. Keep `KERNEL_ACP_COMMAND` and `KERNEL_ACP_EXTRA_ARGS` only for the explicit
   `custom` profile so ordinary Copilot agents cannot accidentally bypass the
   required command and safety settings.
5. Keep the existing ACP JSON-RPC transport and event passthrough shared and
   unchanged where possible.
6. Move `_write_opencode_config()` and its API-key requirement entirely behind
   the OpenCode profile. This milestone is a hard dependency of the Copilot
   profile because Copilot must support local Connections without API keys.

Likely files:

- `kernels/kernel_acp/src/kernel_acp/__init__.py`
- new focused modules under `kernels/kernel_acp/src/kernel_acp/` if needed
- `kernels/kernel_acp/tests/test_acp.py`

### Milestone 3: Implement the Copilot ACP-server profile

1. Build exactly this base command:

   ```text
   copilot --acp --yolo
   ```

   Add `--disable-builtin-mcps` as defense in depth and
   `--secret-env-vars=...` for all provider credential/header keys. These do not
   replace the required base flags.

2. Validate that a Connection and `KERNEL_ACP_MODEL_NAME` are present.
3. Translate the provider-neutral Connection and model keys into the
   `COPILOT_PROVIDER_*` and `COPILOT_MODEL` variables in the table above.
4. Force `COPILOT_OFFLINE=true` after all overlays.
5. Strip GitHub token variables before spawning the child.
6. Clear direct `COPILOT_PROVIDER_*` and `COPILOT_MODEL` inputs, then populate
   only Connection/model-derived values.
7. Use a session-local `COPILOT_HOME` with no inherited login state.
8. Preserve AgentSpace system prompts by writing a session-local Copilot custom
   agent file under `$COPILOT_HOME/agents/` and adding `--agent agentspace` when
   a non-empty `KERNEL_SYSTEM_PROMPT` is present.
9. Keep `agent_host`'s generic ACP skills mount at
   `/workspace/.agents/skills`, then have the Copilot profile create
   `$COPILOT_HOME/skills` as a symlink to that mounted directory. This keeps
   ACP-server selection inside the kernel instead of teaching `agent_host`
   about `KERNEL_ACP_SERVER`, and it avoids sharing a credential-bearing home
   volume.
10. Preserve ACP session load/resume behavior based on the capabilities Copilot
   advertises; do not add Copilot-specific session parsing.
11. Surface missing/invalid provider configuration and ACP errors as normal
    AgentSpace session errors without exposing credentials.
12. Ensure ACP `terminal/create` and configured MCP servers do not inherit
    Connection or `COPILOT_PROVIDER_*` secrets unless a future explicit,
    allowlisted secret-forwarding feature is designed.

`--yolo` means Copilot should not normally send ACP permission requests. The
shared client-side permission implementation remains for other ACP profiles,
but Copilot's effective safety boundary is the kernel container and its
workspace mounts.

Unit tests should cover:

- exact command and flag order;
- forced offline mode;
- removal of all GitHub token variables;
- rejection of direct/raw Copilot provider environment overrides;
- every Connection/model mapping;
- `chat_completions` to `completions` translation;
- optional local-provider API keys;
- secret-free errors and logs;
- terminal and MCP subprocesses cannot read provider secrets;
- custom-agent creation and empty-prompt cleanup;
- ACP initialize/new/load/prompt/update behavior using captured Copilot
  transcripts.

### Milestone 4: Remove the legacy Copilot harness and login state

1. Delete `kernels/kernel_copilot/`, including its parser and tests.
2. Remove the legacy package from Docker copy steps and all Python workspace
   exclusion lists.
3. Remove `CopilotCli` / `copilot-cli` from Python and Rust harness enums,
   parsing, serialization, exhaustive tests, and UI defaults.
4. Migrate persisted agent rows from `harness=copilot-cli` to `harness=acp` and
   add `KERNEL_ACP_SERVER=copilot` to their environment.
5. For migrated agents without a Connection or model, preserve the record but
   block new sessions with an actionable configuration error.
6. Keep historical sessions readable; do not rewrite stored message/event
   history.
7. Remove the shared `/root/.copilot` volume, `AGENT_HOST_COPILOT_VOLUME`,
   `copilot_volume`, and related mount tests.
8. Remove the `CopilotCli`-specific skills mount arm and test the generic ACP
   mount plus the Copilot profile's session-local skills symlink.
9. Remove the standalone `setup` service and all script branches that instruct
   users to run `copilot login`.
10. Rename Copilot-specific compose/launcher files if they remain useful as
    generic ACP smoke tools.

Likely files and directories:

- `kernels/kernel_copilot/`
- `kernels/kernel_host/Dockerfile`
- `kernels/kernel_host/src/kernel_host/registry.py`
- `kernels/kernel_host/compose.copilot.yaml`
- `kernels/kernel_host/spawn-kernel.sh`
- `kernels/kernel_host/spawn-kernel.ps1`
- `pyproject.toml`
- `services/agent_host_rs/src/models.rs`
- `services/agent_host_rs/src/docker_runtime.rs`
- `services/agent_host_rs/src/skills.rs`
- `services/agent_host_rs/src/sessions.rs`
- `services/agent_host_rs/compose.yaml`
- `services/agent_host_rs/run-service.sh`
- `services/agent_host_rs/run-service.ps1`
- `services/client_service_rs/src/models.rs`
- `services/client_service_rs/src/agent_host.rs`
- `services/client_service_rs/src/store/sqlite.rs`
- `services/client_service_rs/src/api.rs`
- `compose.yaml`

### Milestone 5: Update the web UI and documentation

1. Make `acp` the only user-facing harness.
2. Add an ACP server selector with a clear `GitHub Copilot CLI` option; selecting
   it writes `KERNEL_ACP_SERVER=copilot`.
3. Require a Connection and model in the agent form for the Copilot profile.
4. Extend Connection create/edit forms with provider type and supported
   advanced settings, preserving password treatment for secret values.
5. Update recognized ACP keys and model-prefill logic.
6. Explain that `--yolo` grants all tool, path, and URL permissions inside the
   kernel container.
7. Explain that `COPILOT_OFFLINE=true` blocks GitHub integration but does not
   make a remote provider local.
8. Remove every login/setup instruction and every statement that the old
   `copilot-cli` kernel is the primary path.
9. Update architecture and operator documentation to describe ACP as the only
   stream protocol and Copilot as an ACP server profile.

Likely files:

- `clients/webui/src/AgentsView.tsx`
- `clients/webui/src/ConfigKernelsView.tsx`
- `clients/webui/src/ConnectionsView.tsx`
- `clients/webui/src/envPrefill.ts`
- `clients/webui/src/types.ts`
- `clients/webui/src/api.ts`
- `README.md`
- `AGENTS.md`
- relevant files under `docs/`

### Milestone 6: End-to-end verification and rollout

1. Add a local provider test double that supports streaming and tool calling.
2. Build the kernel image with the pinned Copilot CLI version.
3. Start a Copilot-backed ACP session with:
   - an empty, temporary `COPILOT_HOME`;
   - no GitHub token variables;
   - no Copilot credential volume;
   - a Connection pointing to the local provider;
   - `COPILOT_OFFLINE=true`.
4. Assert the ACP lifecycle, streamed assistant text, tool call/update events,
   filesystem changes, and prompt result.
5. Assert that the local provider receives the selected wire model and expected
   authentication format.
6. Assert that no request is made to GitHub. The test environment should deny
   GitHub egress rather than relying only on log inspection.
7. Verify reset and resume behavior.
8. Run `just check`.
9. Build the full Compose stack images.
10. Perform a manual web flow: create Connection, create Copilot ACP agent,
    start session, complete a tool-using turn, reset, and resume.

## Migration and Rollout Strategy

1. Land the upstream-version pin and compatibility test first.
2. Land Connection schema/API changes with backward-compatible defaults.
3. Land the ACP profile refactor while OpenCode remains the default for
   existing ACP agents.
4. Land the Copilot ACP profile and end-to-end test.
5. Migrate `copilot-cli` agents to `acp` plus
   `KERNEL_ACP_SERVER=copilot`.
6. Remove the legacy harness and shared login state only after migration tests
   prove existing databases still open and historical sessions remain readable.
7. Make new ACP agents choose an ACP server explicitly; do not silently route
   an incomplete Copilot configuration to OpenCode or GitHub.

## Acceptance Criteria

- The only public kernel harness is `acp`; `copilot-cli` is rejected as a new
  harness value.
- GitHub Copilot CLI is invoked through `AcpKernel` as
  `copilot --acp --yolo`.
- `kernels/kernel_copilot` and its JSONL parser no longer exist.
- A Copilot-backed agent cannot be created or started without a Connection and
  model.
- Connection URL, provider type, wire API, and credentials reach Copilot under
  the correct `COPILOT_PROVIDER_*` names.
- `COPILOT_OFFLINE` is always `true` and cannot be overridden.
- `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, and `GITHUB_TOKEN` are absent from the
  Copilot child process.
- No GitHub login/setup UI, API, script, documentation, or shared credential
  volume remains.
- OpenCode ACP behavior remains available through its ACP-server profile.
- Existing `copilot-cli` agents migrate to ACP/Copilot without making
  historical sessions unreadable.
- A no-login, no-GitHub-egress end-to-end test completes an ACP tool-using turn
  against a local provider.
- Repository checks and Compose image builds pass.

## Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Copilot ACP continues requiring GitHub login | Hard milestone-0 release gate; do not weaken the no-login requirement. |
| Copilot changes ACP or provider behavior between releases | Pin the validated CLI release and update it through an explicit compatibility test. |
| Raw agent environment overrides offline or credential policy | Apply forced settings last and remove GitHub token keys in the Copilot profile. |
| Old shared Copilot state authenticates unexpectedly | Remove the shared home volume and use an empty session-local `COPILOT_HOME`. |
| Provider claims OpenAI compatibility but lacks tools/streaming | Validate with a model capability smoke turn and return the provider error without fallback. |
| Provider model name differs from known model configuration | Support separate model ID and wire model settings. |
| `/models` discovery is provider-specific | Keep manual model entry and make discovery best-effort per provider type. |
| `--yolo` grants broad permissions | Keep isolation at the kernel container/workspace boundary and document the behavior prominently. |
| Connection migration leaves old Copilot agents incomplete | Preserve records, mark required fields in UI, and fail session creation with actionable validation. |
| Secret values leak in API responses or logs | Return presence metadata only, redact spawn/debug output, and add explicit negative tests. |

## Open Upstream Question

Which stable Copilot CLI release will make BYOK-only ACP sessions pass
`session/new` without GitHub authentication? Until that is answered by a
release and verified with
`RUN_COPILOT_ACP_COMPATIBILITY=1 uv run pytest
kernels/kernel_acp/tests/test_acp.py`, the experimental profile remains
intentionally disabled.
