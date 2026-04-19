# Discord Gateway — Implementation Plan

## Scope (locked)

- **Single 1:1 DM only.** No guild channels, no threads, no group DMs, no slash
  commands, no components, no voice — explicitly out of scope for v1.
- **One gateway instance == one Discord bot == one DM partner == one
  client_service session.** The session is created lazily on the first inbound
  DM and reused for the lifetime of the gateway record.
- **One-shot replies** to start; agent emits one assistant message → gateway
  splits it into N Discord messages with a small typing-indicator delay between
  chunks to feel natural.
- **Owner-only DM allowlist via env var.** No pairing flow, no allowlist UI —
  just `DISCORD_OWNER_USER_ID`.

Anything beyond this (per-channel sessions, mention-routing in guilds, slash
commands, streaming via message edits, threads, voice, multi-account) is a
follow-up and intentionally not built into v1.

## Configuration surface

These are entered through the existing GatewaysView when the user creates a
`discord` gateway. No new UI is required.

### Secrets (stored in `gateways.secrets_json`, never returned by the API)

| Key | Required | Purpose |
|---|---|---|
| `DISCORD_BOT_TOKEN` | yes | Bot token from the Discord Developer Portal |

### Env vars (stored in `gateways.env_vars` as plaintext `.env`)

| Key | Required | Default | Purpose |
|---|---|---|---|
| `DISCORD_OWNER_USER_ID` | yes | — | Discord user ID (snowflake) of the only user the bot will reply to |
| `DISCORD_TYPING_DELAY_MS` | no | `600` | Delay between chunks (typing indicator shown during this) |
| `DISCORD_CHUNK_MAX_CHARS` | no | `1900` | Hard cap per outbound Discord message (Discord limit is 2000; leave headroom) |

These three keys are the entire user-facing configuration. Server IDs / guild
allowlists are intentionally absent because we ignore guilds entirely.

## Architecture

Mirror `gateway_echo` exactly.

### New package: `gateways/gateway_discord/`

```
gateways/gateway_discord/
├── pyproject.toml         # depends on: gateway, discord.py>=2.4
├── src/gateway_discord/
│   ├── __init__.py        # exports DiscordGateway
│   └── discord_gateway.py # the implementation
└── tests/
    └── test_gateway_discord.py
```

### `DiscordGateway` class — implements `Gateway` Protocol

State (per the existing protocol):

```python
class DiscordGateway:
    name           -> "discord"
    status         -> GatewayStatus  (stopped | starting | running | error)
    last_error     -> str | None
    _client        : discord.Client | None
    _client_task   : asyncio.Task | None     # discord.Client.start()
    _config        : GatewayConfig | None
    _session_id    : str | None              # the one client_service session
    _owner_id      : int | None
    _typing_delay  : float
    _chunk_max     : int
    _send_lock     : asyncio.Lock            # serialise outbound bursts
    _events        : deque[GatewayEvent]     # ring buffer for /gateway/events
```

### Lifecycle

`async def start(config: GatewayConfig) -> None`:
1. Read `DISCORD_BOT_TOKEN` from `config.env` (it's merged from secrets
   upstream); error if missing → set ERROR + last_error.
2. Read `DISCORD_OWNER_USER_ID` (int); error if missing/non-numeric.
3. Read optional knobs with defaults.
4. Construct `discord.Client` with `intents = discord.Intents.default()` plus
   `message_content=True`. (We do **not** need `members` or `presence` for v1.)
5. Register `@client.event on_message` and `on_ready`.
6. Spawn `self._client_task = asyncio.create_task(client.start(token))`.
7. Wait for `on_ready` (with a timeout — say 30s) before flipping
   `status = RUNNING`. If timeout or login error → ERROR.

`async def stop() -> None`:
1. `await self._client.close()` (idempotent, gracefully closes the WSS).
2. Cancel `_client_task` if still alive; await with `contextlib.suppress`.
3. `status = STOPPED`.

`extra_router() -> APIRouter`:
- `GET /gateway/status` → `{status, owner_id, session_id, last_error, latency_ms}`.
- `GET /gateway/events` → ring buffer of `GatewayEvent`s (same shape as echo).
- No inbox endpoint — Discord pushes inbound, not us.

### Inbound flow (`on_message`)

