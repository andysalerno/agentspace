# ACP Agent Backends

The `acp` harness speaks [Agent Client Protocol](https://agentclientprotocol.com)
(JSON-RPC over stdio) to an agent server. AgentSpace ships more than one server:
select it per agent with `KERNEL_ACP_AGENT`.

| `KERNEL_ACP_AGENT` | Server command | Underlying agent |
| --- | --- | --- |
| `opencode` (default) | `opencode acp` | [opencode](https://opencode.ai) |
| `pi` | `pi-acp` | [pi](https://github.com/earendil-works/pi) via the [`pi-acp`](https://github.com/svkozak/pi-acp) adapter |

pi has no native ACP mode by design, so the `pi` backend launches `pi-acp`, the
adapter listed in the ACP registry. It spawns `pi --mode rpc` and translates pi's
RPC stream into ACP `session/update` notifications.

## Shared configuration

Both backends are configured from the session's Connection plus the agent's
system prompt. The kernel writes the agent's native config files before spawning
it, so no agent-specific credentials setup is required.

| Environment variable | Purpose |
| --- | --- |
| `CONNECTION_URL` | Model endpoint base URL (required) |
| `CONNECTION_API_KEY` | Model endpoint API key (required) |
| `KERNEL_ACP_MODEL_NAME` | Model id sent to the endpoint (required) |
| `CONNECTION_API_FLAVOR` | `chat_completions` (default) or `responses` |
| `KERNEL_SYSTEM_PROMPT` | Replaces the agent's default system prompt |
| `KERNEL_ACP_AGENT` | Backend to launch (`opencode` or `pi`) |
| `KERNEL_ACP_COMMAND` | Overrides the launched command entirely |
| `KERNEL_ACP_EXTRA_ARGS` | Newline-separated extra arguments |
| `KERNEL_ACP_WORKSPACE_DIR` | Working directory (default `/workspace`) |
| `KERNEL_ACP_MCP_SERVERS` | JSON array passed to `session/new` |
| `KERNEL_ACP_PERMISSION_OPTION` | Preferred `session/request_permission` option id |
| `KERNEL_ACP_SKILLS_DIR` | Skills mount exposed to `pi` (default `/workspace/.agents/skills`) |

## What each backend writes

`opencode`:

- `~/.config/opencode/opencode.json` — `customprovider` provider, model, and
  permission policy.
- `~/.config/opencode/agents/custom.md` — the system prompt as a primary agent,
  selected through `OPENCODE_CONFIG_CONTENT`.

`pi` (under `PI_CODING_AGENT_DIR`, default `~/.pi/agent`):

- `models.json` — `customprovider` with the Connection's base URL, API key, and
  model, using `openai-completions` or `openai-responses` per API flavor. pi
  reads `apiKey` as an expression (`$NAME` interpolates from the environment, a
  leading `!` runs a shell command), so the key is escaped before it is written.
- `settings.json` — pins `defaultProvider`/`defaultModel`, and lists the
  AgentSpace skills mount (`/workspace/.agents/skills`, override with
  `KERNEL_ACP_SKILLS_DIR`) under `skills`.
- `SYSTEM.md` — the system prompt, removed when no prompt is configured.

The backend also defaults `PI_OFFLINE`, `PI_SKIP_VERSION_CHECK`, and
`PI_TELEMETRY` so pi does not make startup network calls; set them explicitly on
the agent to override.

## Notes

- pi's project trust stays off (`defaultProjectTrust: "never"`). Trusting the
  workspace would also load its `.pi/settings.json`, extensions, and packages,
  which pi executes at startup with the kernel's environment — including the
  Connection API key — before the model takes a turn. Naming the skills
  directory explicitly loads AgentSpace's skills as data without granting the
  workspace that reach.
- pi executes tools in-process rather than delegating through ACP `fs/*` and
  `terminal/*`, so its shell output arrives as incremental
  `_meta.terminal_output` updates on the tool call. AgentSpace appends those to
  the tool call output.
- The `pi-acp` adapter accepts `KERNEL_ACP_MCP_SERVERS` in `session/new` but
  does not forward MCP servers to pi.
- Both CLIs are preinstalled in the `kernel_host` image at pinned versions.
