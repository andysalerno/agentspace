---
name: evolve-durable-sessions
description: Add or change durable AgentSpace session fields without breaking persistence, recovery, APIs, or the WebUI.
---

# Skill: Evolving Durable Sessions

When changing session state, update the whole contract:

1. Define the field and public shape in `services/client_service_rs/src/models.rs`.
2. Add a backward-compatible SQLite migration in `services/client_service_rs/src/store/sqlite.rs`. Give old rows an explicit safe default; never infer recoverability from missing data.
3. Wire creation, updates, summaries, recovery, and cleanup in `services/client_service_rs/src/api.rs`.
4. Propagate runtime identity/state through `agent_host` when the field affects containers, volumes, or adoption.
5. Update WebUI types, queries, and every mode-specific view that reads the session.
6. Test old-row migration, API round trips, restart/recovery behavior, and the affected UI flow.

Keep durable intent separate from observed runtime state. Persist stable IDs and immutable launch inputs; rediscover ephemeral container/process details.

Finish with `just check` and, for runtime fields, a real container restart/adoption test.