```
on_message(msg):
  if msg.author.bot: ignore
  if msg.guild is not None: ignore        # DM-only, v1
  if msg.author.id != self._owner_id: ignore (record EVENT type=blocked)
  if msg.content is empty: ignore (attachments not handled in v1)

  record EVENT inbound

  async with self._send_lock:
    session_id = await self._ensure_session()
    try:
      response = await config.client.send_message(
          session_id=session_id, message=msg.content)
    except Exception as exc:
      status=ERROR, last_error=str(exc), record EVENT error
      await msg.channel.send("⚠️ agent error — check gateway logs")
      raise

  reply = extract_assistant_text(response)
  await self._send_chunked(msg.channel, reply)
  record EVENT outbound
```

`_ensure_session` is dead simple — single session for life of the gateway:

```python
async def _ensure_session(self) -> str:
    if self._session_id is not None:
        return self._session_id
    session = await self._config.client.create_session(
        agent_id=self._config.agent_id,
        channel_name=f"discord:dm:{self._owner_id}",
    )
    self._session_id = str(session["session_id"])
    return self._session_id
```

(No client_service API change required for v1 — we keep `_session_id` in
memory. If the gateway container restarts mid-conversation a new session is
created; that's an accepted v1 limitation, identical to how `gateway_echo`
behaves today.)

### Outbound chunking + typing indicator

```python
async def _send_chunked(channel, text):
    chunks = _chunk(text, self._chunk_max)
    for i, chunk in enumerate(chunks):
        if i > 0:
            async with channel.typing():
                await asyncio.sleep(self._typing_delay)
        await channel.send(chunk)
```

`_chunk(text, max_chars)`:
- Split on paragraph boundaries (`\n\n`) first.
- If a paragraph still exceeds `max_chars`, split on `\n`.
- If a single line still exceeds `max_chars`, hard-split at `max_chars`.
- Never produce an empty chunk.
- Pure function, fully unit-testable without Discord at all.

The typing indicator before chunk 0 is skipped because the first `send` should
be near-immediate (the agent already kept the user waiting).

### Error handling and status transitions

Same model the code review settled on for echo:

| Event | `status` | `last_error` |
|---|---|---|
| Token missing / bad | ERROR | descriptive string |
| `on_ready` not seen in 30s | ERROR | "discord login timed out" |
| WSS disconnect → discord.py auto-reconnects | RUNNING (untouched) | unchanged |
| `client_service.send_message` raises | ERROR | str(exc), reply user-facing apology |
| `discord` send raises (rate limit, permission, etc.) | RUNNING | str(exc), record event but keep gateway alive |
| `stop()` called | STOPPED | unchanged |

Once in ERROR the gateway stays in ERROR; recovery is a manual restart from
the webui (consistent with echo).

## Code changes outside the new package

| File | Change |
|---|---|
| [gateways/gateway/src/gateway/protocol.py](gateways/gateway/src/gateway/protocol.py) | add `DISCORD = "discord"` to `GatewayType` |
| [gateways/gateway_host/pyproject.toml](gateways/gateway_host/pyproject.toml) | add `gateway-discord` dep |
| [gateways/gateway_host/src/gateway_host/registry.py](gateways/gateway_host/src/gateway_host/registry.py) | register `GatewayType.DISCORD: DiscordGateway` |
| [gateways/gateway_host/Dockerfile](gateways/gateway_host/Dockerfile) | `COPY gateways/gateway_discord ...` before the `uv sync` step |
| [pyproject.toml](pyproject.toml) (workspace) | add `gateway-discord = { workspace = true }` and add `gateways/gateway_discord/src` to pyright `extraPaths` |

No webui changes. No `client_service` changes. No `agent_host` changes. The
new gateway type appears automatically in the create-gateway dropdown via
`/gateway-types`, which already enumerates `GatewayType`.

## Testing strategy

We can't (and won't) talk to real Discord in tests. Two layers:

### Pure-function unit tests (no Discord at all)
- `_chunk(text, max_chars)`: paragraph split, line split, hard split, empty
  string, single short paragraph, exactly-at-boundary, multi-paragraph mixed.
  Probably 6-8 cases, all instant.

### Integration tests with a fake `discord.Client`
- Construct `DiscordGateway`, monkeypatch `discord.Client` with a stub that:
  - records `start(token)` and `close()` calls
  - lets the test fire `on_message` synthetically
  - exposes a fake `Channel` whose `send` records calls and whose `typing()`
    is a no-op async context manager
- Drive `start()` → fire DM from owner → assert `client_service.send_message`
  was called and the right number of `channel.send` chunks were emitted.
- Drive `start()` → fire DM from non-owner → assert no calls.
- Drive `start()` → fire message in a guild (msg.guild non-None) → assert
  ignored.
- Drive `start()` → make `client_service.send_message` raise → assert status
  becomes ERROR, last_error is set, an apology is sent, and a subsequent
  inbound DM is also blocked or also fails cleanly (we'll pick one — easiest
  is "stays ERROR until restart, subsequent DMs get the apology").
- Stop → assert close was called and status is STOPPED.

`FakeClient` for `ClientServiceClient` already exists in
`gateways/gateway_echo/tests/test_gateway_echo.py`; we'll lift it into a
shared `tests/conftest.py` or duplicate it locally — duplicate is fine for v1.

## Local dev / live test plan

1. `uv sync` to pull `discord.py` into the workspace lockfile.
2. Build images: `just stack-build`.
3. Bring stack up: `just stack-up`.
4. In webui:
   - Create an agent if needed.
   - Create a gateway with type=`discord`, agent=that agent,
     `DISCORD_OWNER_USER_ID=<your-snowflake>`, secret
     `DISCORD_BOT_TOKEN=<token>`, enabled=true.
5. DM the bot from the owner account. Expect a reply.
6. Verify in webui logs panel: status=running, events show inbound/outbound.
7. Send a long message that exceeds `DISCORD_CHUNK_MAX_CHARS`; verify
   chunking + typing indicator.

## Risks / things to double-check during implementation

1. **`discord.py` event loop coexistence.** `discord.Client.start()` is an
   async coroutine that runs forever. Running it as a `create_task` inside
   the gateway_host's existing FastAPI loop is the pattern discord.py
   officially supports, but we should verify we shut it down cleanly during
   `stop()` so the gateway_host container can exit. The `await client.close()`
   then `task.cancel()` pattern is what their docs recommend.
2. **discord.py's `Client.start` swallows `KeyboardInterrupt` differently
   than running it via `client.run()`.** Using `start` is correct for our
   embedded use; just need to confirm exceptions surface.
3. **Image size.** discord.py is pure-python with a small dep tree
   (aiohttp, yarl already pulled in by httpx transitively for the most
   part). Should be a small bump to the gateway image.
4. **Token leakage.** Already handled by `secrets` flow — token is in
   `os.environ` inside the container only, never in client_service responses.
   We must not log the token. Use `logger.info("token loaded (length=%d)", len(token))`,
   never the token itself.
5. **Privileged intents.** We only enable `message_content`. The user must
   toggle this on in the Developer Portal **before** the bot will receive
   any DM text — call this out in error messaging if `on_message` arrives
   with empty `content` and the bot is enabled.
6. **Rate limits.** discord.py handles HTTP-level 429s for us; the typing
   delay between chunks also helps avoid bursting. No extra work needed for v1.

## Out of scope (filed for future iterations)

These are intentionally **not** built. Each is a clean follow-up:

- Per-channel sessions (`channel_name = "discord:guild:<g>:channel:<c>"`)
- Streaming via message edits
- Slash commands / interactive components
- Pairing-code DM allowlist
- Mention-only mode in guild channels
- Voice channels
- Threads / forums
- Multi-owner allowlists
- Inbound attachment handling
- Outbound file/embed attachments
- Reactions

## Estimated work breakdown

1. Add `GatewayType.DISCORD` enum value + workspace plumbing.
2. Create `gateways/gateway_discord/` package skeleton with pyproject.
3. Implement `_chunk` pure function + tests.
4. Implement `DiscordGateway` class, FakeClient harness, and
   integration tests.
5. Wire into `gateway_host` registry + Dockerfile.
6. Run full test suite + lint sweep.
7. Live smoke test against a real Discord bot in a personal server.
8. Commit.

Each step is independently verifiable. Step 7 requires a real bot token, so
that's the only part that can't be done purely from the dev box.
